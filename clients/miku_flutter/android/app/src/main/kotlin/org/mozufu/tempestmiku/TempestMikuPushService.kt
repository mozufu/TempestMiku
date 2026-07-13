package org.mozufu.tempestmiku

import android.os.Handler
import android.os.Looper
import io.flutter.plugin.common.EventChannel
import org.json.JSONObject
import org.unifiedpush.android.connector.FailedReason
import org.unifiedpush.android.connector.PushService
import org.unifiedpush.android.connector.data.PushEndpoint
import org.unifiedpush.android.connector.data.PushMessage
import java.nio.charset.StandardCharsets
import java.util.UUID

internal object UnifiedPushEvents {
    private var sink: EventChannel.EventSink? = null
    private val pending = mutableListOf<Map<String, Any>>()
    private val mainHandler = Handler(Looper.getMainLooper())

    @Synchronized
    fun listen(events: EventChannel.EventSink) {
        sink = events
        pending.toList().forEach(events::success)
        pending.clear()
    }

    @Synchronized
    fun cancel() {
        sink = null
    }

    fun emit(event: Map<String, Any>) {
        mainHandler.post {
            synchronized(this) {
                val current = sink
                if (current == null) {
                    pending.removeAll { queued -> queued["type"] == event["type"] }
                    pending.add(event)
                } else {
                    current.success(event)
                }
            }
        }
    }
}

class TempestMikuPushService : PushService() {
    override fun onNewEndpoint(endpoint: PushEndpoint, instance: String) {
        val keys = endpoint.pubKeySet
        if (keys == null) {
            UnifiedPushEvents.emit(mapOf("type" to "registrationFailed"))
            return
        }
        UnifiedPushEvents.emit(
            mapOf(
                "type" to "registration",
                "registration" to mapOf(
                    "endpoint" to endpoint.url,
                    "p256dh" to keys.pubKey,
                    "auth" to keys.auth,
                ),
            ),
        )
    }

    override fun onMessage(message: PushMessage, instance: String) {
        if (!message.decrypted) return
        val payload = try {
            JSONObject(String(message.content, StandardCharsets.UTF_8))
        } catch (_: Exception) {
            return
        }
        if (payload.optInt("version") != 1) return
        val sessionId = validUuid(payload.optString("sessionId")) ?: return
        val approvalId = validUuid(payload.optString("approvalId")) ?: return
        when (payload.optString("kind")) {
            "approval_requested" -> ApprovalNotifications.show(
                this,
                sessionId,
                approvalId,
                "",
            )
            "approval_resolved" -> ApprovalNotifications.cancel(this, approvalId)
        }
    }

    override fun onRegistrationFailed(reason: FailedReason, instance: String) {
        UnifiedPushEvents.emit(mapOf("type" to "registrationFailed"))
    }

    override fun onUnregistered(instance: String) {
        UnifiedPushEvents.emit(mapOf("type" to "unregistered"))
    }

    private fun validUuid(value: String): String? = try {
        UUID.fromString(value).toString()
    } catch (_: IllegalArgumentException) {
        null
    }
}
