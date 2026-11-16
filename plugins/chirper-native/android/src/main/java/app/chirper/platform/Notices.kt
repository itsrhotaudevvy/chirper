package app.chirper.platform

import android.annotation.SuppressLint
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.net.Uri
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import androidx.core.app.Person
import androidx.core.app.RemoteInput

/**
 * Notification channels and the message notifications. A message notification is a MessagingStyle
 * conversation of the last few messages of a room, with an inline reply. Tapping it opens
 * `chirper://chat/<roomId>`. With hidden content the notification is generic and has no history.
 */
internal object Notices {
    const val CHANNEL_CONNECTION = "connection"
    const val CHANNEL_MESSAGES = "messages"
    const val ACTION_REPLY = "app.chirper.platform.action.REPLY"
    const val EXTRA_ROOM_ID = "room_id"
    const val REPLY_KEY = "reply_text"
    private const val HISTORY_PER_ROOM = 6
    private const val SCHEME = "chirper"
    private const val ME = "You"

    private val history = mutableMapOf<String, ArrayDeque<Pair<String, String>>>()

    fun notificationId(roomId: String): Int = roomId.hashCode()

    fun createChannels(context: Context) {
        val manager = context.getSystemService(NotificationManager::class.java)
        manager.createNotificationChannel(
            NotificationChannel(
                CHANNEL_CONNECTION,
                context.getString(R.string.connection_channel_name),
                NotificationManager.IMPORTANCE_LOW,
            ).apply { description = context.getString(R.string.connection_channel_description) },
        )
        manager.createNotificationChannel(
            NotificationChannel(
                CHANNEL_MESSAGES,
                context.getString(R.string.messages_channel_name),
                NotificationManager.IMPORTANCE_DEFAULT,
            ).apply { description = context.getString(R.string.messages_channel_description) },
        )
    }

    /** Posts or updates the notification of [roomId]. Returns false when notifications are off. */
    @SuppressLint("MissingPermission") // The caller checks the runtime permission first.
    fun post(context: Context, roomId: String, title: String, body: String, hideContent: Boolean): Boolean {
        val manager = NotificationManagerCompat.from(context)
        if (!manager.areNotificationsEnabled()) return false
        val generic = NotificationCompat.Builder(context, CHANNEL_MESSAGES)
            .setSmallIcon(android.R.drawable.stat_notify_chat)
            .setContentTitle(context.getString(R.string.app_name_short))
            .setContentText(context.getString(R.string.generic_message_text))
            .build()
        val builder = NotificationCompat.Builder(context, CHANNEL_MESSAGES)
            .setSmallIcon(android.R.drawable.stat_notify_chat)
            .setContentIntent(openChatPendingIntent(context, roomId))
            .setAutoCancel(true)
            .setCategory(NotificationCompat.CATEGORY_MESSAGE)
            .setVisibility(NotificationCompat.VISIBILITY_PRIVATE)
            .setPublicVersion(generic)
            .addAction(replyAction(context, roomId))
        if (hideContent) {
            builder.setContentTitle(context.getString(R.string.app_name_short))
                .setContentText(context.getString(R.string.generic_message_text))
        } else {
            builder.setStyle(messagingStyle(roomId, title, body))
        }
        manager.notify(notificationId(roomId), builder.build())
        return true
    }

    private fun messagingStyle(roomId: String, sender: String, body: String): NotificationCompat.MessagingStyle {
        val lines = synchronized(history) {
            val room = history.getOrPut(roomId) { ArrayDeque() }
            room.addLast(sender to body)
            while (room.size > HISTORY_PER_ROOM) room.removeFirst()
            room.toList()
        }
        val style = NotificationCompat.MessagingStyle(Person.Builder().setName(ME).build())
        for ((name, text) in lines) {
            style.addMessage(text, System.currentTimeMillis(), Person.Builder().setName(name).build())
        }
        return style
    }

    private fun replyAction(context: Context, roomId: String): NotificationCompat.Action {
        val input = RemoteInput.Builder(REPLY_KEY)
            .setLabel(context.getString(R.string.reply_hint))
            .build()
        val intent = Intent(context, ReplyReceiver::class.java)
            .setAction(ACTION_REPLY)
            .putExtra(EXTRA_ROOM_ID, roomId)
        // A reply needs a mutable PendingIntent: the system fills in the RemoteInput result.
        val pending = PendingIntent.getBroadcast(
            context,
            notificationId(roomId),
            intent,
            PendingIntent.FLAG_MUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        return NotificationCompat.Action.Builder(android.R.drawable.ic_menu_send, context.getString(R.string.reply_action), pending)
            .addRemoteInput(input)
            .setAllowGeneratedReplies(false)
            .build()
    }

    private fun openChatPendingIntent(context: Context, roomId: String): PendingIntent =
        PendingIntent.getActivity(
            context,
            notificationId(roomId),
            openChatIntent(context, roomId),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )

    /** The launcher activity with the chat link, so the app's deep-link handling opens the room. */
    fun openChatIntent(context: Context, roomId: String): Intent {
        val intent = context.packageManager.getLaunchIntentForPackage(context.packageName) ?: Intent()
        return intent
            .setAction(Intent.ACTION_VIEW)
            .setData(Uri.parse("$SCHEME://chat/$roomId"))
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP)
    }
}
