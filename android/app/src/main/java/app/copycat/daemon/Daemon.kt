package app.copycat.daemon

import android.os.Build
import android.os.Looper
import org.json.JSONObject
import java.io.File
import kotlin.concurrent.thread
import kotlin.system.exitProcess

object Daemon {

    @JvmStatic
    fun main(args: Array<String>) {
        if (args.size < 3) {
            Dlog.p("用法: Daemon <数据目录> <库目录> <App uid>")
            exitProcess(2)
        }
        val (dataDir, libDir, uid) = args
        @Suppress("DEPRECATION")
        Looper.prepareMainLooper()

        val key = System.`in`.readNBytes(32)
        System.`in`.close()
        if (key.size != 32) {
            Dlog.p("未收到主密钥")
            exitProcess(2)
        }
        System.load("$libDir/libcopycat.so")
        val err = Native.start(dataDir, key, deviceName())
        key.fill(0)
        if (err != null) {
            Dlog.p("启动失败: $err")
            exitProcess(1)
        }
        File(dataDir, "daemon.pid").writeText(android.os.Process.myPid().toString())

        val ipc = IpcBinder(uid.toInt(), dataDir)
        val clipboard = Clipboard { text, html, source -> Native.localClip(text, html, source) }
        clipboard.listen()
        Runtime.getRuntime().addShutdownHook(Thread { Native.stop() })

        thread(name = "actions") {
            while (true) {
                val json = Native.nextAction() ?: break
                try {
                    val a = JSONObject(json)
                    when (a.getString("type")) {
                        "write" -> clipboard.write(a.getString("text"), a.optString("html").takeIf { !a.isNull("html") }, a.getBoolean("mark"))
                        "event" -> ipc.broadcast(a.getString("json"))
                        "attach" -> ipc.attachToApp()
                        "finished" -> ipc.notifyTransfer(a.getString("json"))
                        "removed" -> a.getJSONArray("paths").let { p -> ipc.notifyRemoved(Array(p.length()) { p.getString(it) }) }
                        "exit" -> break
                    }
                } catch (e: Throwable) {
                    Dlog.p("处理动作失败: $e")
                }
            }
            exitProcess(0)
        }
        ipc.attachToApp()
        Dlog.p("守护进程已就绪 pid=${android.os.Process.myPid()}")
        Looper.loop()
    }

    private fun deviceName(): String {
        val fromSettings = try {
            Runtime.getRuntime().exec(arrayOf("settings", "get", "global", "device_name"))
                .inputStream.bufferedReader().readText().trim()
        } catch (e: Throwable) {
            ""
        }
        if (fromSettings.isNotEmpty() && fromSettings != "null") return fromSettings
        val props = listOf("ro.vendor.oplus.market.name", "ro.product.marketname", "ro.product.vendor.marketname")
        val sp = Class.forName("android.os.SystemProperties").getMethod("get", String::class.java)
        for (p in props) {
            val v = sp.invoke(null, p) as String
            if (v.isNotBlank()) return v
        }
        return Build.MODEL.ifBlank { "安卓手机" }
    }
}
