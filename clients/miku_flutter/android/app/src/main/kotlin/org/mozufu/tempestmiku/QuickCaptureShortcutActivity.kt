package org.mozufu.tempestmiku

import android.app.Activity
import android.os.Bundle

/** Avoids the task-clearing flags Android applies directly to static shortcuts. */
class QuickCaptureShortcutActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        if (intent?.action == ACTION_QUICK_CAPTURE_V1) {
            startActivity(QuickCaptureIntents.create(this))
        }
        finish()
    }
}
