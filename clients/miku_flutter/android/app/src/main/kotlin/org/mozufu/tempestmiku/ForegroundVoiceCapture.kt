package org.mozufu.tempestmiku

import android.media.AudioFormat
import android.media.AudioRecord
import android.media.MediaRecorder
import java.io.BufferedOutputStream
import java.io.FileOutputStream
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicReference

internal data class CompletedVoiceCapture(
    val captureId: String,
    val pcm16: ByteArray,
)

private const val VOICE_RECORDER_JOIN_TIMEOUT_MS = 2_000L
private const val VOICE_RECORDER_FORCE_JOIN_TIMEOUT_MS = 2_000L

internal fun awaitVoiceRecorderExit(
    thread: Thread,
    timeoutMillis: Long = VOICE_RECORDER_JOIN_TIMEOUT_MS,
): Boolean {
    try {
        thread.join(timeoutMillis)
    } catch (_: InterruptedException) {
        Thread.currentThread().interrupt()
        return false
    }
    return !thread.isAlive
}

/**
 * One foreground-only microphone capture.
 *
 * Raw PCM is written only below noBackupFilesDir, bounded before every write,
 * and removed before bytes are returned to Flutter. The owning Activity also
 * cancels this object from onPause/onDestroy.
 */
internal class ForegroundVoiceCapture(
    private val files: VoiceCaptureFiles,
    private val recorderFactory: () -> AudioRecord = ::newVoiceRecorder,
) {
    private data class Active(
        val captureId: String,
        val recorder: AudioRecord,
        val cancelled: AtomicBoolean,
        val discardResult: AtomicBoolean,
        val released: AtomicBoolean,
        val retirementMonitorStarted: AtomicBoolean,
        val failure: AtomicReference<String?>,
        val retirementLock: Any,
        val thread: Thread,
    )

    private val lock = Any()
    private var active: Active? = null
    private var retiring: Active? = null
    private val cancelledIds = ArrayDeque<String>()
    private val gate = VoiceCaptureGate()

    fun recoverOrphans(): Int {
        val pending = synchronized(lock) {
            check(active == null) { "cannot recover while voice capture is active" }
            retiring
        }
        if (pending != null && !finishCancellation(pending)) {
            throw IllegalStateException("voice recorder cleanup is still pending")
        }
        return synchronized(lock) {
            check(active == null && retiring == null) {
                "cannot recover while voice capture cleanup is pending"
            }
            gate.reset()
            cancelledIds.clear()
            files.purgeOrphans()
        }
    }

    fun start(captureId: String) {
        VoiceCaptureFiles.requireCaptureId(captureId)
        synchronized(lock) {
            check(retiring == null) { "previous voice capture cleanup is still pending" }
            gate.begin(captureId)
            var recorder: AudioRecord? = null
            try {
                val output = files.fileFor(captureId)
                if (output.exists()) files.delete(captureId)
                recorder = recorderFactory()
                check(recorder.state == AudioRecord.STATE_INITIALIZED) {
                    "microphone recorder could not initialize"
                }
                val readyRecorder = recorder
                val cancelled = AtomicBoolean(false)
                val discardResult = AtomicBoolean(false)
                val released = AtomicBoolean(false)
                val retirementMonitorStarted = AtomicBoolean(false)
                val failure = AtomicReference<String?>(null)
                val retirementLock = Any()
                val thread = Thread(
                    {
                        val buffer = ByteArray(4096)
                        try {
                            BufferedOutputStream(FileOutputStream(output, false)).use { sink ->
                                var written = 0
                                while (!cancelled.get() && written < VOICE_MAX_PCM16_BYTES) {
                                    val requested = minOf(buffer.size, VOICE_MAX_PCM16_BYTES - written)
                                    val read = readyRecorder.read(
                                        buffer,
                                        0,
                                        requested,
                                        AudioRecord.READ_BLOCKING,
                                    )
                                    if (read > 0) {
                                        val evenRead = read - (read % 2)
                                        if (evenRead > 0) {
                                            sink.write(buffer, 0, evenRead)
                                            written += evenRead
                                        }
                                    } else if (read < 0) {
                                        voiceRecorderReadFailure(read, cancelled.get())?.let {
                                            failure.compareAndSet(null, it)
                                        }
                                        break
                                    }
                                }
                                sink.flush()
                            }
                        } catch (error: Exception) {
                            if (!cancelled.get()) {
                                failure.compareAndSet(
                                    null,
                                    "microphone capture failed: ${error.message ?: error.javaClass.simpleName}",
                                )
                            }
                            files.delete(captureId)
                        } finally {
                            buffer.fill(0)
                            try {
                                if (readyRecorder.recordingState == AudioRecord.RECORDSTATE_RECORDING) {
                                    readyRecorder.stop()
                                }
                            } catch (_: IllegalStateException) {
                                // A concurrent foreground stop may already have stopped it.
                            } finally {
                                releaseRecorder(readyRecorder, released)
                            }
                        }
                    },
                    "miku-voice-capture",
                )
                active =
                    Active(
                        captureId,
                        readyRecorder,
                        cancelled,
                        discardResult,
                        released,
                        retirementMonitorStarted,
                        failure,
                        retirementLock,
                        thread,
                    )
                readyRecorder.startRecording()
                thread.start()
            } catch (error: Exception) {
                active = null
                gate.cancel(captureId)
                try {
                    if (recorder?.recordingState == AudioRecord.RECORDSTATE_RECORDING) {
                        recorder.stop()
                    }
                } catch (_: IllegalStateException) {
                    // Initialization rollback remains fail-closed.
                }
                recorder?.release()
                files.delete(captureId)
                throw error
            }
        }
    }

    fun stop(captureId: String): CompletedVoiceCapture {
        val current = synchronized(lock) {
            val session = active ?: error("no voice capture is active")
            gate.complete(captureId)
            gate.beginRetirement(captureId)
            active = null
            retiring = session
            session
        }
        return synchronized(current.retirementLock) {
            stopRecorder(current)
            if (!awaitRecorderCleanup(current)) {
                current.discardResult.set(true)
                rememberCancelled(current.captureId)
                files.delete(captureId)
                monitorRetirement(current)
                error("voice recorder did not stop")
            }
            if (current.discardResult.get()) {
                files.delete(captureId)
                completeRetirement(current)
                error("voice capture was cancelled")
            }
            current.failure.get()?.let { failure ->
                files.delete(captureId)
                completeRetirement(current)
                error(failure)
            }
            val file = files.fileFor(captureId)
            var rejectedPcm: ByteArray? = null
            val bytes = try {
                val pcm = file.readBytes()
                rejectedPcm = pcm
                VoiceCaptureFiles.requirePcm16Size(pcm.size)
                rejectedPcm = null
                pcm
            } catch (error: Exception) {
                rejectedPcm?.fill(0)
                completeRetirement(current)
                throw error
            } finally {
                files.delete(captureId)
            }
            completeRetirement(current)
            CompletedVoiceCapture(captureId, bytes)
        }
    }

    fun cancel(captureId: String? = null): Boolean {
        val current = synchronized(lock) {
            val session = active
            if (session != null) {
                if (captureId != null && captureId != session.captureId) return false
                check(gate.cancel(captureId)) { "active voice capture could not be cancelled" }
                gate.beginRetirement(session.captureId)
                active = null
                retiring = session
                rememberCancelledLocked(session.captureId)
                session
            } else {
                val pending = retiring
                if (pending != null) {
                    if (captureId != null && captureId != pending.captureId) return false
                    rememberCancelledLocked(pending.captureId)
                    pending
                } else {
                    if (captureId != null && captureId in cancelledIds) return true
                    return false
                }
            }
        }
        current.discardResult.set(true)
        if (!finishCancellation(current)) {
            throw IllegalStateException("voice recorder did not stop after forced cleanup")
        }
        return true
    }

    private fun finishCancellation(current: Active): Boolean =
        synchronized(current.retirementLock) {
            current.discardResult.set(true)
            stopRecorder(current)
            val stopped = awaitRecorderCleanup(current)
            files.delete(current.captureId)
            if (stopped) {
                completeRetirement(current)
            } else {
                monitorRetirement(current)
            }
            stopped
        }

    private fun stopRecorder(active: Active) {
        active.cancelled.set(true)
        try {
            active.recorder.stop()
        } catch (_: IllegalStateException) {
            // Cancellation remains fail-closed even if Android already stopped it.
        }
    }

    private fun awaitRecorderCleanup(current: Active): Boolean {
        if (awaitVoiceRecorderExit(current.thread)) return true
        current.thread.interrupt()
        releaseRecorder(current.recorder, current.released)
        return awaitVoiceRecorderExit(
            current.thread,
            timeoutMillis = VOICE_RECORDER_FORCE_JOIN_TIMEOUT_MS,
        )
    }

    private fun monitorRetirement(current: Active) {
        if (!current.retirementMonitorStarted.compareAndSet(false, true)) return
        Thread(
            {
                try {
                    current.thread.join()
                } finally {
                    files.delete(current.captureId)
                    completeRetirement(current)
                }
            },
            "miku-voice-retirement",
        ).apply {
            isDaemon = true
            start()
        }
    }

    private fun completeRetirement(current: Active) {
        synchronized(lock) {
            if (retiring === current && !current.thread.isAlive) {
                gate.finishRetirement(current.captureId)
                retiring = null
            }
        }
    }

    private fun rememberCancelled(captureId: String) {
        synchronized(lock) { rememberCancelledLocked(captureId) }
    }

    private fun rememberCancelledLocked(captureId: String) {
        if (captureId !in cancelledIds) cancelledIds.addLast(captureId)
        while (cancelledIds.size > 64) cancelledIds.removeFirst()
    }
}

internal fun voiceRecorderReadFailure(
    readResult: Int,
    stopRequested: Boolean,
): String? =
    if (readResult < 0 && !stopRequested) {
        "microphone read failed ($readResult)"
    } else {
        null
    }

private fun releaseRecorder(
    recorder: AudioRecord,
    released: AtomicBoolean,
) {
    if (released.compareAndSet(false, true)) recorder.release()
}

private fun newVoiceRecorder(): AudioRecord {
    val minimum = AudioRecord.getMinBufferSize(
        VOICE_SAMPLE_RATE,
        AudioFormat.CHANNEL_IN_MONO,
        AudioFormat.ENCODING_PCM_16BIT,
    )
    check(minimum > 0) { "16 kHz mono PCM16 recording is unsupported" }
    return AudioRecord(
        MediaRecorder.AudioSource.VOICE_RECOGNITION,
        VOICE_SAMPLE_RATE,
        AudioFormat.CHANNEL_IN_MONO,
        AudioFormat.ENCODING_PCM_16BIT,
        maxOf(minimum, 4096),
    )
}
