package org.mozufu.tempestmiku

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.RemoteInput
import android.content.Context
import android.content.Intent
import android.graphics.drawable.Icon
import android.os.Build
import org.json.JSONObject

internal const val ACTION_OPEN_NOTIFICATION_ROUTE =
    "org.mozufu.tempestmiku.OPEN_NOTIFICATION_ROUTE"
internal const val EXTRA_ROUTE_VERSION = "routeVersion"
internal const val EXTRA_DELIVERY_ID = "deliveryId"
internal const val EXTRA_ROUTE_KIND = "routeKind"
internal const val EXTRA_EVENT_SEQ = "eventSeq"
internal const val EXTRA_EXPIRES_AT = "expiresAt"

internal object NotificationIntentData {
    fun put(intent: Intent, route: NotificationRoute): Intent = intent.apply {
        putExtra(EXTRA_ROUTE_VERSION, route.version)
        putExtra(EXTRA_DELIVERY_ID, route.deliveryId)
        putExtra(EXTRA_ROUTE_KIND, route.kind.wireName)
        putExtra(EXTRA_SESSION_ID, route.sessionId)
        route.approvalId?.let { putExtra(EXTRA_APPROVAL_ID, it) }
        route.eventSeq?.let { putExtra(EXTRA_EVENT_SEQ, it) }
        putExtra(EXTRA_EXPIRES_AT, route.expiresAt.toString())
    }

    fun route(intent: Intent): NotificationRoute? = NotificationRouteParser.parse(
        version = intent.getIntExtra(EXTRA_ROUTE_VERSION, 0),
        deliveryId = intent.getStringExtra(EXTRA_DELIVERY_ID).orEmpty(),
        kind = intent.getStringExtra(EXTRA_ROUTE_KIND).orEmpty(),
        sessionId = intent.getStringExtra(EXTRA_SESSION_ID).orEmpty(),
        approvalId = intent.getStringExtra(EXTRA_APPROVAL_ID),
        eventSeq = if (intent.hasExtra(EXTRA_EVENT_SEQ)) intent.getLongExtra(EXTRA_EVENT_SEQ, 0) else null,
        expiresAt = intent.getStringExtra(EXTRA_EXPIRES_AT).orEmpty(),
    )

    fun route(json: JSONObject?): NotificationRoute? {
        json ?: return null
        return NotificationRouteParser.parse(
            version = json.optInt("version"),
            deliveryId = json.optString("deliveryId"),
            kind = json.optString("kind"),
            sessionId = json.optString("sessionId"),
            approvalId = json.optString("approvalId").takeIf(String::isNotBlank),
            eventSeq = if (json.has("eventSeq")) json.optLong("eventSeq") else null,
            expiresAt = json.optString("expiresAt"),
        )
    }

    fun toJson(route: NotificationRoute): JSONObject = JSONObject()
        .put("version", route.version)
        .put("deliveryId", route.deliveryId)
        .put("kind", route.kind.wireName)
        .put("sessionId", route.sessionId)
        .put("expiresAt", route.expiresAt.toString())
        .also { json ->
            route.approvalId?.let { json.put("approvalId", it) }
            route.eventSeq?.let { json.put("eventSeq", it) }
        }
}

internal object SessionNotifications {
    const val REPLY_KEY = "tempestmiku.inlineReply"
    private const val CHANNEL_ID = "session_messages"
    private const val CHANNEL_NAME = "Conversation updates"

    fun ensureChannel(context: Context) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return
        context.getSystemService(NotificationManager::class.java).createNotificationChannel(
            NotificationChannel(CHANNEL_ID, CHANNEL_NAME, NotificationManager.IMPORTANCE_HIGH).apply {
                description = "Alerts when TempestMiku finishes a reply."
                setShowBadge(true)
            },
        )
    }

    fun show(context: Context, route: NotificationRoute) {
        if (route.kind != NotificationRouteKind.SESSION_READY) return
        ensureChannel(context)
        val replyIntent = NotificationIntentData.put(
            Intent(context, InlineReplyReceiver::class.java),
            route,
        )
        val replyPendingIntent = PendingIntent.getBroadcast(
            context,
            route.deliveryId.hashCode(),
            replyIntent,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_MUTABLE,
        )
        val remoteInput = RemoteInput.Builder(REPLY_KEY)
            .setLabel("Reply to Miku")
            .build()
        val replyActionBuilder = Notification.Action.Builder(
            Icon.createWithResource(context, R.mipmap.ic_launcher),
            "Reply",
            replyPendingIntent,
        )
            .addRemoteInput(remoteInput)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.N) {
            replyActionBuilder.setAllowGeneratedReplies(false)
        }
        val replyAction = replyActionBuilder.build()
        val builder = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            Notification.Builder(context, CHANNEL_ID)
        } else {
            Notification.Builder(context)
        }
        val publicVersion = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            Notification.Builder(context, CHANNEL_ID)
        } else {
            Notification.Builder(context)
        }.setSmallIcon(R.mipmap.ic_launcher)
            .setContentTitle("TempestMiku")
            .setContentText("New conversation activity.")
            .build()
        val expandedStyle = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.N) {
            Notification.MessagingStyle("You")
                .addMessage(
                    "Open the conversation or send a reply.",
                    System.currentTimeMillis(),
                    "Miku",
                )
        } else {
            Notification.BigTextStyle()
                .bigText("Open the conversation or send a reply.")
        }
        val notification = builder
            .setSmallIcon(R.mipmap.ic_launcher)
            .setContentTitle("Miku replied")
            .setContentText("Open the conversation or send a reply.")
            .setStyle(expandedStyle)
            .setCategory(Notification.CATEGORY_MESSAGE)
            .setAutoCancel(true)
            .setOnlyAlertOnce(true)
            .setContentIntent(openPendingIntent(context, route))
            .setVisibility(Notification.VISIBILITY_PRIVATE)
            .setPublicVersion(publicVersion)
            .addAction(replyAction)
            .build()
        context.getSystemService(NotificationManager::class.java)
            .notify(route.notificationId, notification)
    }

    fun openPendingIntent(context: Context, route: NotificationRoute): PendingIntent {
        val open = NotificationIntentData.put(
            Intent(context, MainActivity::class.java).apply {
                action = ACTION_OPEN_NOTIFICATION_ROUTE
                flags = Intent.FLAG_ACTIVITY_CLEAR_TOP or Intent.FLAG_ACTIVITY_SINGLE_TOP
            },
            route,
        )
        return PendingIntent.getActivity(
            context,
            route.deliveryId.hashCode(),
            open,
            PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
        )
    }
}
