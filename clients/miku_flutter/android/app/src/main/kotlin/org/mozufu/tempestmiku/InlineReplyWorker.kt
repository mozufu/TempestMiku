package org.mozufu.tempestmiku

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.RemoteInput
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import androidx.work.Constraints
import androidx.work.Data
import androidx.work.ExistingWorkPolicy
import androidx.work.NetworkType
import androidx.work.OneTimeWorkRequestBuilder
import androidx.work.WorkManager
import androidx.work.Worker
import androidx.work.WorkerParameters
import org.json.JSONObject
import java.io.IOException
import java.net.HttpURLConnection
import java.net.URL
import java.nio.charset.StandardCharsets
import java.security.KeyStore
import java.time.Instant
import java.util.concurrent.TimeUnit
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

internal data class InlineReplyAuthority(val serverBaseUrl: String, val token: String)

internal object InlineReplySecretStore {
    private const val KEY_ALIAS = "tempestmiku.inlineReply.v1"
    private const val PREFERENCES = "tempestmiku.inlineReply.v1"
    private const val AUTHORITY = "authority"
    private val authorityAad = "tempestmiku.inlineReply.authority.v1".toByteArray()
    private val workAad = "tempestmiku.inlineReply.work.v1".toByteArray()

    fun saveAuthority(context: Context, serverBaseUrl: String, token: String): Boolean {
        val normalized = validateServerBaseUrl(serverBaseUrl) ?: return false
        if (!token.startsWith("tmk_dev_") || token.length > 512) return false
        val value = JSONObject()
            .put("version", 1)
            .put("serverBaseUrl", normalized)
            .put("token", token)
            .toString()
        return try {
            context.getSharedPreferences(PREFERENCES, Context.MODE_PRIVATE)
                .edit()
                .putString(AUTHORITY, encrypt(value, authorityAad))
                .commit()
        } catch (_: Exception) {
            false
        }
    }

    fun readAuthority(context: Context): InlineReplyAuthority? {
        val encrypted = context.getSharedPreferences(PREFERENCES, Context.MODE_PRIVATE)
            .getString(AUTHORITY, null) ?: return null
        return try {
            val value = JSONObject(decrypt(encrypted, authorityAad))
            if (value.optInt("version") != 1) return null
            val serverBaseUrl = validateServerBaseUrl(value.optString("serverBaseUrl")) ?: return null
            val token = value.optString("token")
            if (!token.startsWith("tmk_dev_") || token.length > 512) return null
            InlineReplyAuthority(serverBaseUrl, token)
        } catch (_: Exception) {
            null
        }
    }

    fun clearAuthority(context: Context) {
        context.getSharedPreferences(PREFERENCES, Context.MODE_PRIVATE)
            .edit()
            .remove(AUTHORITY)
            .commit()
    }

    fun encryptWork(value: String): String = encrypt(value, workAad)
    fun decryptWork(value: String): String = decrypt(value, workAad)

    private fun encrypt(value: String, aad: ByteArray): String {
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.ENCRYPT_MODE, secretKey())
        cipher.updateAAD(aad)
        val ciphertext = cipher.doFinal(value.toByteArray(StandardCharsets.UTF_8))
        return Base64.encodeToString(cipher.iv, Base64.NO_WRAP) + "." +
            Base64.encodeToString(ciphertext, Base64.NO_WRAP)
    }

    private fun decrypt(value: String, aad: ByteArray): String {
        val parts = value.split('.', limit = 2)
        require(parts.size == 2)
        val iv = Base64.decode(parts[0], Base64.NO_WRAP)
        val ciphertext = Base64.decode(parts[1], Base64.NO_WRAP)
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.DECRYPT_MODE, secretKey(), GCMParameterSpec(128, iv))
        cipher.updateAAD(aad)
        return String(cipher.doFinal(ciphertext), StandardCharsets.UTF_8)
    }

    private fun secretKey(): SecretKey {
        val keyStore = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        (keyStore.getKey(KEY_ALIAS, null) as? SecretKey)?.let { return it }
        return KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore").run {
            init(
                KeyGenParameterSpec.Builder(
                    KEY_ALIAS,
                    KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT,
                )
                    .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                    .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                    .setRandomizedEncryptionRequired(true)
                    .build(),
            )
            generateKey()
        }
    }

    private fun validateServerBaseUrl(value: String): String? {
        val uri = Uri.parse(value.trim())
        val host = uri.host?.lowercase() ?: return null
        val secure = uri.scheme == "https"
        val debugLoopback = BuildConfig.DEBUG && uri.scheme == "http" &&
            (host == "localhost" || host == "127.0.0.1" || host == "10.0.2.2" || host == "::1")
        if (!secure && !debugLoopback) return null
        if (uri.userInfo != null || uri.query != null || uri.fragment != null) return null
        if (uri.path?.let { it.isNotEmpty() && it != "/" } == true) return null
        return uri.buildUpon().path(null).query(null).fragment(null).build().toString().trimEnd('/')
    }
}

class InlineReplyReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val route = NotificationIntentData.route(intent) ?: return
        val text = InlineReplyPolicy.sanitize(
            RemoteInput.getResultsFromIntent(intent)?.getCharSequence(SessionNotifications.REPLY_KEY),
        )
        if (text == null) {
            InlineReplyFeedback.failed(context, route, "Reply not sent: enter a shorter message.")
            return
        }
        if (Instant.now() >= route.expiresAt) {
            InlineReplyFeedback.failed(context, route, "Reply not sent: this notification expired.")
            return
        }
        if (InlineReplySecretStore.readAuthority(context) == null) {
            InlineReplyFeedback.failed(context, route, "Reply not sent: pair this device again.")
            return
        }
        val payload = JSONObject()
            .put("route", NotificationIntentData.toJson(route))
            .put("text", text)
            .toString()
        val encrypted = try {
            InlineReplySecretStore.encryptWork(payload)
        } catch (_: Exception) {
            InlineReplyFeedback.failed(context, route, "Reply not sent: secure storage failed.")
            return
        }
        val request = OneTimeWorkRequestBuilder<InlineReplyWorker>()
            .setInputData(Data.Builder().putString(InlineReplyWorker.INPUT, encrypted).build())
            .setConstraints(
                Constraints.Builder().setRequiredNetworkType(NetworkType.CONNECTED).build(),
            )
            .setBackoffCriteria(
                androidx.work.BackoffPolicy.EXPONENTIAL,
                10,
                TimeUnit.SECONDS,
            )
            .addTag("tempestmiku-inline-reply")
            .build()
        WorkManager.getInstance(context).enqueueUniqueWork(
            "tempestmiku-inline-reply-${route.deliveryId}",
            ExistingWorkPolicy.KEEP,
            request,
        )
        InlineReplyFeedback.sending(context, route)
    }
}

class InlineReplyWorker(context: Context, parameters: WorkerParameters) : Worker(context, parameters) {
    companion object { const val INPUT = "encryptedReply" }

    override fun doWork(): Result {
        val decoded = try {
            JSONObject(InlineReplySecretStore.decryptWork(inputData.getString(INPUT) ?: return Result.failure()))
        } catch (_: Exception) {
            return Result.failure()
        }
        val route = NotificationIntentData.route(decoded.optJSONObject("route")) ?: return Result.failure()
        val text = InlineReplyPolicy.sanitize(decoded.optString("text")) ?: return terminal(route, "Reply not sent: invalid message.")
        if (Instant.now() >= route.expiresAt) return terminal(route, "Reply not sent: this notification expired.")
        val authority = InlineReplySecretStore.readAuthority(applicationContext)
            ?: return terminal(route, "Reply not sent: pair this device again.")
        val body = JSONObject()
            .put("clientMessageId", route.clientMessageId)
            .put("content", text)
            .toString()
        val responseCode = try {
            val connection = URL("${authority.serverBaseUrl}/sessions/${route.sessionId}/messages")
                .openConnection() as HttpURLConnection
            connection.requestMethod = "POST"
            connection.connectTimeout = 10_000
            connection.readTimeout = 20_000
            connection.doOutput = true
            connection.setRequestProperty("Accept", "application/json")
            connection.setRequestProperty("Content-Type", "application/json")
            connection.setRequestProperty("Authorization", "Bearer ${authority.token}")
            connection.outputStream.use { output ->
                output.write(body.toByteArray(StandardCharsets.UTF_8))
            }
            connection.responseCode.also { connection.disconnect() }
        } catch (_: IOException) {
            null
        } catch (_: RuntimeException) {
            return terminal(route, "Reply not sent: invalid server target.")
        }
        return when (
            InlineReplyOutcomePolicy.classify(
                responseCode,
                runAttemptCount,
                Instant.now(),
                route.expiresAt,
            )
        ) {
            InlineReplyDisposition.SUCCESS -> {
                InlineReplyFeedback.sent(applicationContext, route)
                Result.success()
            }
            InlineReplyDisposition.REVOKED -> {
                InlineReplySecretStore.clearAuthority(applicationContext)
                terminal(route, "Reply not sent: pairing was revoked.")
            }
            InlineReplyDisposition.MISSING_SESSION ->
                terminal(route, "Reply not sent: the session no longer exists.")
            InlineReplyDisposition.EXPIRED ->
                terminal(route, "Reply not sent: this notification expired.")
            InlineReplyDisposition.RETRY -> {
                InlineReplyFeedback.retrying(applicationContext, route)
                Result.retry()
            }
            InlineReplyDisposition.PERMANENT_FAILURE ->
                terminal(route, "Reply not sent: server rejected the message.")
        }
    }

    private fun terminal(route: NotificationRoute, message: String): Result {
        InlineReplyFeedback.failed(applicationContext, route, message)
        return Result.failure()
    }
}

internal object InlineReplyFeedback {
    private const val CHANNEL_ID = "message_replies"

    fun sending(context: Context, route: NotificationRoute) = show(context, route, "Sending reply…")
    fun retrying(context: Context, route: NotificationRoute) = show(context, route, "Reply waiting for the server…")
    fun sent(context: Context, route: NotificationRoute) = show(context, route, "Reply sent.")
    fun failed(context: Context, route: NotificationRoute, message: String) = show(context, route, message)

    private fun show(context: Context, route: NotificationRoute, message: String) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            context.getSystemService(NotificationManager::class.java).createNotificationChannel(
                NotificationChannel(CHANNEL_ID, "Message replies", NotificationManager.IMPORTANCE_DEFAULT),
            )
        }
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
            .setContentText("Open the app for details.")
            .build()
        val notification = builder
            .setSmallIcon(R.mipmap.ic_launcher)
            .setContentTitle("TempestMiku")
            .setContentText(message)
            .setOnlyAlertOnce(true)
            .setAutoCancel(true)
            .setVisibility(Notification.VISIBILITY_PRIVATE)
            .setPublicVersion(publicVersion)
            .setContentIntent(SessionNotifications.openPendingIntent(context, route))
            .build()
        context.getSystemService(NotificationManager::class.java)
            .notify(route.notificationId, notification)
    }
}
