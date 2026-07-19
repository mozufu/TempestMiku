package org.mozufu.tempestmiku

import java.io.File

internal const val VOICE_SAMPLE_RATE = 16_000
internal const val VOICE_MAX_SECONDS = 60
internal const val VOICE_MAX_PCM16_BYTES = VOICE_SAMPLE_RATE * VOICE_MAX_SECONDS * 2

/** Pure one-shot protocol gate used by the Android recorder and JVM tests. */
internal class VoiceCaptureGate {
    private var activeId: String? = null
    private var retiringId: String? = null
    private val terminalIds = ArrayDeque<String>()

    fun begin(captureId: String) {
        VoiceCaptureFiles.requireCaptureId(captureId)
        check(activeId == null) { "voice capture is already active" }
        check(retiringId == null) { "previous voice capture cleanup is still pending" }
        check(captureId !in terminalIds) { "voice capture id was already consumed" }
        activeId = captureId
    }

    fun complete(captureId: String) {
        check(activeId == captureId) { "voice capture id does not match active capture" }
        activeId = null
        terminalIds.addLast(captureId)
        while (terminalIds.size > 64) terminalIds.removeFirst()
    }

    fun beginRetirement(captureId: String) {
        check(activeId == null) { "active voice capture cannot retire" }
        check(retiringId == null) { "voice capture cleanup is already pending" }
        check(captureId in terminalIds) { "only a terminal voice capture can retire" }
        retiringId = captureId
    }

    fun finishRetirement(captureId: String) {
        check(retiringId == captureId) { "voice capture cleanup id did not match" }
        retiringId = null
    }

    fun cancel(captureId: String? = null): Boolean {
        val current = activeId ?: return false
        if (captureId != null && captureId != current) return false
        activeId = null
        terminalIds.addLast(current)
        while (terminalIds.size > 64) terminalIds.removeFirst()
        return true
    }

    fun reset() {
        check(retiringId == null) { "voice capture cleanup is still pending" }
        activeId = null
        terminalIds.clear()
    }
}

/** App-private, no-backup scratch storage for one foreground voice capture. */
internal class VoiceCaptureFiles(private val root: File) {
    companion object {
        private val captureIdPattern = Regex(
            "^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-4[0-9a-fA-F]{3}-[89aAbB][0-9a-fA-F]{3}-[0-9a-fA-F]{12}$",
        )
        private val fileNamePattern = Regex(
            "^voice-([0-9a-fA-F]{8}-[0-9a-fA-F]{4}-4[0-9a-fA-F]{3}-[89aAbB][0-9a-fA-F]{3}-[0-9a-fA-F]{12})\\.pcm$",
        )

        fun requireCaptureId(captureId: String) {
            require(captureIdPattern.matches(captureId)) { "invalid voice capture id" }
        }

        fun requirePcm16Size(sizeBytes: Int) {
            require(sizeBytes in 2..VOICE_MAX_PCM16_BYTES && sizeBytes % 2 == 0) {
                "voice capture must be non-empty bounded PCM16"
            }
        }
    }

    fun prepare(): File {
        if (!root.exists() && !root.mkdirs()) {
            throw IllegalStateException("could not create voice scratch directory")
        }
        if (!root.isDirectory) throw IllegalStateException("voice scratch path is not a directory")
        return root
    }

    fun fileFor(captureId: String): File {
        requireCaptureId(captureId)
        return File(prepare(), "voice-$captureId.pcm")
    }

    fun delete(captureId: String) {
        val file = fileFor(captureId)
        if (file.exists() && !file.delete()) {
            file.writeBytes(ByteArray(0))
            file.delete()
        }
    }

    /** Deletes only files created by this feature; called on every process start. */
    fun purgeOrphans(): Int {
        val directory = prepare()
        var deleted = 0
        directory.listFiles()?.forEach { candidate ->
            if (candidate.isFile && fileNamePattern.matches(candidate.name)) {
                if (candidate.delete()) {
                    deleted += 1
                } else {
                    candidate.writeBytes(ByteArray(0))
                    if (candidate.delete()) deleted += 1
                }
            }
        }
        return deleted
    }
}
