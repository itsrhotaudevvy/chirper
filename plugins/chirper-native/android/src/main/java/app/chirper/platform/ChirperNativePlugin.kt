package app.chirper.platform

import android.Manifest
import android.app.Activity
import android.app.Dialog
import android.content.pm.PackageManager
import android.graphics.Color
import android.graphics.Typeface
import android.net.Uri
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.text.TextUtils
import android.view.Gravity
import android.view.View
import android.view.WindowManager
import android.webkit.CookieManager
import android.webkit.WebChromeClient
import android.webkit.WebResourceRequest
import android.webkit.WebSettings
import android.webkit.WebView
import android.webkit.WebViewClient
import android.webkit.SslErrorHandler
import android.net.http.SslError
import android.os.Message
import android.widget.LinearLayout
import android.widget.ProgressBar
import android.widget.TextView
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

@InvokeArg
class LoginWebviewArgs {
    var url: String = ""
    var extractJs: String = ""
    var waitForPattern: String = ""
    var userAgent: String = ""
    var title: String = ""
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

    @Command
    fun openLoginWebview(invoke: Invoke) {
        val args = invoke.parseArgs(LoginWebviewArgs::class.java)
        activity.runOnUiThread {
            showLoginDialog(args, invoke)
        }
    }

    private fun showLoginDialog(args: LoginWebviewArgs, invoke: Invoke) {
        val dialog = Dialog(activity, android.R.style.Theme_DeviceDefault_NoActionBar_Fullscreen)
        dialog.window?.let { win ->
            win.setLayout(
                WindowManager.LayoutParams.MATCH_PARENT,
                WindowManager.LayoutParams.MATCH_PARENT,
            )
            win.setSoftInputMode(WindowManager.LayoutParams.SOFT_INPUT_ADJUST_RESIZE)
        }

        var finished = false
        val handler = Handler(Looper.getMainLooper())

        val root = LinearLayout(activity).apply {
            orientation = LinearLayout.VERTICAL
            setBackgroundColor(Color.parseColor("#0E1517"))
            layoutParams = LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT,
                LinearLayout.LayoutParams.MATCH_PARENT,
            )
        }

        val topBar = LinearLayout(activity).apply {
            orientation = LinearLayout.HORIZONTAL
            setBackgroundColor(Color.parseColor("#161F22"))
            gravity = Gravity.CENTER_VERTICAL
            val h = (56 * activity.resources.displayMetrics.density).toInt()
            layoutParams = LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT,
                h,
            )
            val padH = (16 * activity.resources.displayMetrics.density).toInt()
            setPadding(padH, 0, padH, 0)
        }

        val cancelBtn = TextView(activity).apply {
            text = "Cancel"
            setTextColor(Color.parseColor("#8A6FC9"))
            textSize = 16f
            typeface = Typeface.DEFAULT_BOLD
            isClickable = true
            isFocusable = true
        }

        val titleView = TextView(activity).apply {
            text = if (args.title.isNotEmpty()) args.title else "Sign In"
            setTextColor(Color.WHITE)
            textSize = 17f
            typeface = Typeface.DEFAULT_BOLD
            gravity = Gravity.CENTER
            layoutParams = LinearLayout.LayoutParams(
                0,
                LinearLayout.LayoutParams.WRAP_CONTENT,
                1f,
            )
            maxLines = 1
            ellipsize = TextUtils.TruncateAt.END
        }

        val spacer = TextView(activity).apply {
            text = "Cancel"
            textSize = 16f
            visibility = View.INVISIBLE
        }

        topBar.addView(cancelBtn)
        topBar.addView(titleView)
        topBar.addView(spacer)
        root.addView(topBar)

        val progressBar = ProgressBar(
            activity,
            null,
            android.R.attr.progressBarStyleHorizontal,
        ).apply {
            val h = (3 * activity.resources.displayMetrics.density).toInt()
            layoutParams = LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT,
                h,
            )
            max = 100
            progress = 0
            visibility = View.GONE
        }
        root.addView(progressBar)

        val webView = WebView(activity).apply {
            layoutParams = LinearLayout.LayoutParams(
                LinearLayout.LayoutParams.MATCH_PARENT,
                0,
                1f,
            )
            setBackgroundColor(Color.parseColor("#0E1517"))
        }

        val settings = webView.settings
        settings.javaScriptEnabled = true
        settings.domStorageEnabled = true
        settings.databaseEnabled = true
        settings.setSupportZoom(true)
        settings.builtInZoomControls = true
        settings.displayZoomControls = false
        settings.useWideViewPort = true
        settings.loadWithOverviewMode = true

        settings.mixedContentMode = WebSettings.MIXED_CONTENT_ALWAYS_ALLOW
        settings.javaScriptCanOpenWindowsAutomatically = true
        settings.allowFileAccess = true
        settings.allowContentAccess = true
        settings.mediaPlaybackRequiresUserGesture = false
        settings.setSupportMultipleWindows(true)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            settings.safeBrowsingEnabled = false
        }

        val defaultDesktopUA =
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36"
        if (args.userAgent.isNotEmpty()) {
            settings.userAgentString = args.userAgent
        } else {
            settings.userAgentString = defaultDesktopUA
        }

        val cookieManager = CookieManager.getInstance()
        cookieManager.setAcceptCookie(true)
        cookieManager.setAcceptThirdPartyCookies(webView, true)

        val customJs = if (args.extractJs.isNotEmpty()) {
            "try { var c = ${args.extractJs}; if (c && typeof c === 'string' && c.length > 20) return c; } catch(e) {}"
        } else ""

        val extractScript = """
            (function() {
                $customJs
                try {
                    var raw = localStorage.getItem('token');
                    if (raw) {
                        var v = raw;
                        try { v = JSON.parse(raw); } catch(e) {}
                        if (typeof v === 'string' && v.length > 20) return v;
                    }
                } catch(e) {}
                try {
                    var m = [];
                    window.webpackChunkdiscord_app?.push([['chirper_' + Date.now()], {}, function(e) {
                        for (var c in e.c) m.push(e.c[c]);
                    }]);
                    for (var i = 0; i < m.length; i++) {
                        var mod = m[i]?.exports?.default;
                        if (mod && typeof mod.getToken === 'function') {
                            var t = mod.getToken();
                            if (typeof t === 'string' && t.length > 20) return t;
                        }
                    }
                } catch(e) {}
                return null;
            })()
        """.trimIndent()

        fun finishWith(token: String?, cancelled: Boolean) {
            if (finished) return
            finished = true
            activity.runOnUiThread {
                try {
                    dialog.dismiss()
                    webView.destroy()
                } catch (_: Exception) {}
            }
            val res = JSObject()
            if (token != null) {
                res.put("token", token)
                res.put("cancelled", false)
            } else {
                res.put("cancelled", cancelled)
            }
            invoke.resolve(res)
        }

        val pollRunnable = object : Runnable {
            override fun run() {
                if (!finished && dialog.isShowing) {
                    webView.evaluateJavascript(extractScript) { result ->
                        val clean = result?.trim('"', ' ', '\n', '\r', '\\')
                        if (!clean.isNullOrEmpty() && clean != "null" && clean.length > 20) {
                            finishWith(clean, cancelled = false)
                        }
                    }
                    handler.postDelayed(this, 500)
                }
            }
        }

        webView.webChromeClient = object : WebChromeClient() {
            override fun onProgressChanged(view: WebView?, newProgress: Int) {
                if (newProgress < 100) {
                    progressBar.visibility = View.VISIBLE
                    progressBar.progress = newProgress
                } else {
                    progressBar.visibility = View.GONE
                }
            }

            override fun onCreateWindow(
                view: WebView?,
                isDialog: Boolean,
                isUserGesture: Boolean,
                resultMsg: Message?
            ): Boolean {
                val transport = resultMsg?.obj as? WebView.WebViewTransport
                transport?.webView = view
                resultMsg?.sendToTarget()
                return true
            }
        }

        webView.webViewClient = object : WebViewClient() {
            override fun onReceivedSslError(view: WebView?, handler: SslErrorHandler?, error: SslError?) {
                handler?.proceed()
            }

            override fun shouldOverrideUrlLoading(
                view: WebView?,
                request: WebResourceRequest?,
            ): Boolean {
                val reqUrl = request?.url?.toString() ?: return false
                if (reqUrl.startsWith("https://chirper-login.local/") || reqUrl.startsWith("chirper-login://")) {
                    val tok = request.url?.getQueryParameter("token")
                    if (!tok.isNullOrEmpty()) {
                        finishWith(tok, cancelled = false)
                        return true
                    }
                }
                return false
            }

            override fun onPageFinished(view: WebView?, url: String?) {
                super.onPageFinished(view, url)
                webView.evaluateJavascript(extractScript) { result ->
                    val clean = result?.trim('"', ' ', '\n', '\r', '\\')
                    if (!clean.isNullOrEmpty() && clean != "null" && clean.length > 20) {
                        finishWith(clean, cancelled = false)
                    }
                }
            }
        }

        cancelBtn.setOnClickListener {
            finishWith(null, cancelled = true)
        }

        dialog.setOnCancelListener {
            finishWith(null, cancelled = true)
        }

        dialog.setContentView(root)
        dialog.show()
        handler.postDelayed(pollRunnable, 800)
        webView.loadUrl(args.url)
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
