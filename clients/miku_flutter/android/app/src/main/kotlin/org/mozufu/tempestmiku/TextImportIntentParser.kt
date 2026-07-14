package org.mozufu.tempestmiku

internal const val MAX_TEXT_IMPORT_LENGTH = 16_384
internal const val MAX_TEXT_IMPORT_SUBJECT_LENGTH = 240

internal enum class TextImportSource(val wireValue: String) {
    SHARE("share"),
    SELECTION("selection"),
}

internal data class ParsedTextImportIntent(
    val text: String,
    val subject: String?,
    val truncated: Boolean,
    val source: TextImportSource,
) {
    fun toEvent(): Map<String, Any> = buildMap {
        put("text", text)
        subject?.let { put("subject", it) }
        put("truncated", truncated)
        put("source", source.wireValue)
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
        subject: CharSequence?,
        hasUriPayload: Boolean = false,
    ): ParsedTextImportIntent? {
        if (mimeType?.lowercase() != "text/plain" || hasUriPayload) return null
        val source = when (action) {
            ACTION_SEND -> TextImportSource.SHARE
            ACTION_PROCESS_TEXT -> TextImportSource.SELECTION
            else -> return null
        }
        val sanitizedText = sanitize(
            when (source) {
                TextImportSource.SHARE -> sharedText
                TextImportSource.SELECTION -> selectedText
            },
        ).trim()
        if (sanitizedText.isEmpty()) return null
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
