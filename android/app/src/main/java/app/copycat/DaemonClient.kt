package app.copycat

import android.content.Context
import android.os.Binder
import android.os.IBinder
import android.os.Parcel
import app.copycat.daemon.Protocol
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.channels.BufferOverflow
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeoutOrNull
import org.json.JSONObject
import java.io.File

object DaemonClient {
    private val binder = MutableStateFlow<IBinder?>(null)
    val connected: StateFlow<IBinder?> = binder

    private val _events = MutableSharedFlow<JSONObject>(extraBufferCapacity = 64, onBufferOverflow = BufferOverflow.DROP_OLDEST)
    val events: SharedFlow<JSONObject> = _events

    private val callback = object : Binder() {
        override fun onTransact(code: Int, data: Parcel, reply: Parcel?, flags: Int): Boolean {
            if (code != Protocol.TX_EVENT) return super.onTransact(code, data, reply, flags)
            data.enforceInterface(Protocol.CALLBACK_DESCRIPTOR)
            data.readString()?.let { runCatching { _events.tryEmit(JSONObject(it)) } }
            return true
        }
    }

    fun onAttached(b: IBinder) {
        try {
            b.linkToDeath({ binder.compareAndSet(b, null) }, 0)
            val p = Parcel.obtain()
            val r = Parcel.obtain()
            try {
                p.writeInterfaceToken(Protocol.DESCRIPTOR)
                p.writeStrongBinder(callback)
                b.transact(Protocol.TX_SUBSCRIBE, p, r, 0)
                r.readException()
            } finally {
                p.recycle()
                r.recycle()
            }
            binder.value = b
        } catch (e: Exception) {
            binder.value = null
        }
    }

    sealed interface Ensure {
        data object Ok : Ensure
        data object NoRoot : Ensure
        data class Failed(val reason: String) : Ensure
    }

    suspend fun ensure(ctx: Context): Ensure {
        if (binder.value?.isBinderAlive == true) return Ensure.Ok
        if (!Root.available()) return Ensure.NoRoot
        val script = File(ctx.filesDir, "copycatd.sh")
        withContext(Dispatchers.IO) {
            ctx.assets.open("copycatd.sh").use { input -> script.outputStream().use { input.copyTo(it) } }
        }
        val dst = "${Protocol.ROOT_DIR}/copycatd.sh"
        val r = Root.run(
            "mkdir -p ${Protocol.ROOT_DIR} && chmod 700 ${Protocol.ROOT_DIR} && " +
                "cp '${script.absolutePath}' $dst.new && chmod 700 $dst.new && mv -f $dst.new $dst && sh $dst attach"
        )
        if (r.code != 0) return Ensure.Failed(r.out.ifEmpty { "启动脚本执行失败" })
        val ok = withTimeoutOrNull(15_000) { binder.first { it?.isBinderAlive == true } }
        return if (ok != null) Ensure.Ok else Ensure.Failed("守护进程没有响应，请查看 ${Protocol.DATA_DIR}/daemon.log")
    }

    suspend fun request(cmd: String, vararg args: Pair<String, Any?>): Any? = withContext(Dispatchers.IO) {
        val b = binder.value ?: throw DaemonException("守护进程未连接")
        val req = JSONObject().put("cmd", cmd)
        for ((k, v) in args) req.put(k, v ?: JSONObject.NULL)
        val p = Parcel.obtain()
        val r = Parcel.obtain()
        val resp = try {
            p.writeInterfaceToken(Protocol.DESCRIPTOR)
            p.writeString(req.toString())
            b.transact(Protocol.TX_REQUEST, p, r, 0)
            r.readException()
            r.readString() ?: "{}"
        } catch (e: Exception) {
            throw DaemonException("与守护进程通信失败：${e.message}")
        } finally {
            p.recycle()
            r.recycle()
        }
        val j = JSONObject(resp)
        if (j.has("err")) throw DaemonException(j.getString("err"))
        if (j.isNull("ok")) null else j.get("ok")
    }

    suspend fun sendFiles(device: String, files: List<Pair<String, android.os.ParcelFileDescriptor>>): String = withContext(Dispatchers.IO) {
        val b = binder.value ?: throw DaemonException("守护进程未连接")
        val p = Parcel.obtain()
        val r = Parcel.obtain()
        val resp = try {
            p.writeInterfaceToken(Protocol.DESCRIPTOR)
            p.writeString(device)
            p.writeInt(files.size)
            files.forEach { p.writeString(it.first) }
            files.forEach { p.writeFileDescriptor(it.second.fileDescriptor) }
            b.transact(Protocol.TX_SEND_FILES, p, r, 0)
            r.readException()
            r.readString() ?: "{}"
        } catch (e: Exception) {
            throw DaemonException("与守护进程通信失败：${e.message}")
        } finally {
            p.recycle()
            r.recycle()

            files.forEach { runCatching { it.second.close() } }
        }
        val j = JSONObject(resp)
        if (j.has("err")) throw DaemonException(j.getString("err"))
        j.getString("ok")
    }

    suspend fun openFile(path: String): Pair<Long, android.os.ParcelFileDescriptor> = withContext(Dispatchers.IO) {
        val b = binder.value ?: throw DaemonException("守护进程未连接")
        val p = Parcel.obtain()
        val r = Parcel.obtain()
        try {
            p.writeInterfaceToken(Protocol.DESCRIPTOR)
            p.writeString(path)
            b.transact(Protocol.TX_OPEN_FILE, p, r, 0)
            r.readException()
            val size = r.readLong()
            size to android.os.ParcelFileDescriptor.CREATOR.createFromParcel(r)
        } catch (e: DaemonException) {
            throw e
        } catch (e: Exception) {
            throw DaemonException(e.message ?: "无法打开文件")
        } finally {
            p.recycle()
            r.recycle()
        }
    }

    suspend fun root(cmd: String) = Root.run("sh ${Protocol.ROOT_DIR}/copycatd.sh $cmd")
}

class DaemonException(msg: String) : Exception(msg)
