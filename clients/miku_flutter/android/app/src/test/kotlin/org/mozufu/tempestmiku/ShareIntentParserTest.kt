package org.mozufu.tempestmiku

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class ShareIntentParserTest {
    @Test
    fun acceptsPlainTextAndSanitizesControlCharacters() {
        val parsed = ShareIntentParser.parse(
            action = "android.intent.action.SEND",
            mimeType = "text/plain",
            text = "  https://example.test/\u0000path\n  ",
            subject = " Example title\u0007 ",
        )

        requireNotNull(parsed)
        assertEquals("https://example.test/path", parsed.text)
        assertEquals("Example title", parsed.subject)
        assertFalse(parsed.truncated)
    }

    @Test
    fun rejectsUnknownActionsTypesAndEmptyText() {
        assertNull(ShareIntentParser.parse("android.intent.action.VIEW", "text/plain", "x", null))
        assertNull(ShareIntentParser.parse("android.intent.action.SEND", "text/html", "x", null))
        assertNull(ShareIntentParser.parse("android.intent.action.SEND", "text/plain", " \u0000 ", null))
        assertNull(
            ShareIntentParser.parse(
                "android.intent.action.SEND",
                "text/plain",
                "x",
                null,
                hasUriPayload = true,
            ),
        )
    }

    @Test
    fun boundsTextAndSubject() {
        val parsed = ShareIntentParser.parse(
            action = "android.intent.action.SEND",
            mimeType = "text/plain",
            text = "x".repeat(MAX_SHARE_TEXT_LENGTH + 50),
            subject = "s".repeat(MAX_SHARE_SUBJECT_LENGTH + 20),
        )

        requireNotNull(parsed)
        assertEquals(MAX_SHARE_TEXT_LENGTH, parsed.text.length)
        assertEquals(MAX_SHARE_SUBJECT_LENGTH, parsed.subject?.length)
        assertTrue(parsed.truncated)

        val emojiBoundary = ShareIntentParser.parse(
            action = "android.intent.action.SEND",
            mimeType = "text/plain",
            text = "x".repeat(MAX_SHARE_TEXT_LENGTH - 1) + "😀",
            subject = null,
        )
        requireNotNull(emojiBoundary)
        assertEquals(MAX_SHARE_TEXT_LENGTH - 1, emojiBoundary.text.length)
        assertTrue(emojiBoundary.truncated)
    }
}
