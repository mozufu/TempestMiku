package org.mozufu.tempestmiku

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class TextImportIntentParserTest {
    @Test
    fun acceptsPlainTextShareAndSanitizesControlCharacters() {
        val parsed = TextImportIntentParser.parse(
            action = "android.intent.action.SEND",
            mimeType = "text/plain",
            sharedText = "  https://example.test/\u0000path\n  ",
            selectedText = "ignored selection",
            quickCaptureText = null,
            quickCaptureId = null,
            subject = " Example title\u0007 ",
        )

        requireNotNull(parsed)
        assertEquals("https://example.test/path", parsed.text)
        assertEquals("Example title", parsed.subject)
        assertEquals(TextImportSource.SHARE, parsed.source)
        assertFalse(parsed.truncated)
    }

    @Test
    fun acceptsSelectedPlainTextWithoutShareExtras() {
        val parsed = TextImportIntentParser.parse(
            action = "android.intent.action.PROCESS_TEXT",
            mimeType = "text/plain",
            sharedText = "ignored share",
            selectedText = "  Explain this\u0000 code  ",
            quickCaptureText = null,
            quickCaptureId = null,
            subject = "ignored subject",
        )

        requireNotNull(parsed)
        assertEquals("Explain this code", parsed.text)
        assertNull(parsed.subject)
        assertEquals(TextImportSource.SELECTION, parsed.source)
        assertFalse(parsed.truncated)
    }

    @Test
    fun rejectsUnknownActionsTypesEmptyTextAndUriPayloads() {
        assertNull(parse(action = "android.intent.action.VIEW"))
        assertNull(parse(mimeType = "text/html"))
        assertNull(parse(sharedText = " \u0000 "))
        assertNull(parse(hasDisallowedPayload = true))
        assertNull(
            parse(
                action = "android.intent.action.PROCESS_TEXT",
                sharedText = "wrong extra",
                selectedText = null,
            ),
        )
        assertNull(
            parse(
                action = "android.intent.action.SEND",
                sharedText = null,
                selectedText = "wrong extra",
            ),
        )
    }

    @Test
    fun boundsSharedAndSelectedTextWithoutSplittingSurrogates() {
        val shared = parse(
            sharedText = "x".repeat(MAX_TEXT_IMPORT_LENGTH + 50),
            subject = "s".repeat(MAX_TEXT_IMPORT_SUBJECT_LENGTH + 20),
        )

        requireNotNull(shared)
        assertEquals(MAX_TEXT_IMPORT_LENGTH, shared.text.length)
        assertEquals(MAX_TEXT_IMPORT_SUBJECT_LENGTH, shared.subject?.length)
        assertTrue(shared.truncated)

        val selection = parse(
            action = "android.intent.action.PROCESS_TEXT",
            sharedText = null,
            selectedText = "x".repeat(MAX_TEXT_IMPORT_LENGTH - 1) + "😀",
        )
        requireNotNull(selection)
        assertEquals(MAX_TEXT_IMPORT_LENGTH - 1, selection.text.length)
        assertNull(selection.subject)
        assertTrue(selection.truncated)
    }

    @Test
    fun acceptsEmptyQuickCaptureWithVersionedId() {
        val parsed = parse(
            action = ACTION_QUICK_CAPTURE_V1,
            mimeType = null,
            sharedText = null,
            quickCaptureText = null,
            quickCaptureId = QUICK_CAPTURE_ID,
        )

        requireNotNull(parsed)
        assertEquals("", parsed.text)
        assertNull(parsed.subject)
        assertEquals(TextImportSource.QUICK_CAPTURE, parsed.source)
        assertEquals(QUICK_CAPTURE_ID, parsed.eventId)
        assertEquals(QUICK_CAPTURE_ID, parsed.toEvent()["eventId"])
        assertFalse(parsed.truncated)
    }

    @Test
    fun sanitizesAndBoundsQuickCapturePrefill() {
        val parsed = parse(
            action = ACTION_QUICK_CAPTURE_V1,
            mimeType = null,
            sharedText = null,
            quickCaptureText =
                "  ${"x".repeat(MAX_TEXT_IMPORT_LENGTH)}\u0000extra  ",
            quickCaptureId = QUICK_CAPTURE_ID,
            subject = "ignored",
        )

        requireNotNull(parsed)
        assertEquals(MAX_TEXT_IMPORT_LENGTH, parsed.text.length)
        assertNull(parsed.subject)
        assertTrue(parsed.truncated)
    }

    @Test
    fun rejectsMalformedQuickCaptureContracts() {
        assertNull(
            parse(
                action = ACTION_QUICK_CAPTURE_V1,
                mimeType = null,
                sharedText = null,
                quickCaptureId = null,
            ),
        )
        assertNull(
            parse(
                action = ACTION_QUICK_CAPTURE_V1,
                mimeType = null,
                sharedText = null,
                quickCaptureId = "not-a-versioned-capture-id",
            ),
        )
        assertNull(
            parse(
                action = ACTION_QUICK_CAPTURE_V1,
                mimeType = "text/plain",
                sharedText = null,
                quickCaptureId = QUICK_CAPTURE_ID,
            ),
        )
        assertNull(
            parse(
                action = ACTION_QUICK_CAPTURE_V1,
                mimeType = null,
                sharedText = null,
                quickCaptureId = QUICK_CAPTURE_ID,
                hasDisallowedPayload = true,
            ),
        )
    }

    private fun parse(
        action: String = "android.intent.action.SEND",
        mimeType: String? = "text/plain",
        sharedText: CharSequence? = "x",
        selectedText: CharSequence? = null,
        quickCaptureText: CharSequence? = null,
        quickCaptureId: String? = null,
        subject: CharSequence? = null,
        hasDisallowedPayload: Boolean = false,
    ): ParsedTextImportIntent? = TextImportIntentParser.parse(
        action = action,
        mimeType = mimeType,
        sharedText = sharedText,
        selectedText = selectedText,
        quickCaptureText = quickCaptureText,
        quickCaptureId = quickCaptureId,
        subject = subject,
        hasDisallowedPayload = hasDisallowedPayload,
    )

    private companion object {
        const val QUICK_CAPTURE_ID = "12345678-1234-4abc-8def-1234567890ab"
    }
}
