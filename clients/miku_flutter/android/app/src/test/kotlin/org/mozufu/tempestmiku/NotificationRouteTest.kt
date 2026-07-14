package org.mozufu.tempestmiku

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import java.time.Instant

class NotificationRouteTest {
    private val deliveryId = "00000000-0000-4000-8000-000000000001"
    private val sessionId = "00000000-0000-4000-8000-000000000002"
    private val approvalId = "00000000-0000-4000-8000-000000000003"

    @Test
    fun parsesVersionedSessionAndApprovalRoutes() {
        val session = parse(kind = "session_ready", eventSeq = 42)
        assertEquals(NotificationRouteKind.SESSION_READY, session?.kind)
        assertEquals("notification-$deliveryId", session?.clientMessageId)
        assertEquals(42L, session?.eventSeq)

        val approval = parse(kind = "approval_requested", approvalId = approvalId)
        assertEquals(NotificationRouteKind.APPROVAL_REQUESTED, approval?.kind)
        assertEquals(approvalId, approval?.approvalId)
    }

    @Test
    fun routeDecoderFailsClosedForUnknownMismatchedOrMalformedData() {
        assertNull(parse(version = 2, kind = "session_ready", eventSeq = 1))
        assertNull(parse(kind = "unknown", eventSeq = 1))
        assertNull(parse(kind = "session_ready", approvalId = approvalId, eventSeq = 1))
        assertNull(parse(kind = "session_ready", eventSeq = null))
        assertNull(parse(kind = "approval_requested", approvalId = null))
        assertNull(parse(kind = "approval_requested", approvalId = approvalId, eventSeq = 1))
        assertNull(parse(kind = "session_ready", eventSeq = 1, sessionId = "not-a-uuid"))
        assertNull(parse(kind = "session_ready", eventSeq = 1, expiresAt = "tomorrow"))
    }

    @Test
    fun stableIdDeduplicatesDeliveryButNotDistinctNotifications() {
        val first = parse(kind = "session_ready", eventSeq = 1)!!
        val duplicate = parse(kind = "session_ready", eventSeq = 1)!!
        val other = NotificationRouteParser.parse(
            1,
            "00000000-0000-4000-8000-000000000004",
            "session_ready",
            sessionId,
            null,
            2,
            "2099-01-01T00:00:00Z",
        )!!
        assertEquals(first.clientMessageId, duplicate.clientMessageId)
        assertTrue(first.clientMessageId != other.clientMessageId)
        assertTrue(first.clientMessageId.length <= 128)
    }

    @Test
    fun replyPolicySanitizesControlsAndRejectsEmptyOrOversizedInput() {
        assertEquals("hello\nMiku", InlineReplyPolicy.sanitize(" \u0000hello\r\nMiku\u0007 "))
        assertNull(InlineReplyPolicy.sanitize(" \u0000\t "))
        assertNull(InlineReplyPolicy.sanitize("a".repeat(InlineReplyPolicy.MAX_CODE_POINTS + 1)))
        val emoji = "😀".repeat(InlineReplyPolicy.MAX_CODE_POINTS)
        assertEquals(InlineReplyPolicy.MAX_CODE_POINTS, InlineReplyPolicy.sanitize(emoji)?.codePointCount(0, emoji.length))
        assertNull(InlineReplyPolicy.sanitize("界".repeat(1400)))
    }

    @Test
    fun outcomePolicyBoundsRetryAndClassifiesTerminalFailures() {
        val now = Instant.parse("2098-12-31T23:00:00Z")
        val expiry = now.plusSeconds(60)
        assertEquals(
            InlineReplyDisposition.RETRY,
            InlineReplyOutcomePolicy.classify(null, 0, now, expiry),
        )
        assertEquals(
            InlineReplyDisposition.RETRY,
            InlineReplyOutcomePolicy.classify(503, 3, now, expiry),
        )
        assertEquals(
            InlineReplyDisposition.PERMANENT_FAILURE,
            InlineReplyOutcomePolicy.classify(503, 4, now, expiry),
        )
        assertEquals(
            InlineReplyDisposition.EXPIRED,
            InlineReplyOutcomePolicy.classify(200, 0, expiry, expiry),
        )
        assertEquals(
            InlineReplyDisposition.REVOKED,
            InlineReplyOutcomePolicy.classify(403, 0, now, expiry),
        )
        assertEquals(
            InlineReplyDisposition.MISSING_SESSION,
            InlineReplyOutcomePolicy.classify(404, 0, now, expiry),
        )
        assertEquals(
            InlineReplyDisposition.PERMANENT_FAILURE,
            InlineReplyOutcomePolicy.classify(409, 0, now, expiry),
        )
    }

    private fun parse(
        version: Int = 1,
        kind: String,
        sessionId: String = this.sessionId,
        approvalId: String? = null,
        eventSeq: Long? = null,
        expiresAt: String = Instant.parse("2099-01-01T00:00:00Z").toString(),
    ): NotificationRoute? = NotificationRouteParser.parse(
        version,
        deliveryId,
        kind,
        sessionId,
        approvalId,
        eventSeq,
        expiresAt,
    )
}
