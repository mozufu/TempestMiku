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
import android.icu.text.Transliterator
import android.util.Log
import java.io.File
import androidx.annotation.NonNull
import io.flutter.embedding.android.FlutterActivity
import io.flutter.embedding.engine.FlutterEngine
import io.flutter.plugin.common.MethodChannel
import io.flutter.plugin.common.EventChannel
import org.unifiedpush.android.connector.UnifiedPush

private const val ACTION_APPROVAL_DECISION =
    "org.mozufu.tempestmiku.APPROVAL_NOTIFICATION_DECISION"
internal const val EXTRA_SESSION_ID = "sessionId"
internal const val EXTRA_APPROVAL_ID = "approvalId"
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
        private const val VOICE_CAPTURE_CHANNEL =
            "org.mozufu.tempestmiku/voice-capture"
        private const val VOICE_MODEL_CHANNEL =
            "org.mozufu.tempestmiku/voice-model"
        private const val REQUEST_NOTIFICATIONS = 701
        private const val REQUEST_RECORD_AUDIO = 702
        private val VOICE_CAPTURE_PROCESS_LOCK = Any()
        private var processVoiceCapture: ForegroundVoiceCapture? = null
        private val APP_BUILD_FINGERPRINT_CACHE = AppBuildFingerprintCache()
    }

    private var permissionResult: MethodChannel.Result? = null
    private var voicePermissionResult: MethodChannel.Result? = null
    private var isActivityForeground = false
    @Volatile private var voiceModelOperationActive = false
    private var actionSink: EventChannel.EventSink? = null
    private var shareImportSink: EventChannel.EventSink? = null
    private val pendingShareImports = SinglePendingEventBuffer<Map<String, Any>>()
    private val pendingActions = mutableListOf<Map<String, Any>>()
    private val voiceCapture: ForegroundVoiceCapture by lazy {
        synchronized(VOICE_CAPTURE_PROCESS_LOCK) {
            processVoiceCapture
                ?: ForegroundVoiceCapture(
                    VoiceCaptureFiles(File(noBackupFilesDir, "voice_capture")),
                ).also { processVoiceCapture = it }
        }
    }
    private val voiceModels: VoiceModelInstaller by lazy {
        VoiceModelInstaller(File(noBackupFilesDir, "voice_models"))
    }

    override fun configureFlutterEngine(@NonNull flutterEngine: FlutterEngine) {
        super.configureFlutterEngine(flutterEngine)
        MethodChannel(flutterEngine.dartExecutor.binaryMessenger, CHANNEL).setMethodCallHandler {
                call,
                result ->
            when (call.method) {
                "initialize" -> {
                    ApprovalNotifications.ensureChannel(this)
                    SessionNotifications.ensureChannel(this)
                    result.success(null)
                }
                "configureInlineReply" -> {
                    val serverBaseUrl = call.argument<String>("serverBaseUrl")
                    val deviceToken = call.argument<String>("deviceToken")
                    if (serverBaseUrl == null || deviceToken == null) {
                        InlineReplySecretStore.clearAuthority(this)
                        result.success(null)
                    } else if (InlineReplySecretStore.saveAuthority(this, serverBaseUrl, deviceToken)) {
                        result.success(null)
                    } else {
                        result.error("invalid_reply_authority", "reply authority was rejected", null)
                    }
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
                        pendingShareImports.drain(events::success)
                    }

                    override fun onCancel(arguments: Any?) {
                        shareImportSink = null
                    }
                },
            )
        MethodChannel(flutterEngine.dartExecutor.binaryMessenger, VOICE_CAPTURE_CHANNEL)
            .setMethodCallHandler { call, result ->
                when (call.method) {
                    "inspectBuild" -> inspectVoiceBuild(result)
                    "recover" -> recoverVoiceCapture(result)
                    "requestPermission" -> requestVoicePermission(result)
                    "start" -> startVoiceCapture(call.argument<String>("captureId"), result)
                    "stop" -> stopVoiceCapture(call.argument<String>("captureId"), result)
                    "cancel" -> cancelVoiceCapture(call.argument<String>("captureId"), result)
                    else -> result.notImplemented()
                }
            }
        MethodChannel(flutterEngine.dartExecutor.binaryMessenger, VOICE_MODEL_CHANNEL)
            .setMethodCallHandler { call, result ->
                when (call.method) {
                    "inspect" -> runVoiceModelOperation(result) { voiceModels.inspect().toChannelValue() }
                    "install" -> runVoiceModelOperation(result) { voiceModels.install().toChannelValue() }
                    "delete" -> runVoiceModelOperation(result) { voiceModels.delete().toChannelValue() }
                    "toTraditional" -> toTraditional(call.argument<String>("text"), result)
                    else -> result.notImplemented()
                }
            }
        try {
            voiceCapture.recoverOrphans()
        } catch (error: Exception) {
            // Keep the app usable but leave the process-wide capture gate
            // closed until its retiring recorder actually exits.
            Log.e("TempestMikuVoice", "voice capture recovery is still pending", error)
        }
        handleNotificationIntent(intent)
        handleTextImportIntent(intent)
    }

    override fun onResume() {
        super.onResume()
        isActivityForeground = true
    }

    override fun onPause() {
        isActivityForeground = false
        cancelVoiceCaptureForLifecycle("pause")
        super.onPause()
    }

    override fun onDestroy() {
        isActivityForeground = false
        cancelVoiceCaptureForLifecycle("destroy")
        voicePermissionResult?.success(false)
        voicePermissionResult = null
        super.onDestroy()
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
        val granted = grantResults.firstOrNull() == PackageManager.PERMISSION_GRANTED
        when (requestCode) {
            REQUEST_NOTIFICATIONS -> {
                permissionResult?.success(granted)
                permissionResult = null
            }
            REQUEST_RECORD_AUDIO -> {
                voicePermissionResult?.success(granted)
                voicePermissionResult = null
            }
        }
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

    private fun requestVoicePermission(result: MethodChannel.Result) {
        if (!isActivityForeground) {
            result.error("not_foreground", "voice permission requires the foreground app", null)
            return
        }
        if (checkSelfPermission(Manifest.permission.RECORD_AUDIO) ==
            PackageManager.PERMISSION_GRANTED
        ) {
            result.success(true)
            return
        }
        if (voicePermissionResult != null) {
            result.error("permission_in_progress", "voice permission request is already active", null)
            return
        }
        voicePermissionResult = result
        requestPermissions(arrayOf(Manifest.permission.RECORD_AUDIO), REQUEST_RECORD_AUDIO)
    }

    private fun recoverVoiceCapture(result: MethodChannel.Result) {
        try {
            result.success(voiceCapture.recoverOrphans())
        } catch (error: Exception) {
            result.error("voice_recovery_failed", error.message ?: "voice recovery failed", null)
        }
    }

    private fun startVoiceCapture(captureId: String?, result: MethodChannel.Result) {
        if (!isActivityForeground) {
            result.error("not_foreground", "voice capture requires the foreground app", null)
            return
        }
        if (checkSelfPermission(Manifest.permission.RECORD_AUDIO) !=
            PackageManager.PERMISSION_GRANTED
        ) {
            result.error("permission_denied", "microphone permission is required", null)
            return
        }
        if (captureId == null) {
            result.error("invalid_capture", "captureId is required", null)
            return
        }
        try {
            voiceCapture.start(captureId)
            result.success(null)
        } catch (error: Exception) {
            result.error("voice_start_failed", error.message ?: "voice capture could not start", null)
        }
    }

    private fun stopVoiceCapture(captureId: String?, result: MethodChannel.Result) {
        if (!isActivityForeground) {
            voiceCapture.cancel(captureId)
            result.error("not_foreground", "voice capture left the foreground", null)
            return
        }
        if (captureId == null) {
            result.error("invalid_capture", "captureId is required", null)
            return
        }
        Thread(
            {
                try {
                    val completed = voiceCapture.stop(captureId)
                    runOnUiThread {
                        try {
                            // StandardMethodCodec encodes success synchronously; erase the
                            // native microphone copy immediately after it has been handed off.
                            result.success(
                                mapOf(
                                    "captureId" to completed.captureId,
                                    "sampleRate" to VOICE_SAMPLE_RATE,
                                    "pcm16" to completed.pcm16,
                                ),
                            )
                        } finally {
                            completed.pcm16.fill(0)
                        }
                    }
                } catch (error: Exception) {
                    voiceCapture.cancel()
                    runOnUiThread {
                        result.error(
                            "voice_stop_failed",
                            error.message ?: "voice capture could not stop",
                            null,
                        )
                    }
                }
            },
            "miku-voice-stop",
        ).start()
    }

    private fun inspectVoiceBuild(result: MethodChannel.Result) {
        Thread(
            {
                try {
                    val fingerprint =
                        APP_BUILD_FINGERPRINT_CACHE.inspect(
                            applicationId = BuildConfig.APPLICATION_ID,
                            versionName = BuildConfig.VERSION_NAME,
                            versionCode = BuildConfig.VERSION_CODE,
                            buildType = BuildConfig.BUILD_TYPE,
                            baseApk = File(applicationInfo.sourceDir),
                        )
                    runOnUiThread { result.success(fingerprint.toChannelValue()) }
                } catch (_: Exception) {
                    // Never couple package inspection to microphone capture, and
                    // never disclose the installed APK path through channel errors.
                    runOnUiThread {
                        result.error(
                            "build_fingerprint_failed",
                            "app build fingerprint is unavailable",
                            null,
                        )
                    }
                }
            },
            "miku-build-fingerprint",
        ).start()
    }

    private fun cancelVoiceCapture(captureId: String?, result: MethodChannel.Result) {
        try {
            result.success(voiceCapture.cancel(captureId))
        } catch (error: Exception) {
            result.error("voice_cancel_failed", error.message ?: "voice capture could not cancel", null)
        }
    }

    private fun cancelVoiceCaptureForLifecycle(reason: String) {
        try {
            voiceCapture.cancel()
        } catch (error: Exception) {
            // ForegroundVoiceCapture retains the retiring handle, blocks the
            // next start, and keeps monitoring it. Do not turn a failed join
            // into a lifecycle success or crash the rest of the companion UI.
            Log.e("TempestMikuVoice", "voice cleanup failed during $reason", error)
        }
    }

    private fun runVoiceModelOperation(
        result: MethodChannel.Result,
        operation: () -> Map<String, Any>,
    ) {
        synchronized(this) {
            if (voiceModelOperationActive) {
                result.error("model_operation_active", "a voice model operation is already active", null)
                return
            }
            voiceModelOperationActive = true
        }
        Thread(
            {
                try {
                    val value = operation()
                    runOnUiThread { result.success(value) }
                } catch (error: Exception) {
                    runOnUiThread {
                        result.error(
                            "voice_model_operation_failed",
                            error.message ?: "voice model operation failed",
                            null,
                        )
                    }
                } finally {
                    voiceModelOperationActive = false
                }
            },
            "miku-voice-model",
        ).start()
    }

    private fun toTraditional(text: String?, result: MethodChannel.Result) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) {
            result.error(
                "traditional_conversion_unsupported",
                "Traditional Chinese conversion requires Android 10 or newer",
                null,
            )
            return
        }
        if (text == null || text.length > 16_384) {
            result.error("invalid_transcript", "transcript was missing or oversized", null)
            return
        }
        try {
            result.success(Transliterator.getInstance("Simplified-Traditional").transliterate(text))
        } catch (error: Exception) {
            result.error(
                "traditional_conversion_failed",
                error.message ?: "Traditional Chinese conversion failed",
                null,
            )
        }
    }

    private fun handleNotificationIntent(intent: Intent?) {
        if (intent?.action == ACTION_OPEN_NOTIFICATION_ROUTE) {
            val route = NotificationIntentData.route(intent)
            val sessionId = route?.sessionId ?: intent.getStringExtra(EXTRA_SESSION_ID).orEmpty()
            val approvalId = route?.approvalId ?: intent.getStringExtra(EXTRA_APPROVAL_ID)
            val routeKind = route?.kind?.wireName ?: intent.getStringExtra(EXTRA_ROUTE_KIND).orEmpty()
            if (sessionId.isNotEmpty() && routeKind in setOf("session_ready", "approval_requested")) {
                emitNotificationAction(
                    mapOf(
                        "type" to "route",
                        "sessionId" to sessionId,
                        "routeKind" to routeKind,
                        if (approvalId != null) "approvalId" to approvalId else "approvalId" to "",
                    ),
                    "route:$routeKind:$sessionId:${approvalId.orEmpty()}",
                )
            }
            clearNotificationIntent(intent)
            return
        }
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
        emitNotificationAction(payload, "decision:$approvalId:$decision")
        clearNotificationIntent(intent)
    }

    private fun emitNotificationAction(payload: Map<String, Any>, dedupeKey: String) {
        val withKey = payload + ("dedupeKey" to dedupeKey)
        val sink = actionSink
        if (sink == null) {
            pendingActions.removeAll { queued -> queued["dedupeKey"] == dedupeKey }
            pendingActions.add(withKey)
        } else {
            sink.success(withKey)
        }
    }

    private fun clearNotificationIntent(intent: Intent) {
        intent.action = Intent.ACTION_MAIN
        for (extra in listOf(
            EXTRA_SESSION_ID,
            EXTRA_APPROVAL_ID,
            EXTRA_DECISION,
            EXTRA_ROUTE_VERSION,
            EXTRA_DELIVERY_ID,
            EXTRA_ROUTE_KIND,
            EXTRA_EVENT_SEQ,
            EXTRA_EXPIRES_AT,
        )) {
            intent.removeExtra(extra)
        }
    }

    private fun handleTextImportIntent(intent: Intent?) {
        val textImportIntent = intent ?: return
        if (
            textImportIntent.action !in
            setOf(Intent.ACTION_SEND, Intent.ACTION_PROCESS_TEXT, ACTION_QUICK_CAPTURE_V1)
        ) return
        val clipContainsUri = textImportIntent.clipData?.let { clip ->
            (0 until clip.itemCount).any { index -> clip.getItemAt(index).uri != null }
        } ?: false
        val hasUriPayload =
            textImportIntent.flags and URI_GRANT_FLAGS != 0 ||
                textImportIntent.hasExtra(Intent.EXTRA_STREAM) ||
                clipContainsUri
        val hasUnexpectedQuickCapturePayload =
            textImportIntent.action == ACTION_QUICK_CAPTURE_V1 &&
                (
                    textImportIntent.data != null ||
                        textImportIntent.clipData != null ||
                        textImportIntent.selector != null ||
                        textImportIntent.extras?.keySet()?.any { key ->
                            key !in setOf(EXTRA_QUICK_CAPTURE_ID, EXTRA_QUICK_CAPTURE_TEXT)
                        } == true
                )
        val parsed = TextImportIntentParser.parse(
            action = textImportIntent.action,
            mimeType = textImportIntent.type,
            sharedText = textImportIntent.getCharSequenceExtra(Intent.EXTRA_TEXT),
            selectedText = textImportIntent.getCharSequenceExtra(Intent.EXTRA_PROCESS_TEXT),
            quickCaptureText = textImportIntent.getCharSequenceExtra(EXTRA_QUICK_CAPTURE_TEXT),
            quickCaptureId = textImportIntent.getStringExtra(EXTRA_QUICK_CAPTURE_ID),
            subject = textImportIntent.getCharSequenceExtra(Intent.EXTRA_SUBJECT),
            hasDisallowedPayload = hasUriPayload || hasUnexpectedQuickCapturePayload,
        )
        if (parsed != null) {
            val payload = parsed.toEvent()
            val consumer = shareImportSink?.let { sink ->
                { event: Map<String, Any> -> sink.success(event) }
            }
            pendingShareImports.offer(payload, consumer)
        }
        textImportIntent.action = Intent.ACTION_MAIN
        textImportIntent.removeExtra(Intent.EXTRA_TEXT)
        textImportIntent.removeExtra(Intent.EXTRA_PROCESS_TEXT)
        textImportIntent.removeExtra(Intent.EXTRA_PROCESS_TEXT_READONLY)
        textImportIntent.removeExtra(Intent.EXTRA_SUBJECT)
        textImportIntent.removeExtra(Intent.EXTRA_STREAM)
        textImportIntent.removeExtra(EXTRA_QUICK_CAPTURE_ID)
        textImportIntent.removeExtra(EXTRA_QUICK_CAPTURE_TEXT)
        textImportIntent.data = null
        textImportIntent.clipData = null
        textImportIntent.selector = null
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
            this.action = ACTION_OPEN_NOTIFICATION_ROUTE
            flags = Intent.FLAG_ACTIVITY_CLEAR_TOP or Intent.FLAG_ACTIVITY_SINGLE_TOP
            putExtra(EXTRA_SESSION_ID, sessionId)
            putExtra(EXTRA_APPROVAL_ID, approvalId)
            putExtra(EXTRA_ROUTE_KIND, "approval_requested")
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
