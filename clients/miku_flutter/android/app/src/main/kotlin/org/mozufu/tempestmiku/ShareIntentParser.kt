package org.mozufu.tempestmiku

internal const val MAX_SHARE_TEXT_LENGTH = 16_384
internal const val MAX_SHARE_SUBJECT_LENGTH = 240

internal data class ParsedShareIntent(
    val text: String,
    val subject: String?,
    val truncated: Boolean,
) {
    fun toEvent(): Map<String, Any> = buildMap {
        put("text", text)
        subject?.let { put("subject", it) }
        put("truncated", truncated)
    }
}

internal object ShareIntentParser {
    private const val ACTION_SEND = "android.intent.action.SEND"

    fun parse(
        action: String?,
        mimeType: String?,
        text: CharSequence?,
        subject: CharSequence?,
        hasUriPayload: Boolean = false,
    ): ParsedShareIntent? {
        if (
            action != ACTION_SEND ||
            mimeType?.lowercase() != "text/plain" ||
            hasUriPayload
        ) return null
        val sanitizedText = sanitize(text).trim()
        if (sanitizedText.isEmpty()) return null
        val sanitizedSubject = sanitize(subject).trim()
        val textTruncated = sanitizedText.length > MAX_SHARE_TEXT_LENGTH
        val subjectTruncated = sanitizedSubject.length > MAX_SHARE_SUBJECT_LENGTH
        return ParsedShareIntent(
            text = takeWithoutSplittingSurrogate(sanitizedText, MAX_SHARE_TEXT_LENGTH),
            subject = takeWithoutSplittingSurrogate(sanitizedSubject, MAX_SHARE_SUBJECT_LENGTH)
                .ifEmpty { null },
            truncated = textTruncated || subjectTruncated,
        )
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
