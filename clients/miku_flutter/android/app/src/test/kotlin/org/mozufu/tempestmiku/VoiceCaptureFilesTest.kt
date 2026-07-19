package org.mozufu.tempestmiku

import java.io.File
import java.nio.file.Files
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import kotlin.concurrent.thread
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class VoiceCaptureFilesTest {
    private val first = "10000000-0000-4000-8000-000000000001"
    private val second = "20000000-0000-4000-8000-000000000002"

    @Test
    fun `bounds accept exact maximum and reject malformed audio`() {
        VoiceCaptureFiles.requireCaptureId(first)
        VoiceCaptureFiles.requirePcm16Size(2)
        VoiceCaptureFiles.requirePcm16Size(VOICE_MAX_PCM16_BYTES)

        assertFails { VoiceCaptureFiles.requireCaptureId("../../capture") }
        assertFails { VoiceCaptureFiles.requirePcm16Size(0) }
        assertFails { VoiceCaptureFiles.requirePcm16Size(3) }
        assertFails { VoiceCaptureFiles.requirePcm16Size(VOICE_MAX_PCM16_BYTES + 2) }
    }

    @Test
    fun `protocol rejects duplicate and mismatched completion and cancel is terminal`() {
        val gate = VoiceCaptureGate()
        gate.begin(first)
        assertFails { gate.begin(second) }
        assertFails { gate.complete(second) }
        assertFalse(gate.cancel(second))
        assertTrue(gate.cancel(first))
        assertFalse(gate.cancel(first))
        assertFails { gate.begin(first) }
        gate.begin(second)
        gate.complete(second)
        assertFails { gate.begin(second) }
    }

    @Test
    fun `retiring recorder blocks the next capture until exact cleanup finishes`() {
        val gate = VoiceCaptureGate()
        gate.begin(first)
        gate.complete(first)
        gate.beginRetirement(first)

        assertFails { gate.begin(second) }
        assertFails { gate.finishRetirement(second) }
        assertFails { gate.reset() }

        gate.finishRetirement(first)
        gate.begin(second)
        assertTrue(gate.cancel(second))
        gate.beginRetirement(second)
        gate.finishRetirement(second)
        gate.reset()
    }

    @Test
    fun `cold start recovery deletes only bounded voice scratch files`() {
        val root = Files.createTempDirectory("miku-voice-test").toFile()
        try {
            val files = VoiceCaptureFiles(root)
            files.fileFor(first).writeBytes(byteArrayOf(0, 0))
            val unrelated = File(root, "keep.txt").apply { writeText("keep") }

            assertEquals(1, files.purgeOrphans())
            assertFalse(files.fileFor(first).exists())
            assertTrue(unrelated.exists())
            assertEquals("keep", unrelated.readText())
        } finally {
            root.deleteRecursively()
        }
    }

    @Test
    fun `recorder exit wait never reports a live capture thread as stopped`() {
        val started = CountDownLatch(1)
        val release = CountDownLatch(1)
        val recorderThread =
            thread(name = "blocked-voice-recorder-test") {
                started.countDown()
                release.await(5, TimeUnit.SECONDS)
            }
        assertTrue(started.await(5, TimeUnit.SECONDS))
        try {
            assertFalse(awaitVoiceRecorderExit(recorderThread, timeoutMillis = 10))
        } finally {
            release.countDown()
            recorderThread.join(5_000)
        }
        assertFalse(recorderThread.isAlive)
        assertTrue(awaitVoiceRecorderExit(recorderThread, timeoutMillis = 10))
    }

    @Test
    fun `negative recorder reads fail closed unless stop requested them`() {
        assertEquals("microphone read failed (-6)", voiceRecorderReadFailure(-6, false))
        assertEquals(null, voiceRecorderReadFailure(-6, true))
        assertEquals(null, voiceRecorderReadFailure(0, false))
        assertEquals(null, voiceRecorderReadFailure(512, false))
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
