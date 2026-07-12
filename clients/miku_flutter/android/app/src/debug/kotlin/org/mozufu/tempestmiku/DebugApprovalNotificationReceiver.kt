package org.mozufu.tempestmiku

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent

/** Debug-only ADB probe for the exact notification implementation used by Flutter. */
class DebugApprovalNotificationReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val approvalId = intent.getStringExtra(EXTRA_APPROVAL_ID) ?: "debug-approval-probe"
        if (intent.getBooleanExtra(EXTRA_CANCEL, false)) {
            ApprovalNotifications.cancel(context, approvalId)
        } else {
            ApprovalNotifications.show(
                context,
                intent.getStringExtra(EXTRA_SESSION_ID) ?: "debug-session-probe",
                approvalId,
                intent.getStringExtra(EXTRA_ACTION) ?: "Run debug approval probe",
            )
        }
    }

    companion object {
        const val ACTION = "org.mozufu.tempestmiku.DEBUG_APPROVAL_NOTIFICATION"
        const val EXTRA_CANCEL = "cancel"
        const val EXTRA_SESSION_ID = "sessionId"
        const val EXTRA_APPROVAL_ID = "approvalId"
        const val EXTRA_ACTION = "approvalAction"
    }
}
