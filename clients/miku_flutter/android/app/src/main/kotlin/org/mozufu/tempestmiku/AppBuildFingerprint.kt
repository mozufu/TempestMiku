package org.mozufu.tempestmiku

import java.io.File
import java.security.MessageDigest

private val APPLICATION_ID_PATTERN =
    Regex("^[a-z][a-z0-9_]*(?:\\.[a-z][a-z0-9_]*)+$")
private val VERSION_NAME_PATTERN = Regex("^[A-Za-z0-9][A-Za-z0-9._+\\-]{0,127}$")
private val BUILD_TYPE_PATTERN = Regex("^[a-z][a-z0-9_-]{0,31}$")
private val SHA256_PATTERN = Regex("^[0-9a-f]{64}$")

internal data class AppBuildFingerprint(
    val applicationId: String,
    val versionName: String,
    val versionCode: Int,
    val buildType: String,
    val apkSha256: String,
) {
    init {
        require(applicationId.length <= 255 && APPLICATION_ID_PATTERN.matches(applicationId)) {
            "invalid application id"
        }
        require(VERSION_NAME_PATTERN.matches(versionName)) { "invalid version name" }
        require(versionCode in 1..2_100_000_000) { "invalid version code" }
        require(BUILD_TYPE_PATTERN.matches(buildType)) { "invalid build type" }
        require(SHA256_PATTERN.matches(apkSha256)) { "invalid APK SHA-256" }
    }

    fun toChannelValue(): Map<String, Any> =
        linkedMapOf(
            "applicationId" to applicationId,
            "versionName" to versionName,
            "versionCode" to versionCode,
            "buildType" to buildType,
            "apkSha256" to apkSha256,
        )
}

/** Caches the exact installed package identity after the first successful read. */
internal class AppBuildFingerprintCache {
    @Volatile private var cached: AppBuildFingerprint? = null
    private val lock = Any()

    fun inspect(
        applicationId: String,
        versionName: String,
        versionCode: Int,
        buildType: String,
        baseApk: File,
    ): AppBuildFingerprint {
        cached?.let { return it }
        return synchronized(lock) {
            cached?.let { return@synchronized it }
            require(baseApk.isFile) { "installed base APK is unavailable" }
            AppBuildFingerprint(
                applicationId = applicationId,
                versionName = versionName,
                versionCode = versionCode,
                buildType = buildType,
                apkSha256 = baseApk.sha256(),
            ).also { cached = it }
        }
    }
}

private fun File.sha256(): String {
    val digest = MessageDigest.getInstance("SHA-256")
    inputStream().buffered().use { input ->
        val buffer = ByteArray(DEFAULT_BUFFER_SIZE)
        while (true) {
            val count = input.read(buffer)
            if (count < 0) break
            if (count > 0) digest.update(buffer, 0, count)
        }
    }
    return digest.digest().joinToString("") { byte -> "%02x".format(byte.toInt() and 0xff) }
}
