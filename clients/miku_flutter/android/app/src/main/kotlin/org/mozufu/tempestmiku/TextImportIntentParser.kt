package org.mozufu.tempestmiku

internal const val MAX_TEXT_IMPORT_LENGTH = 16_384
internal const val MAX_TEXT_IMPORT_SUBJECT_LENGTH = 240
internal const val ACTION_QUICK_CAPTURE_V1 =
    "org.mozufu.tempestmiku.action.QUICK_CAPTURE_V1"
internal const val EXTRA_QUICK_CAPTURE_ID =
    "org.mozufu.tempestmiku.extra.QUICK_CAPTURE_ID"
internal const val EXTRA_QUICK_CAPTURE_TEXT =
    "org.mozufu.tempestmiku.extra.QUICK_CAPTURE_TEXT"

internal enum class TextImportSource(val wireValue: String) {
    SHARE("share"),
    SELECTION("selection"),
    QUICK_CAPTURE("quick_capture"),
}

internal data class ParsedTextImportIntent(
    val text: String,
    val subject: String?,
    val truncated: Boolean,
    val source: TextImportSource,
    val eventId: String? = null,
) {
    fun toEvent(): Map<String, Any> = buildMap {
        put("text", text)
        subject?.let { put("subject", it) }
        put("truncated", truncated)
        put("source", source.wireValue)
        eventId?.let { put("eventId", it) }
    }
}

internal object TextImportIntentParser {
    private const val ACTION_SEND = "android.intent.action.SEND"
    private const val ACTION_PROCESS_TEXT = "android.intent.action.PROCESS_TEXT"

    fun parse(
        action: String?,
        mimeType: String?,
        sharedText: CharSequence?,
        selectedText: CharSequence?,
        quickCaptureText: CharSequence?,
        quickCaptureId: String?,
        subject: CharSequence?,
        hasDisallowedPayload: Boolean = false,
    ): ParsedTextImportIntent? {
        if (hasDisallowedPayload) return null
        val source = when (action) {
            ACTION_SEND -> TextImportSource.SHARE
            ACTION_PROCESS_TEXT -> TextImportSource.SELECTION
            ACTION_QUICK_CAPTURE_V1 -> TextImportSource.QUICK_CAPTURE
            else -> return null
        }
        if (source == TextImportSource.QUICK_CAPTURE) {
            if (mimeType != null || !isValidQuickCaptureId(quickCaptureId)) return null
        } else if (mimeType?.lowercase() != "text/plain") {
            return null
        }
        val sanitizedText = sanitize(
            when (source) {
                TextImportSource.SHARE -> sharedText
                TextImportSource.SELECTION -> selectedText
                TextImportSource.QUICK_CAPTURE -> quickCaptureText
            },
        ).trim()
        if (sanitizedText.isEmpty() && source != TextImportSource.QUICK_CAPTURE) return null
        val sanitizedSubject = if (source == TextImportSource.SHARE) {
            sanitize(subject).trim()
        } else {
            ""
        }
        val textTruncated = sanitizedText.length > MAX_TEXT_IMPORT_LENGTH
        val subjectTruncated = sanitizedSubject.length > MAX_TEXT_IMPORT_SUBJECT_LENGTH
        return ParsedTextImportIntent(
            text = takeWithoutSplittingSurrogate(sanitizedText, MAX_TEXT_IMPORT_LENGTH),
            subject = takeWithoutSplittingSurrogate(
                sanitizedSubject,
                MAX_TEXT_IMPORT_SUBJECT_LENGTH,
            ).ifEmpty { null },
            truncated = textTruncated || subjectTruncated,
            source = source,
            eventId = quickCaptureId.takeIf { source == TextImportSource.QUICK_CAPTURE },
        )
    }

    private fun isValidQuickCaptureId(value: String?): Boolean {
        if (value == null || value.length != 36) return false
        return value.indices.all { index ->
            when (index) {
                8, 13, 18, 23 -> value[index] == '-'
                else -> value[index].digitToIntOrNull(16) != null
            }
        }
    }

    private fun takeWithoutSplittingSurrogate(value: String, maxLength: Int): String {
        if (value.length <= maxLength) return value
        val end = if (
            maxLength > 0 &&
            value[maxLength - 1].isHighSurrogate() &&
            value[maxLength].isLowSurrogate()
        ) maxLength - 1 else maxLength
        return value.substring(0, end)
    }

    private fun sanitize(value: CharSequence?): String {
        if (value == null) return ""
        return value
            .filter { char -> char == '\n' || char == '\t' || !char.isISOControl() }
            .toString()
    }
}
