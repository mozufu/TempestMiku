package dev.tempestmiku.miku_flutter

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent

/** Debug-only ADB probe for the exact notification implementation used by Flutter. */
class DebugApprovalNotificationReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val approvalId = "debug-approval-probe"
        if (intent.getBooleanExtra(EXTRA_CANCEL, false)) {
            ApprovalNotifications.cancel(context, approvalId)
        } else {
            ApprovalNotifications.show(context, approvalId)
        }
    }

    companion object {
        const val ACTION = "dev.tempestmiku.miku_flutter.DEBUG_APPROVAL_NOTIFICATION"
        const val EXTRA_CANCEL = "cancel"
    }
}
