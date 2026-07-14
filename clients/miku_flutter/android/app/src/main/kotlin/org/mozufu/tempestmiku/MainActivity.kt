package org.mozufu.tempestmiku

import android.Manifest
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.os.Build
import android.os.Bundle
import androidx.annotation.NonNull
import io.flutter.embedding.android.FlutterActivity
import io.flutter.embedding.engine.FlutterEngine
import io.flutter.plugin.common.MethodChannel
import io.flutter.plugin.common.EventChannel
import org.unifiedpush.android.connector.UnifiedPush

private const val ACTION_APPROVAL_DECISION =
    "org.mozufu.tempestmiku.APPROVAL_NOTIFICATION_DECISION"
private const val EXTRA_SESSION_ID = "sessionId"
private const val EXTRA_APPROVAL_ID = "approvalId"
private const val EXTRA_DECISION = "decision"
private const val URI_GRANT_FLAGS =
    Intent.FLAG_GRANT_READ_URI_PERMISSION or
        Intent.FLAG_GRANT_WRITE_URI_PERMISSION or
        Intent.FLAG_GRANT_PERSISTABLE_URI_PERMISSION or
        Intent.FLAG_GRANT_PREFIX_URI_PERMISSION

class MainActivity : FlutterActivity() {
    companion object {
        private const val CHANNEL = "org.mozufu.tempestmiku/notifications"
        private const val ACTION_CHANNEL =
            "org.mozufu.tempestmiku/notification-actions"
        private const val UNIFIED_PUSH_CHANNEL =
            "org.mozufu.tempestmiku/unified-push-events"
        private const val SHARE_IMPORT_CHANNEL =
            "org.mozufu.tempestmiku/share-imports"
        private const val REQUEST_NOTIFICATIONS = 701
    }

    private var permissionResult: MethodChannel.Result? = null
    private var actionSink: EventChannel.EventSink? = null
    private var shareImportSink: EventChannel.EventSink? = null
    private var pendingShareImport: Map<String, Any>? = null
    private val pendingActions = mutableListOf<Map<String, Any>>()

    override fun configureFlutterEngine(@NonNull flutterEngine: FlutterEngine) {
        super.configureFlutterEngine(flutterEngine)
        MethodChannel(flutterEngine.dartExecutor.binaryMessenger, CHANNEL).setMethodCallHandler {
                call,
                result ->
            when (call.method) {
                "initialize" -> {
                    ApprovalNotifications.ensureChannel(this)
                    result.success(null)
                }
                "requestPermission" -> requestNotificationPermission(result)
                "showApproval" -> {
                    val sessionId = call.argument<String>("sessionId")
                    val approvalId = call.argument<String>("approvalId")
                    val action = call.argument<String>("action")
                    if (sessionId.isNullOrBlank() || approvalId.isNullOrBlank()) {
                        result.error(
                            "invalid_approval",
                            "sessionId and approvalId are required",
                            null,
                        )
                    } else {
                        ApprovalNotifications.show(
                            this,
                            sessionId,
                            approvalId,
                            action.orEmpty(),
                        )
                        result.success(null)
                    }
                }
                "cancelApproval" -> {
                    call.argument<String>("approvalId")?.let { approvalId ->
                        ApprovalNotifications.cancel(this, approvalId)
                    }
                    result.success(null)
                }
                "registerUnifiedPush" -> {
                    UnifiedPush.tryUseCurrentOrDefaultDistributor(this) { success ->
                        if (success) {
                            UnifiedPush.register(
                                this,
                                messageForDistributor = "TempestMiku approval alerts",
                            )
                        } else {
                            UnifiedPushEvents.emit(mapOf("type" to "registrationFailed"))
                        }
                        result.success(null)
                    }
                }
                "unregisterUnifiedPush" -> {
                    UnifiedPush.unregister(this)
                    UnifiedPushEvents.emit(mapOf("type" to "unregistered"))
                    result.success(null)
                }
                else -> result.notImplemented()
            }
        }
        EventChannel(flutterEngine.dartExecutor.binaryMessenger, ACTION_CHANNEL)
            .setStreamHandler(
                object : EventChannel.StreamHandler {
                    override fun onListen(arguments: Any?, events: EventChannel.EventSink) {
                        actionSink = events
                        pendingActions.toList().forEach(events::success)
                        pendingActions.clear()
                    }

                    override fun onCancel(arguments: Any?) {
                        actionSink = null
                    }
                },
            )
        EventChannel(flutterEngine.dartExecutor.binaryMessenger, UNIFIED_PUSH_CHANNEL)
            .setStreamHandler(
                object : EventChannel.StreamHandler {
                    override fun onListen(arguments: Any?, events: EventChannel.EventSink) {
                        UnifiedPushEvents.listen(events)
                    }

                    override fun onCancel(arguments: Any?) {
                        UnifiedPushEvents.cancel()
                    }
                },
            )
        EventChannel(flutterEngine.dartExecutor.binaryMessenger, SHARE_IMPORT_CHANNEL)
            .setStreamHandler(
                object : EventChannel.StreamHandler {
                    override fun onListen(arguments: Any?, events: EventChannel.EventSink) {
                        shareImportSink = events
                        pendingShareImport?.let(events::success)
                        pendingShareImport = null
                    }

                    override fun onCancel(arguments: Any?) {
                        shareImportSink = null
                    }
                },
            )
        handleNotificationIntent(intent)
        handleTextImportIntent(intent)
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        handleNotificationIntent(intent)
        handleTextImportIntent(intent)
    }

    override fun onRequestPermissionsResult(
        requestCode: Int,
        permissions: Array<out String>,
        grantResults: IntArray,
    ) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults)
        if (requestCode != REQUEST_NOTIFICATIONS) return
        val granted = grantResults.firstOrNull() == PackageManager.PERMISSION_GRANTED
        permissionResult?.success(granted)
        permissionResult = null
    }

    private fun requestNotificationPermission(result: MethodChannel.Result) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU) {
            result.success(true)
            return
        }
        if (checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) ==
            PackageManager.PERMISSION_GRANTED
        ) {
            result.success(true)
            return
        }
        if (permissionResult != null) {
            result.error("permission_in_progress", "notification permission request is already active", null)
            return
        }
        permissionResult = result
        requestPermissions(arrayOf(Manifest.permission.POST_NOTIFICATIONS), REQUEST_NOTIFICATIONS)
    }

    private fun handleNotificationIntent(intent: Intent?) {
        if (intent?.action != ACTION_APPROVAL_DECISION) return
        val sessionId = intent.getStringExtra(EXTRA_SESSION_ID).orEmpty()
        val approvalId = intent.getStringExtra(EXTRA_APPROVAL_ID).orEmpty()
        val decision = intent.getStringExtra(EXTRA_DECISION).orEmpty()
        if (sessionId.isEmpty() || approvalId.isEmpty() || decision !in setOf("approve", "deny")) {
            return
        }
        val payload = mapOf(
            "sessionId" to sessionId,
            "approvalId" to approvalId,
            "decision" to decision,
            "requiresConfirmation" to (Build.VERSION.SDK_INT < Build.VERSION_CODES.S),
        )
        val sink = actionSink
        if (sink == null) {
            pendingActions.removeAll { queued ->
                queued["approvalId"] == approvalId
            }
            pendingActions.add(payload)
        } else {
            sink.success(payload)
        }
        intent.action = Intent.ACTION_MAIN
        intent.removeExtra(EXTRA_SESSION_ID)
        intent.removeExtra(EXTRA_APPROVAL_ID)
        intent.removeExtra(EXTRA_DECISION)
    }

    private fun handleTextImportIntent(intent: Intent?) {
        val textImportIntent = intent ?: return
        if (textImportIntent.action !in setOf(Intent.ACTION_SEND, Intent.ACTION_PROCESS_TEXT)) return
        val clipContainsUri = textImportIntent.clipData?.let { clip ->
            (0 until clip.itemCount).any { index -> clip.getItemAt(index).uri != null }
        } ?: false
        val hasUriPayload =
            textImportIntent.flags and URI_GRANT_FLAGS != 0 ||
                textImportIntent.hasExtra(Intent.EXTRA_STREAM) ||
                clipContainsUri
        val parsed = TextImportIntentParser.parse(
            action = textImportIntent.action,
            mimeType = textImportIntent.type,
            sharedText = textImportIntent.getCharSequenceExtra(Intent.EXTRA_TEXT),
            selectedText = textImportIntent.getCharSequenceExtra(Intent.EXTRA_PROCESS_TEXT),
            subject = textImportIntent.getCharSequenceExtra(Intent.EXTRA_SUBJECT),
            hasUriPayload = hasUriPayload,
        )
        if (parsed != null) {
            val payload = parsed.toEvent()
            val sink = shareImportSink
            if (sink == null) {
                pendingShareImport = payload
            } else {
                sink.success(payload)
            }
        }
        textImportIntent.action = Intent.ACTION_MAIN
        textImportIntent.removeExtra(Intent.EXTRA_TEXT)
        textImportIntent.removeExtra(Intent.EXTRA_PROCESS_TEXT)
        textImportIntent.removeExtra(Intent.EXTRA_PROCESS_TEXT_READONLY)
        textImportIntent.removeExtra(Intent.EXTRA_SUBJECT)
        textImportIntent.removeExtra(Intent.EXTRA_STREAM)
        textImportIntent.clipData = null
        textImportIntent.flags = textImportIntent.flags and URI_GRANT_FLAGS.inv()
    }

}

internal object ApprovalNotifications {
    private const val APPROVAL_CHANNEL_ID = "approval_requests"
    private const val APPROVAL_CHANNEL_NAME = "Approval requests"

    fun ensureChannel(context: Context) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
        val channel = NotificationChannel(
            APPROVAL_CHANNEL_ID,
            APPROVAL_CHANNEL_NAME,
            NotificationManager.IMPORTANCE_HIGH,
        ).apply {
            description = "Alerts when TempestMiku needs an approval."
            setShowBadge(true)
        }
        notificationManager(context).createNotificationChannel(channel)
    }

    fun show(context: Context, sessionId: String, approvalId: String, action: String) {
        ensureChannel(context)
        val openApp = Intent(context, MainActivity::class.java).apply {
            flags = Intent.FLAG_ACTIVITY_CLEAR_TOP or Intent.FLAG_ACTIVITY_SINGLE_TOP
        }
        val contentIntent = PendingIntent.getActivity(
            context,
            notificationId(approvalId),
            openApp,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
        val approve = notificationAction(context, sessionId, approvalId, "approve", "Approve once")
        val deny = notificationAction(context, sessionId, approvalId, "deny", "Deny")
        val builder = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            Notification.Builder(context, APPROVAL_CHANNEL_ID)
        } else {
            Notification.Builder(context)
        }
        val publicNotification = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            Notification.Builder(context, APPROVAL_CHANNEL_ID)
        } else {
            Notification.Builder(context)
        }.setSmallIcon(R.mipmap.ic_launcher)
            .setContentTitle("TempestMiku needs your approval")
            .setContentText("Unlock to review the request.")
            .build()
        val notification = builder
            .setSmallIcon(R.mipmap.ic_launcher)
            .setContentTitle("TempestMiku needs your approval")
            .setContentText(sanitizeAction(action))
            .setCategory(Notification.CATEGORY_STATUS)
            .setAutoCancel(true)
            .setOnlyAlertOnce(true)
            .setContentIntent(contentIntent)
            .setVisibility(Notification.VISIBILITY_PRIVATE)
            .setPublicVersion(publicNotification)
            .addAction(approve)
            .addAction(deny)
            .build()
        notificationManager(context).notify(notificationId(approvalId), notification)
    }

    fun cancel(context: Context, approvalId: String) {
        notificationManager(context).cancel(notificationId(approvalId))
    }

    private fun notificationManager(context: Context): NotificationManager =
        context.getSystemService(NotificationManager::class.java)

    private fun notificationAction(
        context: Context,
        sessionId: String,
        approvalId: String,
        decision: String,
        title: String,
    ): Notification.Action {
        val intent = Intent(context, MainActivity::class.java).apply {
            action = ACTION_APPROVAL_DECISION
            flags = Intent.FLAG_ACTIVITY_CLEAR_TOP or Intent.FLAG_ACTIVITY_SINGLE_TOP
            putExtra(EXTRA_SESSION_ID, sessionId)
            putExtra(EXTRA_APPROVAL_ID, approvalId)
            putExtra(EXTRA_DECISION, decision)
        }
        val pendingIntent = PendingIntent.getActivity(
            context,
            "$approvalId:$decision".hashCode(),
            intent,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
        val builder = Notification.Action.Builder(null, title, pendingIntent)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            builder.setAuthenticationRequired(true)
        }
        return builder.build()
    }

    private fun sanitizeAction(action: String): String {
        val sanitized = action
            .filterNot(Char::isISOControl)
            .trim()
            .take(160)
        return sanitized.ifEmpty { "Open the app to review a pending request." }
    }

    private fun notificationId(approvalId: String): Int = approvalId.hashCode()
}
