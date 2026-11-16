package app.chirper.platform

import app.tauri.plugin.Channel

/**
 * Carries inline replies from the notification receiver to the Rust side. The Rust side registers a
 * channel once at startup. Replies that arrive before that are kept and delivered with the channel.
 * They are kept in memory only: if the process has been killed, the reply is lost.
 */
internal object ReplySink {
    private val lock = Any()
    private var channel: Channel? = null
    private val pending = ArrayDeque<Pair<String, String>>()

    /** Registers the channel and delivers the replies that were waiting for it. */
    fun attach(newChannel: Channel) {
        val waiting = synchronized(lock) {
            channel = newChannel
            pending.toList().also { pending.clear() }
        }
        waiting.forEach { (roomId, text) -> send(newChannel, roomId, text) }
    }

    /** Returns false when no channel is attached yet, and then keeps the reply for later. */
    fun deliver(roomId: String, text: String): Boolean {
        val current = synchronized(lock) {
            channel ?: run {
                pending.addLast(roomId to text)
                return false
            }
        }
        send(current, roomId, text)
        return true
    }

    private fun send(target: Channel, roomId: String, text: String) {
        target.sendObject(mapOf("roomId" to roomId, "text" to text))
    }
}
