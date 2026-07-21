package org.mozufu.tempestmiku

import android.os.Build
import java.io.BufferedInputStream
import java.io.Closeable
import java.io.File
import java.io.FileOutputStream
import java.io.InputStream
import java.net.HttpURLConnection
import java.net.URI
import java.net.URL
import java.security.MessageDigest
import java.util.UUID

internal const val VOICE_MODEL_COMMIT = "2a7f71bb58885c1b522ed4e683abd397355d9fc4"
internal const val VOICE_MODEL_RUNTIME = "sherpa_onnx 1.13.4"
internal const val VOICE_MODEL_MAX_BYTES = 350L * 1024L * 1024L

internal data class VoiceModelFile(
    val name: String,
    val size: Long,
    val sha256: String,
) {
    val source: URI
        get() =
            URI(
                "https://huggingface.co/csukuangfj/" +
                    "sherpa-onnx-streaming-paraformer-zh/resolve/" +
                    "$VOICE_MODEL_COMMIT/$name",
            )
}

internal data class VoiceModelInstallSpec(
    val files: List<VoiceModelFile>,
    val totalBytes: Long,
    val versionDirectory: String,
    val manifestText: String,
)

internal object VoiceModelContract {
    const val modelId = "csukuangfj/sherpa-onnx-streaming-paraformer-zh@$VOICE_MODEL_COMMIT"
    const val repository =
        "https://huggingface.co/csukuangfj/sherpa-onnx-streaming-paraformer-zh"
    const val license = "Apache-2.0"
    const val licenseUrl =
        "https://www.apache.org/licenses/LICENSE-2.0"
    const val attribution =
        "sherpa-onnx streaming Paraformer Chinese model by csukuangfj; Apache License 2.0"
    const val versionDirectory = "streaming-paraformer-zh-$VOICE_MODEL_COMMIT"
    const val manifestName = "manifest.json"

    val files =
        listOf(
            VoiceModelFile(
                "encoder.int8.onnx",
                165_462_184,
                "81a70226a8934e6ed92aa1d4fc486b428b5398e2f2619ed4897b7294cab90e9a",
            ),
            VoiceModelFile(
                "decoder.int8.onnx",
                71_664_561,
                "f3cca9f77bb9d93c8fcbfb63ae617b6b1ee96818df3aa3b151c40658fe38594f",
            ),
            VoiceModelFile(
                "tokens.txt",
                75_756,
                "59aba8873a2ed1e122c25fee421e25f283b63290efbde85c1f01a853d83cb6e6",
            ),
        )

    val totalBytes: Long = files.sumOf { it.size }

    init {
        check(totalBytes == 237_202_501L)
        check(totalBytes < VOICE_MODEL_MAX_BYTES)
        check(files.map { it.name }.toSet().size == files.size)
    }

    fun manifestText(): String =
        buildString {
            append("{\"schema\":\"tempestmiku.voice-model.v1\"")
            append(",\"model_id\":\"").append(modelId).append('"')
            append(",\"origin\":\"").append(repository).append('"')
            append(",\"commit\":\"").append(VOICE_MODEL_COMMIT).append('"')
            append(",\"license\":\"").append(license).append('"')
            append(",\"license_url\":\"").append(licenseUrl).append('"')
            append(",\"attribution\":\"").append(attribution).append('"')
            append(",\"runtime\":\"").append(VOICE_MODEL_RUNTIME).append('"')
            append(",\"total_bytes\":").append(totalBytes)
            append(",\"files\":[")
            files.forEachIndexed { index, file ->
                if (index > 0) append(',')
                append("{\"name\":\"").append(file.name).append('"')
                append(",\"source\":\"").append(file.source).append('"')
                append(",\"size\":").append(file.size)
                append(",\"sha256\":\"").append(file.sha256).append("\"}")
            }
            append("]}")
        }

    val installSpec: VoiceModelInstallSpec
        get() =
            VoiceModelInstallSpec(
                files = files,
                totalBytes = totalBytes,
                versionDirectory = versionDirectory,
                manifestText = manifestText(),
            )
}

internal data class VoiceModelStatus(
    val state: String,
    val reason: String,
    val modelDirectory: File? = null,
) {
    val ready: Boolean get() = state == "ready" && modelDirectory != null

    fun toChannelValue(): Map<String, Any> =
        buildMap {
            put("state", state)
            put("reason", reason)
            put("modelId", VoiceModelContract.modelId)
            put("totalBytes", VoiceModelContract.totalBytes)
            if (ready) {
                put("encoder", File(modelDirectory, "encoder.int8.onnx").absolutePath)
                put("decoder", File(modelDirectory, "decoder.int8.onnx").absolutePath)
                put("tokens", File(modelDirectory, "tokens.txt").absolutePath)
            }
        }
}

internal interface VoiceModelResponse : Closeable {
    val statusCode: Int
    val location: String?
    val contentLength: Long?
    val body: InputStream
}

internal fun interface VoiceModelHttpClient {
    fun open(uri: URI): VoiceModelResponse
}

internal class UrlConnectionVoiceModelClient : VoiceModelHttpClient {
    override fun open(uri: URI): VoiceModelResponse {
        val connection = uri.toURL().openConnection() as HttpURLConnection
        connection.instanceFollowRedirects = false
        connection.useCaches = false
        connection.connectTimeout = 20_000
        connection.readTimeout = 45_000
        connection.requestMethod = "GET"
        connection.setRequestProperty("Accept", "application/octet-stream")
        connection.setRequestProperty("User-Agent", "TempestMiku-Android/voice-model-v1")
        connection.connect()
        return object : VoiceModelResponse {
            override val statusCode: Int = connection.responseCode
            override val location: String? = connection.getHeaderField("Location")
            override val contentLength: Long? =
                connection.getHeaderField("Content-Length")?.toLongOrNull()
            override val body: InputStream
                get() = connection.inputStream

            override fun close() = connection.disconnect()
        }
    }
}

/**
 * Explicit, app-private installer for the one reviewed production ASR model.
 *
 * The two large Hugging Face LFS objects require one provider-controlled CDN
 * redirect. The initial URL is immutable and commit-pinned; that one redirect
 * is restricted to the exact observed official LFS host. File size and SHA-256
 * remain the trust root. No caller can supply a URL, filename, or query.
 */
internal class VoiceModelInstaller(
    private val root: File,
    private val sdkInt: Int = Build.VERSION.SDK_INT,
    private val spec: VoiceModelInstallSpec = VoiceModelContract.installSpec,
) {
    companion object {
        // Activity recreation creates a new installer instance while an old
        // download thread can still be alive. Keep staging inspection,
        // installation, and deletion mutually exclusive for the whole app
        // process so a new instance cannot purge another instance's live work.
        private val processOperationLock = Any()
    }

    private val versionDirectory = File(root, spec.versionDirectory)

    fun inspect(): VoiceModelStatus = synchronized(processOperationLock) {
        if (sdkInt < Build.VERSION_CODES.Q) {
            return@synchronized VoiceModelStatus(
                "unsupported",
                "on-device Traditional Chinese conversion requires Android 10 or newer",
            )
        }
        root.mkdirs()
        cleanupStaging()
        if (!versionDirectory.exists()) {
            return@synchronized VoiceModelStatus("missing", "not installed")
        }
        try {
            verifyDirectory(versionDirectory)
            VoiceModelStatus("ready", "verified", versionDirectory)
        } catch (error: Exception) {
            VoiceModelStatus("corrupt", error.message ?: "verification failed")
        }
    }

    fun install(
        client: VoiceModelHttpClient = UrlConnectionVoiceModelClient(),
        progress: ((Long, Long) -> Unit)? = null,
        cancelled: () -> Boolean = { false },
    ): VoiceModelStatus = synchronized(processOperationLock) {
        val existing = inspect()
        if (existing.state == "unsupported" || existing.ready) return@synchronized existing
        if (root.usableSpace < spec.totalBytes + (32L * 1024L * 1024L)) {
            throw IllegalStateException("not enough app-private storage for the voice model")
        }
        if (versionDirectory.exists()) deleteTree(versionDirectory)
        val staging = File(root, ".staging-${UUID.randomUUID()}")
        check(staging.mkdir()) { "could not create voice model staging directory" }
        try {
            var total = 0L
            spec.files.forEach { expected ->
                total += download(expected, staging, client, total, progress, cancelled)
                check(total <= VOICE_MODEL_MAX_BYTES) { "voice model total exceeded the safety cap" }
            }
            check(total == spec.totalBytes) { "voice model total size did not match" }
            writeSynced(
                File(staging, VoiceModelContract.manifestName),
                spec.manifestText.toByteArray(Charsets.UTF_8),
            )
            verifyDirectory(staging)
            check(staging.renameTo(versionDirectory)) {
                "could not atomically activate the verified voice model"
            }
            inspect()
        } catch (error: Exception) {
            deleteTree(staging)
            throw error
        }
    }

    fun delete(): VoiceModelStatus = synchronized(processOperationLock) {
        cleanupStaging()
        deleteTree(versionDirectory)
        inspect()
    }

    private fun download(
        expected: VoiceModelFile,
        staging: File,
        client: VoiceModelHttpClient,
        previouslyReceived: Long = 0L,
        progress: ((Long, Long) -> Unit)? = null,
        cancelled: () -> Boolean = { false },
    ): Long {
        validateInitialSource(expected)
        var uri = expected.source
        var redirects = 0
        var lastReported = previouslyReceived
        while (true) {
            client.open(uri).use { response ->
                if (response.statusCode in setOf(301, 302, 303, 307, 308)) {
                    check(redirects == 0) { "voice model download redirected more than once" }
                    uri = validateRedirect(uri, response.location)
                    redirects += 1
                    return@use
                }
                check(response.statusCode == HttpURLConnection.HTTP_OK) {
                    "voice model download returned HTTP ${response.statusCode}"
                }
                response.contentLength?.let { length ->
                    check(length == expected.size) {
                        "${expected.name} Content-Length did not match the manifest"
                    }
                }
                val target = File(staging, expected.name)
                val digest = MessageDigest.getInstance("SHA-256")
                var count = 0L
                FileOutputStream(target).use { output ->
                    BufferedInputStream(response.body).use { input ->
                        val buffer = ByteArray(64 * 1024)
                        while (true) {
                            if (cancelled()) {
                                throw InterruptedException("voice model install cancelled")
                            }
                            val read = input.read(buffer)
                            if (read < 0) break
                            count += read
                            check(count <= expected.size && count <= VOICE_MODEL_MAX_BYTES) {
                                "${expected.name} exceeded its safety cap"
                            }
                            digest.update(buffer, 0, read)
                            output.write(buffer, 0, read)
                            val cumulative = previouslyReceived + count
                            if (cumulative - lastReported >= 512L * 1024L) {
                                lastReported = cumulative
                                progress?.invoke(cumulative, spec.totalBytes)
                            }
                        }
                    }
                    output.fd.sync()
                }
                check(count == expected.size) { "${expected.name} size did not match the manifest" }
                check(digest.digest().toHex() == expected.sha256) {
                    "${expected.name} SHA-256 did not match the manifest"
                }
                progress?.invoke(previouslyReceived + count, spec.totalBytes)
                return count
            }
        }
    }

    internal fun downloadForTest(
        expected: VoiceModelFile,
        staging: File,
        client: VoiceModelHttpClient,
    ): Long = download(expected, staging, client)

    private fun validateInitialSource(expected: VoiceModelFile) {
        val uri = expected.source
        check(uri.scheme == "https" && uri.host == "huggingface.co" && uri.port == -1)
        check(uri.userInfo == null && uri.query == null && uri.fragment == null)
        val exactPrefix =
            "/csukuangfj/sherpa-onnx-streaming-paraformer-zh/resolve/$VOICE_MODEL_COMMIT/"
        check(uri.path == exactPrefix + expected.name) { "voice model source was not exact" }
    }

    internal fun validateRedirect(from: URI, location: String?): URI {
        check(!location.isNullOrBlank()) { "voice model redirect omitted Location" }
        val target = from.resolve(location)
        check(target.scheme == "https" && target.port == -1 && target.userInfo == null) {
            "voice model redirect was not safe HTTPS"
        }
        check(target.fragment == null)
        val sameOrigin = target.host == "huggingface.co" && from.host == "huggingface.co"
        val exactLfsOrigin = target.host == "us.aws.cdn.hf.co" && from.host == "huggingface.co"
        check(sameOrigin || exactLfsOrigin) { "voice model redirect left the pinned origin allowlist" }
        return target
    }

    private fun verifyDirectory(directory: File) {
        check(directory.canonicalFile.parentFile == root.canonicalFile) {
            "voice model directory escaped app-private storage"
        }
        val expectedNames =
            spec.files.mapTo(mutableSetOf()) { it.name }.apply {
                add(VoiceModelContract.manifestName)
            }
        check(directory.list()?.toSet() == expectedNames) { "voice model directory had unexpected files" }
        val manifestFile = File(directory, VoiceModelContract.manifestName)
        check(manifestFile.length() in 1..65_536) { "voice model manifest was missing or oversized" }
        check(manifestFile.readText(Charsets.UTF_8) == spec.manifestText) {
            "voice model manifest metadata did not match the pinned contract"
        }
        spec.files.forEach { expected ->
            val file = File(directory, expected.name)
            check(file.isFile && file.canonicalFile.parentFile == directory.canonicalFile) {
                "${expected.name} was missing or escaped the model directory"
            }
            check(file.length() == expected.size) { "${expected.name} size did not match" }
            check(file.sha256() == expected.sha256) { "${expected.name} SHA-256 did not match" }
        }
    }

    private fun cleanupStaging() {
        if (!root.exists()) return
        root.listFiles()
            ?.filter { it.name.startsWith(".staging-") }
            ?.forEach(::deleteTree)
    }

    private fun deleteTree(target: File) {
        if (!target.exists()) return
        check(target.canonicalPath.startsWith(root.canonicalPath + File.separator)) {
            "refused to delete outside voice model storage"
        }
        target.walkBottomUp().forEach { file ->
            check(file.delete() || !file.exists()) { "could not delete ${file.name}" }
        }
    }

    private fun writeSynced(file: File, bytes: ByteArray) {
        FileOutputStream(file).use { output ->
            output.write(bytes)
            output.fd.sync()
        }
    }
}

private fun File.sha256(): String {
    val digest = MessageDigest.getInstance("SHA-256")
    inputStream().buffered().use { input ->
        val buffer = ByteArray(64 * 1024)
        while (true) {
            val read = input.read(buffer)
            if (read < 0) break
            digest.update(buffer, 0, read)
        }
    }
    return digest.digest().toHex()
}

private fun ByteArray.toHex(): String = joinToString(separator = "") { "%02x".format(it) }
