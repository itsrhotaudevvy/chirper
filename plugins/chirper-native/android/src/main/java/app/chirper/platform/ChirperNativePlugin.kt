package app.chirper.platform

import android.Manifest
import android.app.Activity
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.view.WindowManager
import androidx.core.content.ContextCompat
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.Permission
import app.tauri.annotation.PermissionCallback
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Channel
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import java.util.Base64

@InvokeArg
class PlaintextArgs {
    var plaintext: String = ""
}

@InvokeArg
class UnwrapArgs {
    var iv: String = ""
    var ciphertext: String = ""
}

@InvokeArg
class EnabledArgs {
    var enabled: Boolean = false
}

@InvokeArg
class RunningArgs {
    var running: Boolean = false
}

@InvokeArg
class MessageArgs {
    var roomId: String = ""
    var title: String = ""
    var body: String = ""
    var hideContent: Boolean = false
}

@InvokeArg
class ReplyChannelArgs {
    lateinit var handler: Channel
}

@InvokeArg
class CopyArgs {
    var uri: String = ""
    var maxBytes: Long = 0
}

/**
 * The native commands the Rust side of Chirper calls. Commands resolve without waiting for the main
 * thread, because the Rust caller may be blocking the main thread while it waits for the answer.
 */
@TauriPlugin(
    permissions = [Permission(strings = [Manifest.permission.POST_NOTIFICATIONS], alias = "notifications")],
)
class ChirperNativePlugin(private val activity: Activity) : Plugin(activity) {

    @Command
    fun formFactor(invoke: Invoke) {
        val tablet = activity.resources.configuration.smallestScreenWidthDp >= TABLET_MIN_DP
        invoke.resolve(JSObject().put("formFactor", if (tablet) "tablet" else "phone"))
    }

    @Command
    fun wrapKey(invoke: Invoke) {
        val args = invoke.parseArgs(PlaintextArgs::class.java)
        val plaintext = Base64.getDecoder().decode(args.plaintext)
        try {
            val (iv, ciphertext) = DbKeyWrap.wrap(plaintext)
            invoke.resolve(
                JSObject()
                    .put("iv", Base64.getEncoder().encodeToString(iv))
                    .put("ciphertext", Base64.getEncoder().encodeToString(ciphertext)),
            )
        } catch (e: Exception) {
            invoke.reject("cannot wrap the key: ${e.javaClass.simpleName}")
        } finally {
            plaintext.fill(0)
        }
    }

    @Command
    fun unwrapKey(invoke: Invoke) {
        val args = invoke.parseArgs(UnwrapArgs::class.java)
        try {
            val plaintext = DbKeyWrap.unwrap(
                Base64.getDecoder().decode(args.iv),
                Base64.getDecoder().decode(args.ciphertext),
            )
            try {
                invoke.resolve(JSObject().put("plaintext", Base64.getEncoder().encodeToString(plaintext)))
            } finally {
                plaintext.fill(0)
            }
        } catch (e: Exception) {
            invoke.reject("cannot unwrap the key: ${e.javaClass.simpleName}")
        }
    }

    @Command
    fun noBackupDir(invoke: Invoke) {
        invoke.resolve(JSObject().put("path", activity.noBackupFilesDir.absolutePath))
    }

    @Command
    fun setSecureScreen(invoke: Invoke) {
        val enabled = invoke.parseArgs(EnabledArgs::class.java).enabled
        activity.runOnUiThread {
            if (enabled) {
                activity.window.addFlags(WindowManager.LayoutParams.FLAG_SECURE)
            } else {
                activity.window.clearFlags(WindowManager.LayoutParams.FLAG_SECURE)
            }
        }
        invoke.resolve()
    }

    @Command
    fun setConnectionService(invoke: Invoke) {
        val running = invoke.parseArgs(RunningArgs::class.java).running
        try {
            if (running) ConnectionService.start(activity) else ConnectionService.stop(activity)
            invoke.resolve()
        } catch (e: Exception) {
            invoke.reject("cannot change the connection service: ${e.javaClass.simpleName}")
        }
    }

    @Command
    fun showMessage(invoke: Invoke) {
        val args = invoke.parseArgs(MessageArgs::class.java)
        if (notificationPermissionMissing()) {
            invoke.resolve(outcome(posted = false, permissionMissing = true))
            return
        }
        val posted = Notices.post(activity, args.roomId, args.title, args.body, args.hideContent)
        invoke.resolve(outcome(posted = posted, permissionMissing = false))
    }

    /** Asks for the notification permission (Android 13 and later). The answer comes in a callback. */
    @Command
    fun requestNotificationPermission(invoke: Invoke) {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU) {
            invoke.resolve(JSObject().put("granted", true))
            return
        }
        requestPermissionForAliases(arrayOf("notifications"), invoke, "onNotificationPermission")
    }

    @PermissionCallback
    fun onNotificationPermission(invoke: Invoke) {
        invoke.resolve(JSObject().put("granted", !notificationPermissionMissing()))
    }

    @Command
    fun setReplyChannel(invoke: Invoke) {
        ReplySink.attach(invoke.parseArgs(ReplyChannelArgs::class.java).handler)
        invoke.resolve()
    }

    @Command
    fun copyContentUri(invoke: Invoke) {
        val args = invoke.parseArgs(CopyArgs::class.java)
        try {
            val copied = ContentCopy.copy(activity, Uri.parse(args.uri), args.maxBytes)
            invoke.resolve(JSObject().put("path", copied.path).put("size", copied.size))
        } catch (e: Exception) {
            invoke.reject("cannot copy the picked file: ${e.message ?: e.javaClass.simpleName}")
        }
    }

    private fun notificationPermissionMissing(): Boolean =
        Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU &&
            ContextCompat.checkSelfPermission(activity, Manifest.permission.POST_NOTIFICATIONS) !=
            PackageManager.PERMISSION_GRANTED

    private fun outcome(posted: Boolean, permissionMissing: Boolean): JSObject =
        JSObject().put("posted", posted).put("permissionMissing", permissionMissing)

    private companion object {
        const val TABLET_MIN_DP = 600
    }
}
