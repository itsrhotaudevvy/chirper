package app.chirper.platform

import android.content.Context
import android.net.Uri
import android.provider.OpenableColumns
import java.io.File
import java.io.IOException
import java.util.UUID

/**
 * Copies a file chosen in the system picker (a `content://` URI) into the app cache, so that the
 * Go core can read it by path. Each copy has its own directory, so the file keeps its name. Copies
 * older than an hour are removed the next time a file is picked.
 */
internal object ContentCopy {
    private const val DIRECTORY = "picked"
    private const val MAX_AGE_MS = 60 * 60 * 1000L
    private const val BUFFER_BYTES = 64 * 1024

    class Copied(val path: String, val size: Long)

    fun copy(context: Context, uri: Uri, maxBytes: Long): Copied {
        if (uri.scheme != "content") throw IOException("not a content URI")
        val name = displayName(context, uri)
        val root = File(context.cacheDir, DIRECTORY).apply { mkdirs() }
        removeOld(root)
        val dir = File(root, UUID.randomUUID().toString()).apply { mkdirs() }
        val target = File(dir, name)
        var total = 0L
        try {
            val input = context.contentResolver.openInputStream(uri)
                ?: throw IOException("the picked file cannot be opened")
            input.use { source ->
                target.outputStream().use { sink ->
                    val buffer = ByteArray(BUFFER_BYTES)
                    while (true) {
                        val read = source.read(buffer)
                        if (read < 0) break
                        total += read
                        if (total > maxBytes) throw IOException("the file is larger than the allowed size")
                        sink.write(buffer, 0, read)
                    }
                }
            }
        } catch (e: Exception) {
            dir.deleteRecursively()
            throw e
        }
        return Copied(target.absolutePath, total)
    }

    private fun displayName(context: Context, uri: Uri): String {
        val cursor = context.contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)
        cursor?.use { c ->
            val column = c.getColumnIndex(OpenableColumns.DISPLAY_NAME)
            if (c.moveToFirst() && column >= 0) {
                c.getString(column)?.let { return sanitize(it) }
            }
        }
        return "file"
    }

    /** Removes path separators, control characters and leading dots, and bounds the length. */
    private fun sanitize(name: String): String =
        name.replace(Regex("[\\\\/:*?\"<>|\\p{Cntrl}]"), "_")
            .trimStart('.')
            .take(MAX_NAME_CHARS)
            .ifEmpty { "file" }

    private fun removeOld(root: File) {
        val cutoff = System.currentTimeMillis() - MAX_AGE_MS
        root.listFiles()?.forEach { entry ->
            if (entry.lastModified() < cutoff) entry.deleteRecursively()
        }
    }

    private const val MAX_NAME_CHARS = 128
}
