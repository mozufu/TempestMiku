package org.mozufu.tempestmiku

import java.nio.file.Files
import org.junit.Assert.assertEquals
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test

class AppBuildFingerprintTest {
    @Test
    fun `fingerprint exposes only exact app identity and lowercase APK SHA`() {
        val root = Files.createTempDirectory("miku-build-fingerprint-test").toFile()
        try {
            val apk = root.resolve("base.apk").apply { writeText("signed-apk-fixture") }
            val fingerprint =
                AppBuildFingerprintCache().inspect(
                    applicationId = "org.mozufu.tempestmiku",
                    versionName = "1.0.1",
                    versionCode = 2,
                    buildType = "release",
                    baseApk = apk,
                )

            assertEquals(
                "ade12cd72b92f20a396dceb889ba24f899806a0afd7904cb69ff815909ef2eb1",
                fingerprint.apkSha256,
            )
            assertTrue(Regex("^[0-9a-f]{64}$").matches(fingerprint.apkSha256))
            assertEquals(
                setOf("applicationId", "versionName", "versionCode", "buildType", "apkSha256"),
                fingerprint.toChannelValue().keys,
            )
        } finally {
            root.deleteRecursively()
        }
    }

    @Test
    fun `successful fingerprint is cached for the process`() {
        val root = Files.createTempDirectory("miku-build-fingerprint-cache-test").toFile()
        try {
            val apk = root.resolve("base.apk").apply { writeText("first") }
            val cache = AppBuildFingerprintCache()
            val first =
                cache.inspect(
                    applicationId = "org.mozufu.tempestmiku",
                    versionName = "1.0.1",
                    versionCode = 2,
                    buildType = "release",
                    baseApk = apk,
                )
            apk.writeText("changed-after-first-read")
            val second =
                cache.inspect(
                    applicationId = "ignored.after.cache",
                    versionName = "9.9.9",
                    versionCode = 99,
                    buildType = "debug",
                    baseApk = apk,
                )

            assertSame(first, second)
        } finally {
            root.deleteRecursively()
        }
    }
}
