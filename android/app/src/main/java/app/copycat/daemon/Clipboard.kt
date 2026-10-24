package app.copycat.daemon

import android.content.ClipData
import android.content.ClipDescription
import android.os.Binder
import android.os.Handler
import android.os.HandlerThread
import android.os.IBinder
import android.os.Parcel
import android.os.PersistableBundle
import android.os.Process
import java.lang.reflect.Method

class Clipboard(private val onLocal: (text: String?, html: String?, source: String?) -> Unit) {
    private val clipboard: Any
    private val methods: Map<String, Method>
    private val wm: Any
    private val userId = Process.myUid() / 100000
    private val thread = HandlerThread("clipboard").apply { start() }
    private val handler = Handler(thread.looper)
    private var pendingDelay = 0L

    init {
        val sm = Class.forName("android.os.ServiceManager")
        val b = sm.getMethod("getService", String::class.java).invoke(null, "clipboard") as IBinder
        clipboard = Class.forName("android.content.IClipboard\$Stub").getMethod("asInterface", IBinder::class.java).invoke(null, b)!!
        methods = clipboard.javaClass.methods.associateBy { it.name }
        wm = Class.forName("android.view.WindowManagerGlobal").getMethod("getWindowManagerService").invoke(null)!!
    }

    private fun call(name: String, vararg head: Any?): Any? {
        val m = methods[name] ?: throw NoSuchMethodException(name)
        var h = 0
        var s = 0
        var n = 0
        val args = m.parameterTypes.map { t ->
            when {
                h < head.size && !t.isPrimitive && t != String::class.java -> head[h++]
                t == String::class.java -> if (s++ == 0) "com.android.shell" else null
                t == Int::class.javaPrimitiveType -> if (n++ == 0) userId else 0
                t == Boolean::class.javaPrimitiveType -> false
                else -> null
            }
        }
        return m.invoke(clipboard, *args.toTypedArray())
    }

    private fun locked(): Boolean = try {
        wm.javaClass.getMethod("isKeyguardLocked").invoke(wm) as Boolean
    } catch (e: Throwable) {
        false
    }

    fun listen() {
        val listener = object : Binder() {
            override fun onTransact(code: Int, data: Parcel, reply: Parcel?, flags: Int): Boolean {
                if (code != FIRST_CALL_TRANSACTION) return super.onTransact(code, data, reply, flags)
                data.enforceInterface(LISTENER)
                handler.post { onChanged() }
                return true
            }
        }
        listener.attachInterface(null, LISTENER)
        val proxy = Class.forName("$LISTENER\$Stub").getMethod("asInterface", IBinder::class.java).invoke(null, listener)
        call("addPrimaryClipChangedListener", proxy)
    }

    private val retry = Runnable { onChanged() }

    private fun onChanged() {
        handler.removeCallbacks(retry)
        if (locked()) {
            pendingDelay = (pendingDelay * 2).coerceIn(2_000, 60_000)
            handler.postDelayed(retry, pendingDelay)
            return
        }
        pendingDelay = 0
        try {
            val clip = call("getPrimaryClip") as ClipData? ?: return
            val extras = clip.description.extras
            if (extras?.getBoolean(Protocol.CLIP_MARK) == true) return
            if (extras?.getBoolean(ClipDescription.EXTRA_IS_SENSITIVE) == true) return
            if (clip.itemCount == 0) return
            val item = clip.getItemAt(0)
            val text = item.text?.toString()
            val html = item.htmlText
            if (text.isNullOrEmpty() && html.isNullOrEmpty()) return
            val source = try { call("getPrimaryClipSource") as String? } catch (e: Throwable) { null }
            onLocal(text, html, source)
        } catch (e: Throwable) {
            Dlog.p("读取剪贴板失败: ${e.cause ?: e}")
        }
    }

    fun write(text: String, html: String?, mark: Boolean) {
        handler.post {
            try {
                val clip = if (html != null) ClipData.newHtmlText("CopyCat", text, html) else ClipData.newPlainText("CopyCat", text)
                if (mark) clip.description.extras = PersistableBundle().apply { putBoolean(Protocol.CLIP_MARK, true) }
                call("setPrimaryClip", clip)
            } catch (e: Throwable) {
                Dlog.p("写入剪贴板失败: ${e.cause ?: e}")
            }
        }
    }

    companion object {
        private const val LISTENER = "android.content.IOnPrimaryClipChangedListener"
    }
}
