package org.mozufu.tempestmiku.asrbenchmark

import android.content.Context
import android.icu.text.Transliterator
import android.os.Build
import android.os.PowerManager
import io.flutter.embedding.android.FlutterActivity
import io.flutter.embedding.engine.FlutterEngine
import io.flutter.plugin.common.MethodChannel
import java.io.FileInputStream
import java.security.MessageDigest
import java.util.UUID

class MainActivity : FlutterActivity() {
    override fun configureFlutterEngine(flutterEngine: FlutterEngine) {
        super.configureFlutterEngine(flutterEngine)
        MethodChannel(
            flutterEngine.dartExecutor.binaryMessenger,
            "org.mozufu.tempestmiku.asrbenchmark/device",
        ).setMethodCallHandler { call, result ->
            when (call.method) {
                "getDeviceInfo" ->
                    result.success(
                        mapOf(
                            "manufacturer" to Build.MANUFACTURER,
                            "model" to Build.MODEL,
                            "device" to Build.DEVICE,
                            "sdk" to Build.VERSION.SDK_INT,
                            "fingerprint" to Build.FINGERPRINT,
                            "buildId" to Build.ID,
                            "securityPatch" to Build.VERSION.SECURITY_PATCH,
                            "hardware" to Build.HARDWARE,
                            "board" to Build.BOARD,
                            "product" to Build.PRODUCT,
                            "supportedAbis" to Build.SUPPORTED_ABIS.toList(),
                            "physicalDevice" to isPhysicalDevice(),
                            "benchmarkInstallationId" to benchmarkInstallationId(),
                            "benchmarkApkSha256" to benchmarkApkSha256(),
                        ),
                    )
                "getThermalStatus" -> result.success(thermalStatus())
                "toTraditional" -> toTraditional(call.argument<String>("text"), result)
                else -> result.notImplemented()
            }
        }
    }

    private fun isPhysicalDevice(): Boolean {
        val fingerprint = Build.FINGERPRINT.lowercase()
        val model = Build.MODEL.lowercase()
        val hardware = Build.HARDWARE.lowercase()
        val product = Build.PRODUCT.lowercase()
        return !fingerprint.startsWith("generic") &&
            !fingerprint.contains("emulator") &&
            !model.contains("emulator") &&
            !model.contains("android sdk built for") &&
            hardware !in setOf("goldfish", "ranchu", "vbox86") &&
            !product.contains("sdk_gphone")
    }

    private fun benchmarkInstallationId(): String {
        val preferences =
            getSharedPreferences("benchmark_identity", Context.MODE_PRIVATE)
        val existing = preferences.getString("installation_id", null)
        if (existing != null) {
            val parsed = runCatching { UUID.fromString(existing) }.getOrNull()
            if (parsed != null && parsed.version() == 4 && parsed.variant() == 2) {
                return existing
            }
        }
        val generated = UUID.randomUUID().toString()
        check(preferences.edit().putString("installation_id", generated).commit()) {
            "failed to persist benchmark installation identity"
        }
        return generated
    }

    private fun benchmarkApkSha256(): String {
        val digest = MessageDigest.getInstance("SHA-256")
        FileInputStream(applicationInfo.sourceDir).use { input ->
            val buffer = ByteArray(DEFAULT_BUFFER_SIZE)
            while (true) {
                val read = input.read(buffer)
                if (read < 0) break
                digest.update(buffer, 0, read)
            }
        }
        return digest.digest().joinToString("") { byte ->
            "%02x".format(byte.toInt() and 0xff)
        }
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
            result.success(
                Transliterator.getInstance("Simplified-Traditional").transliterate(text),
            )
        } catch (error: Exception) {
            result.error(
                "traditional_conversion_failed",
                error.message ?: "Traditional Chinese conversion failed",
                null,
            )
        }
    }

    private fun thermalStatus(): Map<String, Any> {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) {
            return mapOf("available" to false)
        }
        val status =
            (getSystemService(Context.POWER_SERVICE) as PowerManager).currentThermalStatus
        val label =
            when (status) {
                PowerManager.THERMAL_STATUS_NONE -> "none"
                PowerManager.THERMAL_STATUS_LIGHT -> "light"
                PowerManager.THERMAL_STATUS_MODERATE -> "moderate"
                PowerManager.THERMAL_STATUS_SEVERE -> "severe"
                PowerManager.THERMAL_STATUS_CRITICAL -> "critical"
                PowerManager.THERMAL_STATUS_EMERGENCY -> "emergency"
                PowerManager.THERMAL_STATUS_SHUTDOWN -> "shutdown"
                else -> "unknown"
            }
        return mapOf("available" to true, "status" to status, "label" to label)
    }
}
