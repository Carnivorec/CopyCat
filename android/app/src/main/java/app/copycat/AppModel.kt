package app.copycat

import android.app.Application
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.asSharedFlow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
import kotlinx.coroutines.withTimeoutOrNull
import org.json.JSONObject

sealed interface Phase {
    data object Checking : Phase
    data object NoRoot : Phase
    data class Failed(val reason: String) : Phase
    data object Ready : Phase

    data object Wiping : Phase

    data object Wiped : Phase

    data class WipeFailed(val reason: String) : Phase
}

class AppModel(app: Application) : AndroidViewModel(app) {
    val phase = MutableStateFlow<Phase>(Phase.Checking)
    val overview = MutableStateFlow<Overview?>(null)
    val pair = MutableStateFlow<PairRequest?>(null)
    val joinSas = MutableStateFlow<String?>(null)
    val revoked = MutableStateFlow(false)
    val bootOn = MutableStateFlow<Boolean?>(null)
    val daemonPid = MutableStateFlow<String?>(null)
    private val _toasts = MutableSharedFlow<String>(extraBufferCapacity = 8)
    val toasts = _toasts.asSharedFlow()

    private var refreshJob: Job? = null

    init {
        connect()
        viewModelScope.launch { DaemonClient.events.collect(::onEvent) }

        viewModelScope.launch {
            DaemonClient.connected.collect { b ->
                if (b == null && phase.value == Phase.Ready) {
                    phase.value = Phase.Checking
                    val back = withTimeoutOrNull(8_000) { DaemonClient.connected.first { it != null } }
                    if (back == null) connect() else onReady()
                }
            }
        }
    }

    fun toast(msg: String) {
        _toasts.tryEmit(msg)
    }

    fun connect() = viewModelScope.launch {
        phase.value = Phase.Checking
        phase.value = when (val r = DaemonClient.ensure(getApplication())) {
            DaemonClient.Ensure.Ok -> Phase.Ready.also { onReady() }
            DaemonClient.Ensure.NoRoot -> Phase.NoRoot
            is DaemonClient.Ensure.Failed -> Phase.Failed(r.reason)
        }
    }

    private fun onReady() {
        phase.value = Phase.Ready
        refresh()
        viewModelScope.launch {
            bootOn.value = DaemonClient.root("boot-status").out == "on"
            daemonPid.value = DaemonClient.root("status").out.removePrefix("running ").takeIf { it != "stopped" }
        }
    }

    fun refresh() {
        refreshJob?.cancel()

        refreshJob = viewModelScope.launch {
            delay(40)
            runCatching { Parse.overview(DaemonClient.request("overview") as JSONObject) }.onSuccess { overview.value = it }
        }
    }

    private fun onEvent(e: JSONObject) {
        when (e.getString("event")) {
            "changed", "arrived" -> refresh()
            "pair-request" -> pair.value = Parse.pairRequest(e.getJSONObject("data"))
            "paired" -> {
                toast("新设备已加入")
                refresh()
            }
            "revoked" -> revoked.value = true
            "join-sas" -> joinSas.value = e.getString("data")

            "transfer" -> runCatching { Parse.transfer(e.getJSONObject("data")) }.onSuccess { t ->
                overview.value = overview.value?.let { ov ->
                    val list = if (ov.transfers.any { it.id == t.id }) ov.transfers.map { if (it.id == t.id) t else it } else listOf(t) + ov.transfers
                    ov.copy(transfers = list)
                }
            }
        }
    }

    private suspend fun act(cmd: String, vararg args: Pair<String, Any?>, done: String? = null): Boolean = try {
        DaemonClient.request(cmd, *args)
        done?.let(::toast)
        refresh()
        true
    } catch (e: Exception) {
        toast(e.message ?: "操作失败")
        false
    }

    fun copy(id: String) = viewModelScope.launch { act("copyItem", "id" to id, done = "已复制") }
    fun forget(id: String) = viewModelScope.launch { act("forgetItem", "id" to id) }
    fun clearHistory() = viewModelScope.launch { act("clearHistory", done = "历史已清空") }
    fun clearHistoryEverywhere() = viewModelScope.launch {
        try {
            val n = (DaemonClient.request("clearHistory", "everywhere" to true) as? Number)?.toInt() ?: 0
            toast(if (n > 0) "已清空本机，并通知了 $n 台在线设备" else "本机历史已清空（没有在线的其他设备）")
        } catch (e: Exception) {
            toast(e.message ?: "操作失败")
        }
        refresh()
    }
    fun removeDevice(id: String) = viewModelScope.launch { act("removeDevice", "id" to id, done = "已移除") }

    fun leaveGroup() = viewModelScope.launch {
        try {
            val r = DaemonClient.request("leaveGroup") as JSONObject
            val peers = r.optInt("peers")
            toast(
                when {
                    peers > 0 -> "已退出设备组，并通知了 $peers 台在线设备"
                    r.optBoolean("relay") -> "已退出设备组，离线的设备连上中转服务器后会收到通知"
                    revoked.value -> "已重新开始，可以重新加入设备组"
                    else -> "已退出设备组。没有在线的设备，请在其他设备上手动移除本机"
                },
            )
            revoked.value = false
        } catch (e: Exception) {
            toast(e.message ?: "退出设备组失败")
        }
        refresh()
    }
    fun setRelayEnabled(on: Boolean) = viewModelScope.launch { act("setRelayEnabled", "enabled" to on) }
    fun setPaused(on: Boolean) = viewModelScope.launch { act("setPaused", "paused" to on) }
    fun saveSettings(s: Settings) = viewModelScope.launch { act("saveSettings", "settings" to s.toJson()) }
    fun cancelTransfer(id: String) = viewModelScope.launch { act("cancelTransfer", "id" to id) }
    fun clearTransfers() = viewModelScope.launch { act("clearTransfers") }

    suspend fun invite(): Invite? = try {
        Parse.invite(DaemonClient.request("createInvite") as JSONObject)
    } catch (e: Exception) {
        toast(e.message ?: "生成配对码失败")
        null
    }

    fun respondPairing(accept: Boolean, name: String) = viewModelScope.launch {
        if (act("respondPairing", "accept" to accept) && accept) toast("“$name”已加入")
        pair.value = null
    }

    suspend fun createGroup(name: String): String? =
        runCatching { DaemonClient.request("createGroup", "name" to name) }
            .also { refresh() }.exceptionOrNull()?.message

    suspend fun joinGroup(uri: String, name: String): String? {
        joinSas.value = null
        return runCatching { DaemonClient.request("joinGroup", "uri" to uri, "name" to name) }
            .also { refresh(); joinSas.value = null }.exceptionOrNull()?.message
    }

    fun setBoot(on: Boolean) = viewModelScope.launch {
        val r = DaemonClient.root(if (on) "boot-on" else "boot-off")
        if (r.code == 0) bootOn.value = on else toast(r.out.ifEmpty { "设置失败" })
    }

    fun wipe() = viewModelScope.launch {
        phase.value = Phase.Wiping
        overview.value = null
        pair.value = null
        joinSas.value = null
        revoked.value = false
        bootOn.value = null
        daemonPid.value = null
        val script = getApplication<Application>().assets.open("wipe.sh").bufferedReader().use { it.readText() }
        val r = Root.run(script, 40)
        val last = r.out.lines().lastOrNull { it.isNotBlank() }
        phase.value = if (r.code == 0 && last == "ok") Phase.Wiped else Phase.WipeFailed(last ?: "未知错误")
    }

    fun restartDaemon() = viewModelScope.launch {
        DaemonClient.root("stop")
        delay(800)
        connect()
    }
}
