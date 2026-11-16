package app.chirper.platform

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import androidx.core.app.NotificationManagerCompat
import androidx.core.app.RemoteInput

/** Receives the inline reply typed in a message notification and hands it to the app. */
class ReplyReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action != Notices.ACTION_REPLY) return
        val roomId = intent.getStringExtra(Notices.EXTRA_ROOM_ID) ?: return
        // The text is never logged: it is a message body.
        val text = RemoteInput.getResultsFromIntent(intent)
            ?.getCharSequence(Notices.REPLY_KEY)
            ?.toString()
            ?.trim()
            ?: return
        if (text.isEmpty() || text.length > MAX_REPLY_CHARS) return
        NotificationManagerCompat.from(context).cancel(Notices.notificationId(roomId))
        if (!ReplySink.deliver(roomId, text)) {
            // The app is not running yet: open the chat so the user can resend if the reply was lost.
            context.startActivity(Notices.openChatIntent(context, roomId))
        }
    }

    private companion object {
        const val MAX_REPLY_CHARS = 10_000
    }
}
