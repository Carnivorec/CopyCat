package app.copycat.daemon

import android.content.AttributionSource
import android.os.Binder
import android.os.Bundle
import android.os.IBinder
import android.os.Parcel
import android.os.ParcelFileDescriptor
import android.os.Process
import java.io.File
import java.io.FileOutputStream
import java.util.concurrent.CopyOnWriteArrayList

class IpcBinder(private val uiUid: Int, private val dataDir: String) : Binder() {
    private val callbacks = CopyOnWriteArrayList<IBinder>()

    init {
        attachInterface(null, Protocol.DESCRIPTOR)
    }

    override fun onTransact(code: Int, data: Parcel, reply: Parcel?, flags: Int): Boolean {
        if (code == INTERFACE_TRANSACTION) return super.onTransact(code, data, reply, flags)
        if (getCallingUid() != uiUid) {
            Dlog.p("拒绝 uid ${getCallingUid()} 的调用（App uid 为 $uiUid）")
            reply?.writeException(SecurityException("仅允许 CopyCat 应用调用"))
            return true
        }
        when (code) {
            Protocol.TX_REQUEST -> {
                data.enforceInterface(Protocol.DESCRIPTOR)
                val req = data.readString() ?: "{}"
                val resp = Native.request(req)
                reply?.writeNoException()
                reply?.writeString(resp)
            }
            Protocol.TX_SUBSCRIBE -> {
                data.enforceInterface(Protocol.DESCRIPTOR)
                val cb = data.readStrongBinder()
                if (cb != null) {
                    callbacks.add(cb)
                    try {
                        cb.linkToDeath({ callbacks.remove(cb) }, 0)
                    } catch (e: Throwable) {
                        callbacks.remove(cb)
                    }
                }
                reply?.writeNoException()
            }
            Protocol.TX_OPEN_FILE -> {
                data.enforceInterface(Protocol.DESCRIPTOR)
                val f = receivedFile(data.readString())
                if (f == null) {
                    reply?.writeException(SecurityException("只能打开收到的文件"))
                    return true
                }
                val pfd = try {
                    ParcelFileDescriptor.open(f, ParcelFileDescriptor.MODE_READ_ONLY)
                } catch (e: Exception) {
                    reply?.writeException(IllegalStateException("无法打开：${e.message}"))
                    return true
                }
                reply?.writeNoException()
                reply?.writeLong(f.length())

                if (reply != null) pfd.writeToParcel(reply, android.os.Parcelable.PARCELABLE_WRITE_RETURN_VALUE) else pfd.close()
            }
            Protocol.TX_SEND_FILES -> {
                data.enforceInterface(Protocol.DESCRIPTOR)
                val device = data.readString() ?: ""
                val n = data.readInt().coerceIn(0, 10_000)
                val names = Array(n) { data.readString() ?: "未命名" }
                val fds = IntArray(n) { regularFd(data.readFileDescriptor()) }
                val resp = Native.sendFiles(device, names, fds)
                reply?.writeNoException()
                reply?.writeString(resp)
            }
            else -> return super.onTransact(code, data, reply, flags)
        }
        return true
    }

    private fun regularFd(pfd: ParcelFileDescriptor?): Int {
        pfd ?: return -1
        if (pfd.statSize >= 0) return pfd.detachFd()
        return try {
            val dir = File(dataDir, "spool").apply { mkdirs() }
            val tmp = File.createTempFile("share", ".tmp", dir)
            try {
                ParcelFileDescriptor.AutoCloseInputStream(pfd).use { input -> FileOutputStream(tmp).use { input.copyTo(it) } }
                ParcelFileDescriptor.open(tmp, ParcelFileDescriptor.MODE_READ_ONLY).detachFd()
            } finally {
                tmp.delete()
            }
        } catch (e: Throwable) {
            Dlog.p("读取分享的内容失败: $e")
            -1
        }
    }

    private fun receivedFile(path: String?): File? {
        path ?: return null
        val root = File(Protocol.RECEIVE_ROOT).canonicalFile
        val f = try {
            File(path).canonicalFile
        } catch (e: Exception) {
            return null
        }
        return f.takeIf { it.path.startsWith(root.path + File.separator) && it.isFile }
    }

    fun broadcast(json: String) {
        for (cb in callbacks) {
            val p = Parcel.obtain()
            try {
                p.writeInterfaceToken(Protocol.CALLBACK_DESCRIPTOR)
                p.writeString(json)
                cb.transact(Protocol.TX_EVENT, p, null, FLAG_ONEWAY)
            } catch (e: Throwable) {
                callbacks.remove(cb)
            } finally {
                p.recycle()
            }
        }
    }

    fun attachToApp(): Boolean = callApp(Protocol.METHOD_ATTACH, Bundle().apply { putBinder(Protocol.EXTRA_BINDER, this@IpcBinder) })

    fun notifyTransfer(json: String): Boolean = callApp(Protocol.METHOD_TRANSFER, Bundle().apply { putString(Protocol.EXTRA_JSON, json) })

    fun notifyRemoved(paths: Array<String>): Boolean = callApp(Protocol.METHOD_REMOVED, Bundle().apply { putStringArray(Protocol.EXTRA_PATHS, paths) })

    private fun callApp(method: String, extras: Bundle): Boolean {
        val token = Binder()
        val userId = Process.myUid() / 100000
        var am: Any? = null
        return try {
            val sm = Class.forName("android.os.ServiceManager")
            val b = sm.getMethod("getService", String::class.java).invoke(null, "activity") as IBinder
            am = Class.forName("android.app.IActivityManager\$Stub").getMethod("asInterface", IBinder::class.java).invoke(null, b)!!
            val get = am.javaClass.methods.first { it.name == "getContentProviderExternal" }
            val holder = get.invoke(am, Protocol.AUTHORITY, userId, token, "copycat") ?: return false
            val provider = holder.javaClass.getField("provider").get(holder) ?: return false
            val str = String::class.java
            val bundle = Bundle::class.java
            val methods = provider.javaClass.methods.filter { it.name == "call" }

            val (call, args) = methods.firstNotNullOfOrNull { m ->
                val shell = AttributionSource.Builder(Process.myUid()).setPackageName("com.android.shell").build()
                when (m.parameterTypes.toList()) {
                    listOf(AttributionSource::class.java, str, str, str, bundle) ->
                        m to listOf(shell, Protocol.AUTHORITY, method, null, extras)
                    listOf(str, str, str, str, str, bundle) ->
                        m to listOf("com.android.shell", null, Protocol.AUTHORITY, method, null, extras)
                    listOf(str, str, str, str, bundle) ->
                        m to listOf("com.android.shell", Protocol.AUTHORITY, method, null, extras)
                    else -> null
                }
            } ?: throw NoSuchMethodException("IContentProvider.call: " + methods.joinToString { m -> m.parameterTypes.joinToString(",") { it.simpleName } })
            call.invoke(provider, *args.toTypedArray())
            if (method == Protocol.METHOD_ATTACH) Dlog.p("已把 binder 交给 App（${call.parameterTypes.joinToString { it.simpleName }}）")
            true
        } catch (e: Throwable) {
            Dlog.p("调用 App 失败（$method）: ${e.cause ?: e}")
            false
        } finally {
            try {
                am?.javaClass?.methods?.firstOrNull { it.name == "removeContentProviderExternalAsUser" }
                    ?.invoke(am, Protocol.AUTHORITY, token, userId)
                    ?: am?.javaClass?.methods?.firstOrNull { it.name == "removeContentProviderExternal" }
                        ?.invoke(am, Protocol.AUTHORITY, token)
            } catch (ignored: Throwable) {
            }
        }
    }

    companion object {
    }
}
