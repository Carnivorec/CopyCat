package app.copycat

import android.service.quicksettings.Tile
import android.service.quicksettings.TileService
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.cancel
import kotlinx.coroutines.launch
import org.json.JSONObject

class PauseTile : TileService() {
    private var scope = CoroutineScope(SupervisorJob() + Dispatchers.Main)

    override fun onStartListening() {
        scope.launch { show(runCatching { paused() }.getOrNull()) }
    }

    override fun onClick() {
        scope.launch {
            val now = runCatching { paused() }.getOrNull() ?: return@launch show(null)
            runCatching { DaemonClient.request("setPaused", "paused" to !now) }
            show(runCatching { paused() }.getOrNull())
        }
    }

    override fun onDestroy() {
        scope.cancel()
        super.onDestroy()
    }

    private suspend fun paused(): Boolean? {
        if (DaemonClient.ensure(this) !is DaemonClient.Ensure.Ok) return null
        val ov = DaemonClient.request("overview") as? JSONObject ?: return null
        return ov.optBoolean("paused")
    }

    private fun show(paused: Boolean?) {
        val t = qsTile ?: return
        t.state = when (paused) {
            null -> Tile.STATE_UNAVAILABLE
            true -> Tile.STATE_ACTIVE
            false -> Tile.STATE_INACTIVE
        }
        t.label = "暂停同步"
        t.subtitle = when (paused) {
            null -> "未运行"
            true -> "已暂停"
            false -> "同步中"
        }
        t.updateTile()
    }
}
