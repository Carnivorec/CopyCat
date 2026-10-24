package app.copycat

import android.content.Intent
import android.graphics.Color
import android.net.Uri
import android.os.Bundle
import android.provider.OpenableColumns
import android.widget.Toast
import androidx.activity.ComponentActivity
import androidx.activity.SystemBarStyle
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.runtime.mutableStateOf
import androidx.lifecycle.lifecycleScope
import app.copycat.ui.ShareSheet
import app.copycat.ui.ShareState
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.json.JSONObject

sealed interface Payload {
    data class Files(val items: List<Item>) : Payload {
        val total get() = items.sumOf { it.size.coerceAtLeast(0) }
    }
    data class Text(val text: String) : Payload

    data class Item(val uri: Uri, val name: String, val size: Long)
}

class ShareActivity : ComponentActivity() {
    private val state = mutableStateOf<ShareState>(ShareState.Loading)

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val bars = SystemBarStyle.light(Color.TRANSPARENT, Color.TRANSPARENT)
        enableEdgeToEdge(bars, bars)
        val payload = parse(intent)
        if (payload == null) {
            Toast.makeText(this, "没有可以发送的内容", Toast.LENGTH_SHORT).show()
            finish()
            return
        }
        setContent {
            ShareSheet(payload, state.value, onSend = { send(payload, it) }, onClose = ::finish)
        }
        lifecycleScope.launch { load(payload) }
    }

    private suspend fun load(payload: Payload) {
        val ensured = DaemonClient.ensure(this)
        if (ensured !is DaemonClient.Ensure.Ok) {
            state.value = ShareState.Error(
                when (ensured) {
                    is DaemonClient.Ensure.Failed -> ensured.reason
                    else -> "需要 Root 权限才能运行 CopyCat 的守护进程"
                },
            )
            return
        }
        val ov = runCatching { Parse.overview(DaemonClient.request("overview") as JSONObject) }.getOrElse {
            state.value = ShareState.Error(it.message ?: "读取设备失败")
            return
        }
        if (!ov.initialized) {
            state.value = ShareState.Error("这台手机还没有加入设备组，请先打开 CopyCat 完成配对")
            return
        }
        val ready = ShareState.Pick(ov)
        state.value = ready
        val only = ov.others.singleOrNull()
        if (only != null && only.online && !ready.needsConfirm(payload, only)) send(payload, only)
    }

    private fun send(payload: Payload, target: Device) {
        val pick = state.value as? ShareState.Pick ?: return
        if (pick.needsConfirm(payload, target) && pick.confirming != target.id) {
            state.value = pick.copy(confirming = target.id, error = null)
            return
        }
        state.value = ShareState.Sending(target.name)
        lifecycleScope.launch {
            try {
                when (payload) {
                    is Payload.Text -> DaemonClient.request("shareText", "device" to target.id, "text" to payload.text)
                    is Payload.Files -> {
                        val fds = withContext(Dispatchers.IO) {
                            val opened = mutableListOf<Pair<String, android.os.ParcelFileDescriptor>>()
                            try {
                                for (item in payload.items) opened += item.name to open(item)
                            } catch (e: Exception) {
                                opened.forEach { runCatching { it.second.close() } }
                                throw e
                            }
                            opened
                        }
                        DaemonClient.sendFiles(target.id, fds)
                    }
                }
                state.value = ShareState.Sent(target.name)
                delay(900)
                finish()
            } catch (e: Exception) {
                state.value = pick.copy(confirming = null, error = e.message ?: "发送失败")
            }
        }
    }

    private fun open(item: Payload.Item): android.os.ParcelFileDescriptor = try {
        contentResolver.openFileDescriptor(item.uri, "r") ?: throw DaemonException("无法读取“${item.name}”")
    } catch (e: SecurityException) {
        throw DaemonException("无法读取“${item.name}”：分享它的应用没有授予访问权限")
    } catch (e: java.io.FileNotFoundException) {
        throw DaemonException("找不到“${item.name}”，它可能已被删除")
    }

    private fun parse(intent: Intent?): Payload? {
        intent ?: return null
        val uris = when (intent.action) {
            Intent.ACTION_SEND -> listOfNotNull(intent.getParcelableExtra(Intent.EXTRA_STREAM, Uri::class.java))
            Intent.ACTION_SEND_MULTIPLE -> intent.getParcelableArrayListExtra(Intent.EXTRA_STREAM, Uri::class.java).orEmpty()
            else -> return null
        }
        if (uris.isNotEmpty()) return Payload.Files(uris.mapIndexed { i, u -> describe(u, i) })
        val text = intent.getCharSequenceExtra(Intent.EXTRA_TEXT)?.toString()?.trim()
            ?: intent.getStringExtra(Intent.EXTRA_SUBJECT)?.trim()
        return text?.takeIf { it.isNotEmpty() }?.let { Payload.Text(it.take(200_000)) }
    }

    private fun describe(uri: Uri, index: Int): Payload.Item {
        var name: String? = null
        var size = -1L
        runCatching {
            contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE), null, null, null)?.use { c ->
                if (c.moveToFirst()) {
                    name = c.getString(0)
                    if (!c.isNull(1)) size = c.getLong(1)
                }
            }
        }
        val fallback = uri.lastPathSegment?.substringAfterLast('/')?.takeIf { it.isNotBlank() } ?: "分享的文件 ${index + 1}"
        return Payload.Item(uri, name?.takeIf { it.isNotBlank() } ?: fallback, size)
    }
}
