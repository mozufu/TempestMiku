package dev.tempestmiku.miku_flutter

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

class MainActivity : FlutterActivity() {
    companion object {
        private const val CHANNEL = "dev.tempestmiku.miku_flutter/notifications"
        private const val REQUEST_NOTIFICATIONS = 701
    }

    private var permissionResult: MethodChannel.Result? = null

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
                    val approvalId = call.argument<String>("approvalId")
                    if (approvalId.isNullOrBlank()) {
                        result.error("invalid_approval", "approvalId is required", null)
                    } else {
                        ApprovalNotifications.show(this, approvalId)
                        result.success(null)
                    }
                }
                "cancelApproval" -> {
                    call.argument<String>("approvalId")?.let { approvalId ->
                        ApprovalNotifications.cancel(this, approvalId)
                    }
                    result.success(null)
                }
                else -> result.notImplemented()
            }
        }
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

    fun show(context: Context, approvalId: String) {
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
        val builder = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            Notification.Builder(context, APPROVAL_CHANNEL_ID)
        } else {
            Notification.Builder(context)
        }
        val notification = builder
            .setSmallIcon(R.mipmap.ic_launcher)
            .setContentTitle("TempestMiku needs your approval")
            .setContentText("Open the app to review a pending request.")
            .setCategory(Notification.CATEGORY_STATUS)
            .setAutoCancel(true)
            .setOnlyAlertOnce(true)
            .setContentIntent(contentIntent)
            .build()
        notificationManager(context).notify(notificationId(approvalId), notification)
    }

    fun cancel(context: Context, approvalId: String) {
        notificationManager(context).cancel(notificationId(approvalId))
    }

    private fun notificationManager(context: Context): NotificationManager =
        context.getSystemService(NotificationManager::class.java)

    private fun notificationId(approvalId: String): Int = approvalId.hashCode()
}
