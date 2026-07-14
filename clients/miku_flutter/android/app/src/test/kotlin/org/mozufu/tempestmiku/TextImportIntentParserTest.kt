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
        assertNull(parse(hasUriPayload = true))
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

    private fun parse(
        action: String = "android.intent.action.SEND",
        mimeType: String = "text/plain",
        sharedText: CharSequence? = "x",
        selectedText: CharSequence? = null,
        subject: CharSequence? = null,
        hasUriPayload: Boolean = false,
    ): ParsedTextImportIntent? = TextImportIntentParser.parse(
        action = action,
        mimeType = mimeType,
        sharedText = sharedText,
        selectedText = selectedText,
        subject = subject,
        hasUriPayload = hasUriPayload,
    )
}
