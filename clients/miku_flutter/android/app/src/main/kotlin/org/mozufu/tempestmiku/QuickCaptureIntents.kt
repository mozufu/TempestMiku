package org.mozufu.tempestmiku

import android.content.Context
import android.content.Intent
import java.util.UUID

internal object QuickCaptureIntents {
    fun create(context: Context, text: CharSequence? = null): Intent =
        Intent(context, MainActivity::class.java).apply {
            action = ACTION_QUICK_CAPTURE_V1
            addFlags(
                Intent.FLAG_ACTIVITY_NEW_TASK or
                    Intent.FLAG_ACTIVITY_CLEAR_TOP or
                    Intent.FLAG_ACTIVITY_SINGLE_TOP,
            )
            putExtra(EXTRA_QUICK_CAPTURE_ID, UUID.randomUUID().toString())
            text?.let { putExtra(EXTRA_QUICK_CAPTURE_TEXT, it) }
        }
}
