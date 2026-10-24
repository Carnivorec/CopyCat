package app.copycat

import org.json.JSONArray
import org.json.JSONObject

data class Device(
    val id: String,
    val short: String,
    val name: String,
    val platform: String,
    val me: Boolean,
    val relay: Boolean,
    val lan: Boolean,
    val screenshots: Boolean = false,
) {
    val online get() = relay || lan
}

data class Clip(
    val id: String,
    val device: String,
    val platform: String,
    val mine: Boolean,
    val time: Long,
    val kind: String,
    val text: String?,
    val chars: Int,
    val hasImage: Boolean,
    val size: Long,

    val pending: Boolean = false,
)

data class Relay(val addr: String, val configured: Boolean, val connected: Boolean, val enabled: Boolean)

data class Settings(
    val deviceName: String,
    val syncImages: Boolean,
    val lanEnabled: Boolean,
    val mailbox: Boolean,
    val relayEnabled: Boolean,
    val excludedApps: List<String>,
    val receiveDir: String = "",
    val askReceiveDir: Boolean = false,
    val openSharedLinks: Boolean = true,
    val sendHotkey: String = "",
    val relayConfirmMb: Int = 50,

    val screenshots: Boolean = false,
    val screenshotTools: List<String> = emptyList(),
    val historyDays: Int = 1,

    val screenshotKeepDays: Int = 0,
    val screenshotMaxCount: Int = 0,
    val screenshotMaxMb: Int = 0,
) {
    fun toJson(): JSONObject = JSONObject()
        .put("deviceName", deviceName)
        .put("syncImages", syncImages)
        .put("lanEnabled", lanEnabled)
        .put("mailbox", mailbox)
        .put("relayEnabled", relayEnabled)
        .put("excludedApps", JSONArray(excludedApps))
        .put("receiveDir", receiveDir)
        .put("askReceiveDir", askReceiveDir)
        .put("openSharedLinks", openSharedLinks)
        .put("sendHotkey", sendHotkey)
        .put("relayConfirmMb", relayConfirmMb)
        .put("screenshots", screenshots)
        .put("screenshotTools", JSONArray(screenshotTools))
        .put("historyDays", historyDays)
        .put("screenshotKeepDays", screenshotKeepDays)
        .put("screenshotMaxCount", screenshotMaxCount)
        .put("screenshotMaxMb", screenshotMaxMb)
}

data class Transfer(
    val id: String,
    val deviceId: String,
    val device: String,
    val outgoing: Boolean,

    val kind: String,
    val files: List<Pair<String, Long>>,
    val count: Int,
    val text: String?,
    val total: Long,
    val done: Long,
    val rate: Long,
    val state: String,
    val error: String?,
    val lan: Boolean,
    val dir: String?,
    val saved: List<String>,
    val time: Long,
    val finished: Long?,

    val savedDirs: List<Boolean> = emptyList(),
) {
    val running get() = state !in setOf("done", "failed", "cancelled")
    val progress get() = if (total > 0) done.toFloat() / total else if (state == "done") 1f else 0f

    val title: String
        get() {
            text?.let { return it.replace(Regex("\\s+"), " ") }
            val first = files.firstOrNull()?.first ?: ""
            val top = first.substringBefore('/')
            return when {
                count == 1 -> first.substringAfterLast('/')
                '/' in first && files.all { it.first.startsWith("$top/") } -> top
                else -> first.substringAfterLast('/') + " 等"
            }
        }
}

data class Overview(
    val initialized: Boolean,
    val me: Device?,
    val relay: Relay?,
    val devices: List<Device>,
    val history: List<Clip>,
    val settings: Settings,
    val paused: Boolean,
    val lanPort: Int?,
    val transfers: List<Transfer> = emptyList(),
    val receiveDir: String = "",
) {
    val others get() = devices.filter { !it.me }
    val onlineCount get() = others.count { it.online }
}

data class PairRequest(val name: String, val platform: String, val sas: String)

data class Invite(val uri: String, val matrix: List<String>)

private fun JSONObject.str(k: String): String? = if (isNull(k)) null else getString(k)

private inline fun <T> JSONArray.map(f: (JSONObject) -> T): List<T> = List(length()) { f(getJSONObject(it)) }

object Parse {
    fun device(j: JSONObject) = Device(
        j.getString("id"), j.getString("short"), j.getString("name"), j.getString("platform"), j.getBoolean("me"),
        j.getBoolean("relay"), j.getBoolean("lan"), j.optBoolean("screenshots"),
    )

    fun clip(j: JSONObject) = Clip(
        j.getString("id"), j.getString("device"), j.getString("platform"), j.getBoolean("mine"), j.getLong("time"),
        j.getString("kind"), j.str("text"), j.getInt("chars"), j.getBoolean("hasImage"), j.getLong("size"),
        j.optBoolean("pending"),
    )

    fun settings(j: JSONObject) = Settings(
        j.getString("deviceName"), j.getBoolean("syncImages"), j.getBoolean("lanEnabled"),
        j.getBoolean("mailbox"), j.getBoolean("relayEnabled"),
        j.getJSONArray("excludedApps").let { a -> List(a.length()) { a.getString(it) } },
        j.optString("receiveDir"), j.optBoolean("askReceiveDir"), j.optBoolean("openSharedLinks", true),
        j.optString("sendHotkey"), j.optInt("relayConfirmMb", 50), j.optBoolean("screenshots"),
        j.optJSONArray("screenshotTools")?.let { a -> List(a.length()) { a.getString(it) } } ?: emptyList(),
        j.optInt("historyDays", 1),
        j.optInt("screenshotKeepDays"), j.optInt("screenshotMaxCount"), j.optInt("screenshotMaxMb"),
    )

    fun transfer(j: JSONObject) = Transfer(
        id = j.getString("id"),
        deviceId = j.getString("deviceId"),
        device = j.getString("device"),
        outgoing = j.getBoolean("outgoing"),
        kind = j.optString("kind", "files"),
        files = j.getJSONArray("files").map { it.getString("name") to it.getLong("size") },
        count = j.getInt("count"),
        text = j.str("text"),
        total = j.getLong("total"),
        done = j.getLong("done"),
        rate = j.getLong("rate"),
        state = j.getString("state"),
        error = j.str("error"),
        lan = j.getBoolean("lan"),
        dir = j.str("dir"),
        saved = j.getJSONArray("saved").let { a -> List(a.length()) { a.getString(it) } },
        time = j.getLong("time"),
        finished = if (j.isNull("finished")) null else j.getLong("finished"),
        savedDirs = j.optJSONArray("savedDirs")?.let { a -> List(a.length()) { a.getBoolean(it) } } ?: emptyList(),
    )

    fun overview(j: JSONObject) = Overview(
        initialized = j.getBoolean("initialized"),
        me = j.optJSONObject("me")?.let(::device),
        relay = j.optJSONObject("relay")?.let {
            Relay(it.getString("addr"), it.optBoolean("configured", true), it.getBoolean("connected"), it.getBoolean("enabled"))
        },
        devices = j.getJSONArray("devices").map(::device),
        history = j.getJSONArray("history").map(::clip),
        settings = settings(j.getJSONObject("settings")),
        paused = j.getBoolean("paused"),
        lanPort = if (j.isNull("lanPort")) null else j.getInt("lanPort"),
        transfers = j.optJSONArray("transfers")?.map(::transfer) ?: emptyList(),
        receiveDir = j.optString("receiveDir"),
    )

    fun invite(j: JSONObject) = Invite(j.getString("uri"), j.getJSONArray("matrix").let { a -> List(a.length()) { a.getString(it) } })

    fun pairRequest(j: JSONObject) = PairRequest(j.getString("name"), j.getString("platform"), j.getString("sas"))
}
