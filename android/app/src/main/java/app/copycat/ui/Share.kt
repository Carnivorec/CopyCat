package app.copycat.ui

import androidx.activity.compose.BackHandler
import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.background
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.em
import androidx.compose.ui.unit.sp
import app.copycat.Device
import app.copycat.Overview
import app.copycat.Payload

sealed interface ShareState {
    data object Loading : ShareState
    data class Error(val message: String) : ShareState
    data class Pick(val ov: Overview, val confirming: String? = null, val error: String? = null) : ShareState {

        fun needsConfirm(p: Payload, d: Device): Boolean {
            val mb = ov.settings.relayConfirmMb
            return p is Payload.Files && !d.lan && mb > 0 && p.total > mb * 1024L * 1024L
        }
    }
    data class Sending(val device: String) : ShareState
    data class Sent(val device: String) : ShareState
}

fun fmtSize(n: Long): String = when {
    n < 0 -> "大小未知"
    n < 1024 -> "$n B"
    n < 1024 * 1024 -> "%.1f KB".format(n / 1024.0)
    n < 1024L * 1024 * 1024 -> "%.1f MB".format(n / 1024.0 / 1024)
    else -> "%.2f GB".format(n / 1024.0 / 1024 / 1024)
}

@Composable
fun ShareSheet(payload: Payload, state: ShareState, onSend: (Device) -> Unit, onClose: () -> Unit) {
    BackHandler(onBack = onClose)
    val rise = remember { Animatable(1f) }
    LaunchedEffect(Unit) { rise.animateTo(0f, S.bouncy()) }
    val density = LocalDensity.current
    Box(Modifier.fillMaxSize()) {
        Box(
            Modifier.fillMaxSize().graphicsLayer { alpha = 1f - rise.value }.background(Color(0x66000000))
                .clickable(remember { MutableInteractionSource() }, null, onClick = onClose),
        )
        Column(
            Modifier.align(Alignment.BottomCenter).fillMaxWidth()
                .graphicsLayer { translationY = rise.value * with(density) { 420.dp.toPx() } }
                .background(C.Paper)
                .clickable(remember { MutableInteractionSource() }, null) {},
        ) {
            Box(Modifier.fillMaxWidth().height(6.dp).background(C.Red))
            Column(Modifier.padding(horizontal = Pad).padding(top = 16.dp).windowInsetsPadding(WindowInsets.navigationBars).padding(bottom = 18.dp)) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Box(Modifier.size(11.dp).background(C.Red))
                    Spacer(Modifier.width(9.dp))
                    Text("发送到电脑", style = T.label, modifier = Modifier.weight(1f))
                    Box(Modifier.pressable(onClose).padding(4.dp)) { Icon(Icons.X, 20.dp) }
                }
                Spacer(Modifier.height(14.dp))
                Summary(payload)
                Spacer(Modifier.height(18.dp))
                AnimatedContent(
                    targetState = state,
                    contentKey = { it::class },
                    transitionSpec = { fadeIn(S.gentle()) togetherWith fadeOut(S.snappy()) },
                    label = "share",
                ) { s ->
                    when (s) {
                        ShareState.Loading -> Text("正在连接守护进程…", style = T.bodyMute, modifier = Modifier.padding(vertical = 18.dp))
                        is ShareState.Error -> Column {
                            Text(s.message, style = text(15.sp, 700, color = C.Red, line = 22.sp))
                            Spacer(Modifier.height(16.dp))
                            GhostButton("关闭", onClose)
                        }
                        is ShareState.Pick -> Picker(payload, s, onSend)
                        is ShareState.Sending -> Text("正在交给 ${s.device}…", style = text(22.sp, 900), modifier = Modifier.padding(vertical = 18.dp))
                        is ShareState.Sent -> Row(Modifier.padding(vertical = 18.dp), verticalAlignment = Alignment.CenterVertically) {
                            Icon(Icons.Check, 26.dp, C.Red)
                            Spacer(Modifier.width(10.dp))
                            Text("已开始发送给 ${s.device}", style = text(22.sp, 900), maxLines = 1, overflow = TextOverflow.Ellipsis)
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun Summary(p: Payload) {
    when (p) {
        is Payload.Text -> {
            val link = Regex("https?://\\S+", RegexOption.IGNORE_CASE).containsMatchIn(p.text)
            Text(if (link) "链接" else "文字", style = T.monoSmall.copy(color = C.Red))
            Spacer(Modifier.height(6.dp))
            Text(p.text, style = text(20.sp, 700, line = 27.sp), maxLines = 3, overflow = TextOverflow.Ellipsis)
        }
        is Payload.Files -> {
            Row(verticalAlignment = Alignment.Bottom) {
                Text("${p.items.size}", style = text(64.sp, 900, F.Grotesk, (-0.05).em, 60.sp))
                Spacer(Modifier.width(10.dp))
                Column(Modifier.padding(bottom = 6.dp)) {
                    Text("个文件", style = text(15.sp, 900))
                    Text(if (p.items.any { it.size < 0 }) "大小未知" else fmtSize(p.total), style = T.mono)
                }
            }
            Spacer(Modifier.height(8.dp))
            for (item in p.items.take(3)) Text(item.name, style = text(14.sp, line = 21.sp), maxLines = 1, overflow = TextOverflow.Ellipsis)
            if (p.items.size > 3) Text("等 ${p.items.size} 项", style = text(14.sp, color = C.Mute, line = 21.sp))
        }
    }
}

@Composable
private fun Picker(payload: Payload, s: ShareState.Pick, onSend: (Device) -> Unit) {
    val others = s.ov.others
    var picked by remember { mutableStateOf(s.confirming ?: others.sortedBy { if (it.lan) 0 else if (it.relay) 1 else 2 }.firstOrNull()?.id) }
    val target = others.firstOrNull { it.id == picked }
    Column {
        Row(Modifier.fillMaxWidth().padding(bottom = 8.dp)) {
            Text("发送给", style = T.label, modifier = Modifier.weight(1f))
            Text("${others.size} DEVICES", style = T.mono)
        }
        Rule(2.dp)
        if (others.isEmpty()) Text("还没有其他设备，请先在 CopyCat 里添加。", style = T.bodyMute, modifier = Modifier.padding(vertical = 14.dp))
        for (d in others) {
            val on = d.id == picked
            Row(Modifier.fillMaxWidth().pressable({ picked = d.id }).padding(vertical = 13.dp), verticalAlignment = Alignment.CenterVertically) {
                Box(Modifier.size(12.dp).background(if (on) C.Red else C.Ink).padding(2.dp).background(if (on) C.Red else C.Paper))
                Spacer(Modifier.width(12.dp))
                Text(d.name, style = text(18.sp, 900), maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
                val (label, color) = when {
                    d.lan -> "局域网" to C.Red
                    d.relay -> "经中转" to C.Ink
                    else -> "离线" to C.Mute
                }
                Text(label, style = T.mono.copy(color = color))
            }
            Rule()
        }
        val confirming = target != null && s.confirming == target.id
        val note = when {
            s.error != null -> s.error
            confirming -> "只能经中转服务器发送，会占用服务器流量。再点一次确认。"
            target != null && s.needsConfirm(payload, target) -> "不在同一局域网，将经中转服务器发送。"
            target != null && !target.online -> "${target.name} 不在线，上线后自动发送（最多等 30 分钟）。"
            else -> null
        }
        if (note != null) {
            Spacer(Modifier.height(12.dp))
            Text(note, style = text(13.sp, if (s.error != null || confirming) 700 else 400, color = if (s.error != null || confirming) C.Red else C.Ink, line = 19.sp))
        }
        Spacer(Modifier.height(16.dp))
        Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            PrimaryButton(
                when {
                    target == null -> "没有可发送的设备"
                    confirming -> "确认经中转发送"
                    else -> "发送给 ${target.name}"
                },
                { target?.let(onSend) },
                enabled = target != null,
                icon = Icons.ArrowRight,
                modifier = Modifier.fillMaxWidth(),
            )
        }
    }
}
