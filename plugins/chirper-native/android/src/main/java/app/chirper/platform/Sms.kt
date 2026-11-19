package app.chirper.platform

import android.Manifest
import android.app.Activity
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.PackageManager
import android.database.ContentObserver
import android.net.Uri
import android.os.Build
import android.os.Handler
import android.os.HandlerThread
import android.provider.ContactsContract
import android.telephony.PhoneNumberUtils
import android.telephony.SmsManager
import android.telephony.TelephonyManager
import androidx.annotation.RequiresApi
import androidx.core.content.ContextCompat
import app.tauri.plugin.Channel
import app.tauri.plugin.JSObject
import org.json.JSONArray
import java.io.ByteArrayOutputStream
import java.io.File
import java.util.Base64
import java.util.Locale
import java.util.UUID
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicInteger

/**
 * SMS and MMS through the phone's own provider. Chirper is not the default SMS app, so this reads
 * the provider, sends through [SmsManager] (the system writes the sent message to the provider) and
 * watches the provider for changes. It cannot mark messages read, delete them or write to the
 * provider.
 *
 * Every provider query runs on the calling thread. Message bodies and addresses are never logged.
 * A missing permission surfaces as [SecurityException]; the commands turn that into a rejection.
 */
internal object SmsProvider {
    /** The largest number of rows one call returns. */
    private const val MAX_LIMIT = 500

    /** Coalesces provider changes: at most one change notice per window. */
    private const val COALESCE_MS = 250L

    private const val ACTION_SENT = "app.chirper.platform.SMS_SENT"
    private const val ACTION_DELIVERED = "app.chirper.platform.SMS_DELIVERED"
    private const val EXTRA_SEND_ID = "sendId"

    // Telephony provider columns and box numbers (the same values as android.provider.Telephony).
    private const val SMS_TYPE_INBOX = 1
    private const val SMS_TYPE_SENT = 2
    private const val SMS_TYPE_DRAFT = 3
    private const val SMS_TYPE_OUTBOX = 4
    private const val SMS_TYPE_FAILED = 5
    private const val SMS_TYPE_QUEUED = 6
    private const val MMS_FROM_ADDRESS_TYPE = 137

    private val SMS_URI: Uri = Uri.parse("content://sms")
    private val MMS_URI: Uri = Uri.parse("content://mms")
    private val MMS_PART_URI: Uri = Uri.parse("content://mms/part")
    private val CONVERSATIONS_URI: Uri = Uri.parse("content://mms-sms/conversations?simple=true")
    private val CANONICAL_ADDRESSES_URI: Uri = Uri.parse("content://mms-sms/canonical-addresses")
    private val WATCH_URI: Uri = Uri.parse("content://mms-sms/")

    private val requestCodes = AtomicInteger(1)
    private val changeScheduled = AtomicBoolean(false)
    private val lock = Any()

    @Volatile
    private var channel: Channel? = null

    @Volatile
    private var handler: Handler? = null
    private var observer: ContentObserver? = null
    private var observerThread: HandlerThread? = null
    private var statusRegistered = false

    /** The permission state, without asking. */
    fun permissions(context: Context): JSObject = JSObject()
        .put("read", granted(context, Manifest.permission.READ_SMS))
        .put("send", granted(context, Manifest.permission.SEND_SMS))
        .put("receive", granted(context, Manifest.permission.RECEIVE_SMS))
        .put("contacts", granted(context, Manifest.permission.READ_CONTACTS))

    private fun granted(context: Context, permission: String): Boolean =
        ContextCompat.checkSelfPermission(context, permission) == PackageManager.PERMISSION_GRANTED

    /** The most recent conversations, newest first. */
    fun threads(context: Context, limit: Int): JSONArray {
        val cap = limit.coerceIn(1, MAX_LIMIT)
        val addresses = canonicalAddresses(context)
        val resolver = AddressResolver(context)
        val rows = ArrayList<Pair<Long, JSObject>>()
        context.contentResolver.query(
            CONVERSATIONS_URI,
            arrayOf("_id", "recipient_ids", "date"),
            null,
            null,
            null,
        )?.use { cursor ->
            while (cursor.moveToNext()) {
                val threadId = cursor.getLong(0)
                val date = cursor.getLong(2)
                val people = cursor.getString(1).orEmpty()
                    .split(' ')
                    .filter { it.isNotBlank() }
                    .mapNotNull { addresses[it] }
                    .map { resolver.address(it) }
                rows += date to JSObject()
                    .put("threadId", threadId)
                    .put("addresses", JSONArray(people))
                    .put("snippetDateMs", date)
            }
        }
        val out = JSONArray()
        rows.sortedByDescending { it.first }.take(cap).forEach { out.put(it.second) }
        return out
    }

    /**
     * Messages, newest first. With [threadId] and [beforeMs] they are the messages of one
     * conversation before that time. With [afterMs] they are the messages of every conversation
     * after that time, oldest first, so a caller can page forward.
     */
    fun messages(
        context: Context,
        threadId: Long?,
        beforeMs: Long?,
        afterMs: Long?,
        limit: Int,
    ): JSONArray {
        val cap = limit.coerceIn(1, MAX_LIMIT)
        val order = if (afterMs != null) "ASC" else "DESC"
        val resolver = AddressResolver(context)
        val rows = ArrayList<Pair<Long, JSObject>>()

        val smsWhere = ArrayList<String>()
        val smsArgs = ArrayList<String>()
        smsWhere += "type != $SMS_TYPE_DRAFT"
        threadId?.let { smsWhere += "thread_id = ?"; smsArgs += it.toString() }
        beforeMs?.let { smsWhere += "date < ?"; smsArgs += it.toString() }
        afterMs?.let { smsWhere += "date > ?"; smsArgs += it.toString() }
        context.contentResolver.query(
            SMS_URI,
            arrayOf("_id", "thread_id", "address", "date", "body", "type"),
            smsWhere.joinToString(" AND "),
            smsArgs.toTypedArray(),
            "date $order LIMIT $cap",
        )?.use { cursor ->
            while (cursor.moveToNext()) {
                val status = when (cursor.getInt(5)) {
                    SMS_TYPE_INBOX -> "received"
                    SMS_TYPE_SENT -> "sent"
                    SMS_TYPE_OUTBOX, SMS_TYPE_QUEUED -> "sending"
                    SMS_TYPE_FAILED -> "failed"
                    else -> continue
                }
                val date = cursor.getLong(3)
                rows += date to JSObject()
                    .put("id", "sms:${cursor.getLong(0)}")
                    .put("threadId", cursor.getLong(1))
                    .put("sender", resolver.address(cursor.getString(2).orEmpty()))
                    .put("fromMe", cursor.getInt(5) != SMS_TYPE_INBOX)
                    .put("dateMs", date)
                    .put("body", cursor.getString(4).orEmpty())
                    .put("parts", JSONArray())
                    .put("status", status)
            }
        }

        // MMS dates are in seconds. The query is inclusive at the second; the exact comparison
        // is made on milliseconds below.
        val mmsWhere = ArrayList<String>()
        val mmsArgs = ArrayList<String>()
        mmsWhere += "msg_box != $SMS_TYPE_DRAFT"
        threadId?.let { mmsWhere += "thread_id = ?"; mmsArgs += it.toString() }
        beforeMs?.let { mmsWhere += "date <= ?"; mmsArgs += (it / 1000).toString() }
        afterMs?.let { mmsWhere += "date >= ?"; mmsArgs += (it / 1000).toString() }
        context.contentResolver.query(
            MMS_URI,
            arrayOf("_id", "thread_id", "date", "msg_box"),
            mmsWhere.joinToString(" AND "),
            mmsArgs.toTypedArray(),
            "date $order LIMIT $cap",
        )?.use { cursor ->
            while (cursor.moveToNext()) {
                val status = when (cursor.getInt(3)) {
                    SMS_TYPE_INBOX -> "received"
                    SMS_TYPE_SENT -> "sent"
                    SMS_TYPE_OUTBOX, SMS_TYPE_QUEUED -> "sending"
                    SMS_TYPE_FAILED -> "failed"
                    else -> continue
                }
                val id = cursor.getLong(0)
                val dateMs = cursor.getLong(2) * 1000
                if (beforeMs != null && dateMs >= beforeMs) continue
                if (afterMs != null && dateMs <= afterMs) continue
                val (body, parts) = mmsContent(context, id)
                rows += dateMs to JSObject()
                    .put("id", "mms:$id")
                    .put("threadId", cursor.getLong(1))
                    .put("sender", mmsSender(context, id, resolver))
                    .put("fromMe", cursor.getInt(3) != SMS_TYPE_INBOX)
                    .put("dateMs", dateMs)
                    .put("body", body)
                    .put("parts", parts)
                    .put("status", status)
            }
        }

        val sorted = if (afterMs != null) {
            rows.sortedBy { it.first }
        } else {
            rows.sortedByDescending { it.first }
        }
        val out = JSONArray()
        sorted.take(cap).forEach { out.put(it.second) }
        return out
    }

    /** The text and the media parts of one MMS. */
    private fun mmsContent(context: Context, id: Long): Pair<String, JSONArray> {
        val texts = ArrayList<String>()
        val media = JSONArray()
        context.contentResolver.query(
            MMS_PART_URI,
            arrayOf("_id", "ct", "name", "cl", "text", "_data"),
            "mid = ?",
            arrayOf(id.toString()),
            "_id ASC",
        )?.use { cursor ->
            while (cursor.moveToNext()) {
                val contentType = cursor.getString(1).orEmpty()
                when {
                    contentType == "text/plain" -> cursor.getString(4)
                        ?.takeIf { it.isNotEmpty() }
                        ?.let { texts += it }
                    contentType == "application/smil" -> Unit
                    else -> {
                        val name = cursor.getString(2) ?: cursor.getString(3).orEmpty()
                        val size = cursor.getString(5)
                            ?.let { File(it).takeIf { file -> file.exists() }?.length() }
                            ?: -1L
                        media.put(
                            JSObject()
                                .put("partId", cursor.getLong(0))
                                .put("mime", contentType)
                                .put("name", name)
                                .put("size", size),
                        )
                    }
                }
            }
        }
        return texts.joinToString("\n") to media
    }

    /** The sender of an MMS: its `from` address. Sent MMS have none, and the caller marks them. */
    private fun mmsSender(context: Context, id: Long, resolver: AddressResolver): JSObject {
        context.contentResolver.query(
            Uri.parse("content://mms/$id/addr"),
            arrayOf("address", "type"),
            "type = ?",
            arrayOf(MMS_FROM_ADDRESS_TYPE.toString()),
            null,
        )?.use { cursor ->
            if (cursor.moveToFirst()) {
                return resolver.address(cursor.getString(0).orEmpty())
            }
        }
        return resolver.address("")
    }

    private fun canonicalAddresses(context: Context): Map<String, String> {
        val out = HashMap<String, String>()
        context.contentResolver.query(
            CANONICAL_ADDRESSES_URI,
            arrayOf("_id", "address"),
            null,
            null,
            null,
        )?.use { cursor ->
            while (cursor.moveToNext()) {
                out[cursor.getString(0).orEmpty()] = cursor.getString(1).orEmpty()
            }
        }
        return out
    }

    /** Resolves addresses to E.164 and contact names, with the country of this phone. */
    private class AddressResolver(private val context: Context) {
        private val country: String = countryIso(context)
        private val contacts: Boolean = ContextCompat.checkSelfPermission(
            context,
            Manifest.permission.READ_CONTACTS,
        ) == PackageManager.PERMISSION_GRANTED
        private val names = HashMap<String, String>()

        fun address(raw: String): JSObject = JSObject()
            .put("raw", raw)
            .put("e164", e164(raw))
            .put("name", name(raw))

        private fun e164(raw: String): String {
            if (raw.isEmpty()) return ""
            return PhoneNumberUtils.formatNumberToE164(raw, country) ?: ""
        }

        private fun name(raw: String): String {
            if (!contacts || raw.isEmpty()) return ""
            return names.getOrPut(raw) { lookupName(raw) }
        }

        private fun lookupName(raw: String): String {
            val uri = Uri.withAppendedPath(
                ContactsContract.PhoneLookup.CONTENT_FILTER_URI,
                Uri.encode(raw),
            )
            return try {
                context.contentResolver.query(
                    uri,
                    arrayOf(ContactsContract.PhoneLookup.DISPLAY_NAME),
                    null,
                    null,
                    null,
                )?.use { cursor ->
                    if (cursor.moveToFirst()) cursor.getString(0).orEmpty() else ""
                } ?: ""
            } catch (_: SecurityException) {
                ""
            }
        }

        private companion object {
            fun countryIso(context: Context): String {
                val telephony = context.getSystemService(TelephonyManager::class.java)
                val iso = telephony?.networkCountryIso.takeUnless { it.isNullOrEmpty() }
                    ?: telephony?.simCountryIso.takeUnless { it.isNullOrEmpty() }
                    ?: Locale.getDefault().country
                return iso.uppercase(Locale.ROOT)
            }
        }
    }

    /**
     * Sends one text to each address and returns the id that the status events carry. A group is
     * sent as one text per address: a non-default app cannot send group MMS. The sent and
     * delivered reports are attached to the last part only, so they describe the whole text.
     */
    fun send(context: Context, addresses: List<String>, text: String, subscriptionId: Int?): String {
        require(addresses.isNotEmpty()) { "no address" }
        require(text.isNotEmpty()) { "no text" }
        val sendId = UUID.randomUUID().toString()
        ensureStatusReceiver(context)
        val manager = smsManager(context, subscriptionId)
        val parts = ArrayList(manager.divideMessage(text))
        for (address in addresses) {
            val sent = ArrayList<PendingIntent?>(parts.size)
            val delivered = ArrayList<PendingIntent?>(parts.size)
            for (index in parts.indices) {
                val last = index == parts.lastIndex
                sent += if (last) statusIntent(context, ACTION_SENT, sendId) else null
                delivered += if (last) statusIntent(context, ACTION_DELIVERED, sendId) else null
            }
            manager.sendMultipartTextMessage(address, null, parts, sent, delivered)
        }
        return sendId
    }

    @Suppress("DEPRECATION")
    private fun smsManager(context: Context, subscriptionId: Int?): SmsManager {
        return if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            defaultManagerApi31(context, subscriptionId)
        } else if (subscriptionId != null) {
            SmsManager.getSmsManagerForSubscriptionId(subscriptionId)
        } else {
            SmsManager.getDefault()
        }
    }

    @RequiresApi(Build.VERSION_CODES.S)
    private fun defaultManagerApi31(context: Context, subscriptionId: Int?): SmsManager {
        val base = context.getSystemService(SmsManager::class.java)
        return if (subscriptionId != null) base.createForSubscriptionId(subscriptionId) else base
    }

    private fun statusIntent(context: Context, action: String, sendId: String): PendingIntent {
        // A dynamic receiver cannot be named in a class, so the intent is pinned to this app.
        val intent = Intent(action)
            .setPackage(context.packageName)
            .putExtra(EXTRA_SEND_ID, sendId)
        return PendingIntent.getBroadcast(
            context,
            requestCodes.incrementAndGet(),
            intent,
            PendingIntent.FLAG_IMMUTABLE,
        )
    }

    private val statusReceiver = object : BroadcastReceiver() {
        override fun onReceive(context: Context, intent: Intent) {
            val sendId = intent.getStringExtra(EXTRA_SEND_ID) ?: return
            val code = resultCode
            val (status, error) = when {
                code == Activity.RESULT_OK && intent.action == ACTION_DELIVERED -> "delivered" to ""
                code == Activity.RESULT_OK -> "sent" to ""
                else -> "failed" to "result $code"
            }
            emit(
                JSObject()
                    .put("kind", "sendStatus")
                    .put("sendId", sendId)
                    .put("status", status)
                    .put("error", error),
            )
        }
    }

    @Synchronized
    private fun ensureStatusReceiver(context: Context) {
        if (statusRegistered) return
        val filter = IntentFilter().apply {
            addAction(ACTION_SENT)
            addAction(ACTION_DELIVERED)
        }
        ContextCompat.registerReceiver(
            context.applicationContext,
            statusReceiver,
            filter,
            ContextCompat.RECEIVER_NOT_EXPORTED,
        )
        statusRegistered = true
    }

    /**
     * Starts reporting provider changes and send results to [target]. A second call replaces the
     * channel and keeps the one observer.
     */
    fun watch(context: Context, target: Channel) {
        synchronized(lock) {
            channel = target
            ensureStatusReceiver(context)
            if (observer != null) return
            val thread = HandlerThread("chirper-sms-watch").also { it.start() }
            val looper = Handler(thread.looper)
            handler = looper
            val content = object : ContentObserver(looper) {
                override fun onChange(selfChange: Boolean) = changed()

                override fun onChange(selfChange: Boolean, uri: Uri?) = changed()
            }
            context.contentResolver.registerContentObserver(WATCH_URI, true, content)
            observer = content
            observerThread = thread
        }
    }

    private fun changed() {
        if (!changeScheduled.compareAndSet(false, true)) return
        handler?.postDelayed({
            changeScheduled.set(false)
            emit(JSObject().put("kind", "changed"))
        }, COALESCE_MS)
    }

    private fun emit(event: JSObject) {
        channel?.send(event)
    }

    /**
     * The bytes of one MMS part, base64. A part larger than [maxBytes] is refused while it is read.
     */
    fun mmsPart(context: Context, partId: Long, maxBytes: Long): String {
        val uri = Uri.withAppendedPath(MMS_PART_URI, partId.toString())
        val input = context.contentResolver.openInputStream(uri)
            ?: throw IllegalStateException("the MMS part is missing")
        return input.use { stream ->
            val out = ByteArrayOutputStream()
            val buffer = ByteArray(16 * 1024)
            while (true) {
                val read = stream.read(buffer)
                if (read < 0) break
                if (out.size().toLong() + read > maxBytes) {
                    throw IllegalStateException("the MMS part is larger than allowed")
                }
                out.write(buffer, 0, read)
            }
            Base64.getEncoder().encodeToString(out.toByteArray())
        }
    }
}
