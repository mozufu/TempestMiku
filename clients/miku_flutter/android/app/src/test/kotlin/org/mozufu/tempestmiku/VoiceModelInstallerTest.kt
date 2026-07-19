package org.mozufu.tempestmiku

import java.io.ByteArrayInputStream
import java.io.File
import java.io.InputStream
import java.net.URI
import java.nio.file.Files
import java.security.MessageDigest
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicReference
import kotlin.concurrent.thread
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class VoiceModelInstallerTest {
    @Test
    fun `pinned contract carries reviewed provenance and stays below total cap`() {
        val manifest = VoiceModelContract.manifestText()
        assertTrue(manifest.contains("\"commit\":\"$VOICE_MODEL_COMMIT\""))
        assertTrue(manifest.contains("\"license\":\"Apache-2.0\""))
        assertTrue(manifest.contains("https://www.apache.org/licenses/LICENSE-2.0"))
        assertTrue(manifest.contains("\"runtime\":\"$VOICE_MODEL_RUNTIME\""))
        assertTrue(manifest.contains("\"total_bytes\":237202501"))
        assertEquals(237_202_501L, VoiceModelContract.totalBytes)
        assertTrue(VoiceModelContract.totalBytes < VOICE_MODEL_MAX_BYTES)
        assertEquals(3, VoiceModelContract.files.size)
        VoiceModelContract.files.forEach { file ->
            assertEquals("https", file.source.scheme)
            assertEquals("huggingface.co", file.source.host)
            assertTrue(file.source.path.contains("/$VOICE_MODEL_COMMIT/"))
            assertEquals(null, file.source.query)
        }
    }

    @Test
    fun `streaming download accepts exact bytes and rejects hash mismatch and oversize`() =
        withRoot { root ->
            val installer = VoiceModelInstaller(root, sdkInt = 35)
            val staging = File(root, ".staging-test").apply { mkdirs() }
            val bytes = "traditional-local".toByteArray()
            val expected =
                VoiceModelFile(
                    name = "tokens.txt",
                    size = bytes.size.toLong(),
                    sha256 = bytes.sha256(),
                )
            assertEquals(
                bytes.size.toLong(),
                installer.downloadForTest(expected, staging, queue(response(200, bytes))),
            )
            assertTrue(File(staging, "tokens.txt").readBytes().contentEquals(bytes))

            File(staging, "tokens.txt").delete()
            assertFails {
                installer.downloadForTest(
                    expected.copy(sha256 = "00".repeat(32)),
                    staging,
                    queue(response(200, bytes)),
                )
            }
            assertFails {
                installer.downloadForTest(
                    expected.copy(size = 1, sha256 = byteArrayOf(bytes.first()).sha256()),
                    staging,
                    queue(response(200, bytes, contentLength = null)),
                )
            }
        }

    @Test
    fun `redirect policy rejects evil host and any second redirect`() =
        withRoot { root ->
            val installer = VoiceModelInstaller(root, sdkInt = 35)
            val staging = File(root, ".staging-test").apply { mkdirs() }
            val expected = VoiceModelFile("tokens.txt", 1, byteArrayOf(1).sha256())
            assertFails {
                installer.downloadForTest(
                    expected,
                    staging,
                    queue(response(302, location = "https://evil.example/model")),
                )
            }
            assertFails {
                installer.downloadForTest(
                    expected,
                    staging,
                    queue(
                        response(307, location = "/same-origin"),
                        response(302, location = "https://us.aws.cdn.hf.co/object"),
                    ),
                )
            }
            val accepted =
                installer.validateRedirect(
                    expected.source,
                    "https://us.aws.cdn.hf.co/signed-object?provider_owned=1",
                )
            assertEquals("us.aws.cdn.hf.co", accepted.host)
            assertFails {
                installer.validateRedirect(
                    expected.source,
                    "https://user@us.aws.cdn.hf.co/object",
                )
            }
        }

    @Test
    fun `verified install atomically activates one version and is idempotent`() =
        withRoot { root ->
            val payloads =
                linkedMapOf(
                    "encoder.int8.onnx" to "enc".toByteArray(),
                    "decoder.int8.onnx" to "decoder".toByteArray(),
                    "tokens.txt" to "tokens".toByteArray(),
                )
            val files =
                payloads.map { (name, bytes) ->
                    VoiceModelFile(name, bytes.size.toLong(), bytes.sha256())
                }
            val spec =
                VoiceModelInstallSpec(
                    files = files,
                    totalBytes = files.sumOf { it.size },
                    versionDirectory = "test-version",
                    manifestText = "{\"schema\":\"test\",\"commit\":\"$VOICE_MODEL_COMMIT\"}",
                )
            val installer = VoiceModelInstaller(root, sdkInt = 35, spec = spec)
            val installed =
                installer.install(
                    queue(*payloads.values.map { response(200, it) }.toTypedArray()),
                )
            assertTrue(installed.ready)
            val activated = File(root, spec.versionDirectory)
            assertTrue(activated.isDirectory)
            assertEquals(
                payloads.keys.toSet() + VoiceModelContract.manifestName,
                activated.list()?.toSet(),
            )
            assertFalse(root.listFiles().orEmpty().any { it.name.startsWith(".staging-") })

            val stillReady =
                installer.install(
                    VoiceModelHttpClient { error("ready install must not access the network") },
                )
            assertTrue(stillReady.ready)
        }

    @Test
    fun `installer operations serialize across activity recreation instances`() =
        withRoot { root ->
            val bytes = "one-process-model".toByteArray()
            val file = VoiceModelFile("tokens.txt", bytes.size.toLong(), bytes.sha256())
            val spec =
                VoiceModelInstallSpec(
                    files = listOf(file),
                    totalBytes = file.size,
                    versionDirectory = "cross-instance-version",
                    manifestText = "{\"schema\":\"cross-instance\"}",
                )
            val first = VoiceModelInstaller(root, sdkInt = 35, spec = spec)
            val recreated = VoiceModelInstaller(root, sdkInt = 35, spec = spec)
            val downloadOpened = CountDownLatch(1)
            val releaseDownload = CountDownLatch(1)
            val installStatus = AtomicReference<VoiceModelStatus?>()
            val inspectStatus = AtomicReference<VoiceModelStatus?>()
            val installFailure = AtomicReference<Throwable?>()
            val inspectFailure = AtomicReference<Throwable?>()

            val installThread =
                thread(name = "voice-model-install-test") {
                    try {
                        installStatus.set(
                            first.install(
                                VoiceModelHttpClient {
                                    downloadOpened.countDown()
                                    check(releaseDownload.await(5, TimeUnit.SECONDS)) {
                                        "timed out waiting to release test download"
                                    }
                                    response(200, bytes)
                                },
                            ),
                        )
                    } catch (error: Throwable) {
                        installFailure.set(error)
                    }
                }
            assertTrue(downloadOpened.await(5, TimeUnit.SECONDS))
            assertTrue(root.listFiles().orEmpty().any { it.name.startsWith(".staging-") })

            val inspectThread =
                thread(name = "voice-model-recreated-inspect-test") {
                    try {
                        inspectStatus.set(recreated.inspect())
                    } catch (error: Throwable) {
                        inspectFailure.set(error)
                    }
                }
            val blockDeadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(2)
            while (
                inspectThread.state != Thread.State.BLOCKED &&
                    inspectThread.state != Thread.State.TERMINATED &&
                    System.nanoTime() < blockDeadline
            ) {
                Thread.yield()
            }
            assertEquals(Thread.State.BLOCKED, inspectThread.state)
            assertTrue(root.listFiles().orEmpty().any { it.name.startsWith(".staging-") })

            releaseDownload.countDown()
            installThread.join(5_000)
            inspectThread.join(5_000)
            assertFalse(installThread.isAlive)
            assertFalse(inspectThread.isAlive)
            installFailure.get()?.let { throw AssertionError("install failed", it) }
            inspectFailure.get()?.let { throw AssertionError("inspect failed", it) }
            assertTrue(installStatus.get()?.ready == true)
            assertTrue(inspectStatus.get()?.ready == true)
        }

    @Test
    fun `missing corrupt delete and cold start staging recovery fail closed`() =
        withRoot { root ->
            val installer = VoiceModelInstaller(root, sdkInt = 35)
            val orphan = File(root, ".staging-orphan").apply { mkdirs() }
            File(orphan, "partial.onnx").writeText("partial")
            assertEquals("missing", installer.inspect().state)
            assertFalse(orphan.exists())

            val activated = File(root, VoiceModelContract.versionDirectory).apply { mkdirs() }
            File(activated, "unexpected").writeText("corrupt")
            assertEquals("corrupt", installer.inspect().state)
            assertFalse(installer.inspect().ready)

            assertEquals("missing", installer.delete().state)
            assertFalse(activated.exists())
        }

    @Test
    fun `Android versions without platform Traditional conversion stay disabled`() =
        withRoot { root ->
            val status = VoiceModelInstaller(root, sdkInt = 28).inspect()
            assertEquals("unsupported", status.state)
            assertFalse(status.ready)
        }

    private fun response(
        status: Int,
        bytes: ByteArray = byteArrayOf(),
        location: String? = null,
        contentLength: Long? = bytes.size.toLong(),
    ): VoiceModelResponse =
        object : VoiceModelResponse {
            override val statusCode = status
            override val location = location
            override val contentLength = contentLength
            override val body: InputStream = ByteArrayInputStream(bytes)
            override fun close() = Unit
        }

    private fun queue(vararg responses: VoiceModelResponse): VoiceModelHttpClient {
        val pending = ArrayDeque(responses.toList())
        return VoiceModelHttpClient { _: URI ->
            check(pending.isNotEmpty()) { "unexpected HTTP request" }
            pending.removeFirst()
        }
    }

    private fun ByteArray.sha256(): String =
        MessageDigest.getInstance("SHA-256")
            .digest(this)
            .joinToString(separator = "") { "%02x".format(it) }

    private fun withRoot(block: (File) -> Unit) {
        val root = Files.createTempDirectory("miku-model-test").toFile()
        try {
            block(root)
        } finally {
            root.deleteRecursively()
        }
    }

    private fun assertFails(block: () -> Unit) {
        var failed = false
        try {
            block()
        } catch (_: IllegalArgumentException) {
            failed = true
        } catch (_: IllegalStateException) {
            failed = true
        }
        assertTrue("expected the operation to fail", failed)
    }
}
