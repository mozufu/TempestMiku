package dev.tempestmiku.miku_flutter

import android.content.Intent
import io.flutter.embedding.android.FlutterActivity
import io.flutter.embedding.engine.FlutterEngine
import io.flutter.plugin.common.MethodChannel

class MainActivity: FlutterActivity() {
    private val channelName = "dev.tempestmiku/pairing"
    private var pairingChannel: MethodChannel? = null

    override fun configureFlutterEngine(flutterEngine: FlutterEngine) {
        super.configureFlutterEngine(flutterEngine)
        pairingChannel = MethodChannel(flutterEngine.dartExecutor.binaryMessenger, channelName)
        pairingChannel?.setMethodCallHandler { call, result ->
            when (call.method) {
                "initialLink" -> result.success(pairingLinkFromIntent(intent))
                else -> result.notImplemented()
            }
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        setIntent(intent)
        pairingLinkFromIntent(intent)?.let { link ->
            pairingChannel?.invokeMethod("link", link)
        }
    }

    private fun pairingLinkFromIntent(intent: Intent?): String? {
        if (intent?.action != Intent.ACTION_VIEW) return null
        val data = intent.data ?: return null
        if (data.scheme != "tempestmiku" || data.host != "pair") return null
        return data.toString()
    }
}
