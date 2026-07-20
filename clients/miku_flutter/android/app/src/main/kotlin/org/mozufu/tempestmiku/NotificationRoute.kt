package org.mozufu.tempestmiku

import java.time.Instant
import java.util.UUID

internal enum class NotificationRouteKind(val wireName: String) {
    APPROVAL_REQUESTED("approval_requested"),
    APPROVAL_RESOLVED("approval_resolved"),
    SESSION_READY("session_ready");

    companion object {
        fun fromWireName(value: String): NotificationRouteKind? =
            entries.firstOrNull { it.wireName == value }
    }
}

internal data class NotificationRoute(
    val version: Int,
    val deliveryId: String,
    val kind: NotificationRouteKind,
    val sessionId: String,
    val approvalId: String?,
    val eventSeq: Long?,
    val expiresAt: Instant,
) {
    val clientMessageId: String get() = "notification-$deliveryId"
    val notificationId: Int get() = deliveryId.hashCode()
    val routeDedupeKey: String get() = "route:$deliveryId"

    fun decisionDedupeKey(decision: String): String = "decision:$deliveryId:$decision"
}

internal object NotificationRouteParser {
    fun parse(
        version: Int,
        deliveryId: String,
        kind: String,
        sessionId: String,
        approvalId: String?,
        eventSeq: Long?,
        expiresAt: String,
    ): NotificationRoute? {
        if (version != 1) return null
        val canonicalDeliveryId = canonicalUuid(deliveryId) ?: return null
        val canonicalSessionId = canonicalUuid(sessionId) ?: return null
        val routeKind = NotificationRouteKind.fromWireName(kind) ?: return null
        val canonicalApprovalId = approvalId?.takeIf(String::isNotBlank)?.let(::canonicalUuid)
        val expiry = try {
            Instant.parse(expiresAt)
        } catch (_: RuntimeException) {
            return null
        }
        when (routeKind) {
            NotificationRouteKind.APPROVAL_REQUESTED,
            NotificationRouteKind.APPROVAL_RESOLVED,
            -> if (canonicalApprovalId == null || eventSeq != null) return null
            NotificationRouteKind.SESSION_READY ->
                if (canonicalApprovalId != null || eventSeq == null || eventSeq <= 0) return null
        }
        return NotificationRoute(
            version = version,
            deliveryId = canonicalDeliveryId,
            kind = routeKind,
            sessionId = canonicalSessionId,
            approvalId = canonicalApprovalId,
            eventSeq = eventSeq,
            expiresAt = expiry,
        )
    }

    private fun canonicalUuid(value: String): String? = try {
        UUID.fromString(value).toString().takeIf { it == value.lowercase() }
    } catch (_: IllegalArgumentException) {
        null
    }
}

internal object InlineReplyPolicy {
    const val MAX_UTF16_UNITS = 2000
    const val MAX_CODE_POINTS = 1000
    const val MAX_UTF8_BYTES = 4096

    fun sanitize(value: CharSequence?): String? {
        val cleaned = value
            ?.toString()
            ?.replace("\r\n", "\n")
            ?.replace('\r', '\n')
            ?.filter { character ->
                character == '\n' || character == '\t' || !character.isISOControl()
            }
            ?.trim()
            .orEmpty()
        if (cleaned.isEmpty() || cleaned.length > MAX_UTF16_UNITS) return null
        if (cleaned.codePointCount(0, cleaned.length) > MAX_CODE_POINTS) return null
        if (cleaned.toByteArray(Charsets.UTF_8).size > MAX_UTF8_BYTES) return null
        return cleaned
    }
}

internal enum class InlineReplyDisposition {
    SUCCESS,
    RETRY,
    EXPIRED,
    REVOKED,
    MISSING_SESSION,
    PERMANENT_FAILURE,
}

internal object InlineReplyOutcomePolicy {
    fun classify(
        responseCode: Int?,
        runAttemptCount: Int,
        now: Instant,
        expiresAt: Instant,
    ): InlineReplyDisposition {
        if (now >= expiresAt) return InlineReplyDisposition.EXPIRED
        if (responseCode != null && responseCode in 200..299) return InlineReplyDisposition.SUCCESS
        if (responseCode == 401 || responseCode == 403) return InlineReplyDisposition.REVOKED
        if (responseCode == 404) return InlineReplyDisposition.MISSING_SESSION
        val transient = responseCode == null || responseCode in 500..599 ||
            responseCode == 408 || responseCode == 425 || responseCode == 429
        if (transient && runAttemptCount < 4 && now.plusSeconds(10) < expiresAt) {
            return InlineReplyDisposition.RETRY
        }
        return InlineReplyDisposition.PERMANENT_FAILURE
    }
}
