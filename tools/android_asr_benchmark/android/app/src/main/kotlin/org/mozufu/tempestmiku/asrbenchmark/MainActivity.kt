package org.mozufu.tempestmiku.asrbenchmark

import android.os.Build
import io.flutter.embedding.android.FlutterActivity
import io.flutter.embedding.engine.FlutterEngine
import io.flutter.plugin.common.MethodChannel

class MainActivity : FlutterActivity() {
    override fun configureFlutterEngine(flutterEngine: FlutterEngine) {
        super.configureFlutterEngine(flutterEngine)
        MethodChannel(
            flutterEngine.dartExecutor.binaryMessenger,
            "org.mozufu.tempestmiku.asrbenchmark/device",
        ).setMethodCallHandler { call, result ->
            if (call.method != "getDeviceInfo") {
                result.notImplemented()
                return@setMethodCallHandler
            }
            result.success(
                mapOf(
                    "manufacturer" to Build.MANUFACTURER,
                    "model" to Build.MODEL,
                    "device" to Build.DEVICE,
                    "sdk" to Build.VERSION.SDK_INT,
                ),
            )
        }
    }
}
