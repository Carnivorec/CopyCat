package app.copycat.ui

import android.content.ClipData
import android.content.ClipDescription
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.PersistableBundle
import androidx.activity.compose.BackHandler
import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.slideInHorizontally
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.slideOutHorizontally
import androidx.compose.animation.slideOutVertically
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBars
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.em
import androidx.compose.ui.unit.sp
import app.copycat.AppModel
import app.copycat.Clip
import app.copycat.Device
import app.copycat.FilesProvider
import app.copycat.Invite
import app.copycat.Notifier
import app.copycat.Overview
import app.copycat.PairRequest
import app.copycat.Phase
import app.copycat.Transfer
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import java.text.SimpleDateFormat
import java.util.Calendar
import java.util.Date
import java.util.Locale

enum class Page(val label: String) { Clipboard("剪贴板"), Devices("设备"), Transfer("传输"), Settings("设置") }

private val hhmm = SimpleDateFormat("HH:mm", Locale.CHINA)
private val mdhm = SimpleDateFormat("MM-dd HH:mm", Locale.CHINA)

fun fmtTime(ms: Long): String {
    val c = Calendar.getInstance()
    val today = c.get(Calendar.DAY_OF_YEAR) to c.get(Calendar.YEAR)
    c.timeInMillis = ms
    return if (c.get(Calendar.DAY_OF_YEAR) to c.get(Calendar.YEAR) == today) hhmm.format(Date(ms)) else mdhm.format(Date(ms))
}

fun kindLabel(k: String) = when (k) {
    "otp" -> "验证码"
    "link" -> "链接"
    "code" -> "代码"
    "image" -> "图片"
    "text" -> "文本"
    else -> "其他"
}

fun statusText(ov: Overview): Pair<String, Boolean> {
    val online = ov.onlineCount
    val lan = ov.others.any { it.lan }
    val relay = ov.relay
    val s = when {
        ov.paused -> "已暂停同步"
        relay?.connected == true -> if (online > 0) "$online 台在线${if (lan) " · 局域网" else ""}" else "已连接 · 暂无其他设备在线"
        online > 0 -> "$online 台在线 · 仅局域网"
        relay?.configured != true -> "等待局域网内的设备"
        relay.enabled -> "正在连接中转服务器"
        else -> "中转已关闭"
    }
    return s to (!ov.paused && (relay?.connected == true || online > 0))
}

fun copySensitive(ctx: Context, text: String) {
    val cm = ctx.getSystemService(ClipboardManager::class.java)
    val clip = ClipData.newPlainText("CopyCat 配对码", text)
    clip.description.extras = PersistableBundle().apply { putBoolean(ClipDescription.EXTRA_IS_SENSITIVE, true) }
    cm.setPrimaryClip(clip)
}

@Composable
fun Root(model: AppModel, pendingInvite: String?, onInviteConsumed: () -> Unit, openPage: Page? = null, onPageOpened: () -> Unit = {}) {
    val phase by model.phase.collectAsState()
    val ov by model.overview.collectAsState()
    val pair by model.pair.collectAsState()
    val revoked by model.revoked.collectAsState()
    RootContent(model, phase, ov, pair, revoked, pendingInvite, onInviteConsumed, openPage = openPage, onPageOpened = onPageOpened)
}

@Composable
internal fun RootContent(
    model: AppModel,
    phase: Phase,
    ov: Overview?,
    pair: PairRequest?,
    revoked: Boolean,
    pendingInvite: String?,
    onInviteConsumed: () -> Unit,
    initialPage: Page = Page.Clipboard,
    openPage: Page? = null,
    onPageOpened: () -> Unit = {},
    confirmWipe: Boolean = false,
) {
    val ctx = LocalContext.current
    var scanning by rememberSaveable { mutableStateOf(false) }
    var wipeAsk by rememberSaveable { mutableStateOf(confirmWipe) }
    val askWipe = { wipeAsk = true }
    var scanned by remember { mutableStateOf<String?>(null) }
    Box(Modifier.fillMaxSize().background(C.Paper)) {
        Column(Modifier.fillMaxSize().windowInsetsPadding(WindowInsets.statusBars)) {
            TopBar(ov, phase)
            Box(Modifier.weight(1f)) {
                when {
                    phase is Phase.NoRoot -> Notice(
                        "需要 Root",
                        "CopyCat 的后台同步由 KernelSU 拉起的守护进程完成。请在 KernelSU 管理器中授予 CopyCat 超级用户权限，然后重试。",
                        "重试",
                    ) { model.connect() }
                    phase is Phase.Failed -> Notice(
                        "守护进程\n没有启动", phase.reason, "重试",
                        secondary = "清除全部数据（卸载前）", onSecondary = askWipe,
                    ) { model.connect() }
                    phase is Phase.Wiping -> Splash("正在清除本机数据")
                    phase is Phase.Wiped -> Notice(
                        "数据已清除",
                        "守护进程已停止，本机的设备身份、密钥、剪贴板历史与设置都已删除，开机自启已取消。现在可以卸载 CopyCat。\n\n" +
                            "收到的文件仍保存在 Download/CopyCat，需要时请自行删除。",
                        "卸载 CopyCat",
                        secondary = "暂不卸载，重新开始", secondaryIcon = Icons.ArrowRight, onSecondary = { model.connect() },
                    ) { uninstall(ctx) }
                    phase is Phase.WipeFailed -> Notice(
                        "清除没有完成", phase.reason, "再试一次",
                        secondary = "返回 CopyCat", secondaryIcon = Icons.ArrowRight, onSecondary = { model.connect() },
                    ) { model.wipe() }
                    phase !is Phase.Ready || ov == null -> Splash()
                    !ov.initialized -> Onboarding(
                        model, ov,
                        invite = scanned ?: pendingInvite,
                        onInviteConsumed = { scanned = null; onInviteConsumed() },
                        onScan = { scanning = true },
                        onWipe = askWipe,
                    )
                    else -> Main(model, ov, initialPage, openPage, onPageOpened, askWipe)
                }
            }
        }
        if (pair != null) PairSheet(model, pair)
        if (scanning) ScanSheet(onResult = { scanned = it; scanning = false }, onClose = { scanning = false })
        if (revoked && phase == Phase.Ready) Notice(
            "本机已被\n移出设备组",
            "同步已停止。点“重新开始”删除本机的设备身份与历史，之后可以重新创建或加入设备组。",
            "重新开始",
            Modifier.background(C.Paper).windowInsetsPadding(WindowInsets.statusBars),
            secondary = "清除全部数据（卸载前）", onSecondary = askWipe,
        ) { model.leaveGroup() }
        if (wipeAsk) WipeConfirm(onConfirm = { wipeAsk = false; model.wipe() }, onCancel = { wipeAsk = false })
        ToastHost(
            model.toasts,
            Modifier.align(Alignment.BottomCenter).windowInsetsPadding(WindowInsets.navigationBars).padding(bottom = 86.dp),
        )
    }
}

@Composable
private fun TopBar(ov: Overview?, phase: Phase) {
    Row(Modifier.fillMaxWidth().padding(horizontal = Pad, vertical = 14.dp), verticalAlignment = Alignment.CenterVertically) {
        Box(Modifier.size(11.dp).background(C.Red))
        Spacer(Modifier.width(9.dp))
        Text("CopyCat", style = text(15.sp, 800, F.Grotesk, (-0.01).em))
        Spacer(Modifier.weight(1f))
        val (label, live) = when {
            phase is Phase.Wiping -> "正在清除" to false
            phase is Phase.Wiped -> "数据已清除" to false
            phase !is Phase.Ready -> "守护进程" to false
            ov == null -> "" to false
            !ov.initialized -> "未加入设备组" to false
            else -> statusText(ov)
        }
        Box(Modifier.size(7.dp).background(if (live) C.Red else C.Mute))
        Spacer(Modifier.width(7.dp))
        Text(label, style = text(12.sp, 500))
    }
}

@Composable
private fun Splash(label: String = "正在连接守护进程") {
    Column(Modifier.fillMaxSize().padding(Pad), verticalArrangement = Arrangement.Center) {
        Text("CopyCat", style = text(64.sp, 900, F.Grotesk, (-0.045).em))
        Spacer(Modifier.height(14.dp))
        val w = remember { Animatable(0f) }
        LaunchedEffect(Unit) {
            while (true) {
                w.animateTo(1f, S.rule())
                w.animateTo(0f, S.rule())
            }
        }
        Box(Modifier.fillMaxWidth(0.7f).height(8.dp)) { Box(Modifier.fillMaxWidth(w.value.coerceIn(0.02f, 1f)).height(8.dp).background(C.Red)) }
        Spacer(Modifier.height(18.dp))
        Text(label, style = T.mono)
    }
}

@Composable
private fun Notice(
    title: String,
    body: String,
    action: String?,
    modifier: Modifier = Modifier,
    secondary: String? = null,
    secondaryIcon: ImageVector = Icons.Trash,
    onSecondary: () -> Unit = {},
    onAction: () -> Unit,
) {
    Column(modifier.fillMaxSize().padding(Pad), verticalArrangement = Arrangement.Center) {
        Box(Modifier.size(14.dp).background(C.Red))
        Spacer(Modifier.height(18.dp))
        Text(title, style = T.title)
        Spacer(Modifier.height(16.dp))
        Text(body, style = T.body)
        if (action != null) {
            Spacer(Modifier.height(28.dp))
            PrimaryButton(action, onAction, icon = Icons.ArrowRight)
        }
        if (secondary != null) {
            Spacer(Modifier.height(14.dp))
            TextAction(secondary, secondaryIcon, onSecondary)
        }
    }
}

private fun uninstall(ctx: Context) {
    ctx.startActivity(Intent(Intent.ACTION_DELETE, Uri.parse("package:${ctx.packageName}")))
}

@Composable
private fun WipeConfirm(onConfirm: () -> Unit, onCancel: () -> Unit) {
    BackHandler(onBack = onCancel)
    Column(
        Modifier.fillMaxSize().background(C.Paper).clickable(remember { MutableInteractionSource() }, null) {}
            .windowInsetsPadding(WindowInsets.statusBars)
            .verticalScroll(rememberScrollState()).padding(Pad),
    ) {
        Spacer(Modifier.height(36.dp))
        Box(Modifier.size(14.dp).background(C.Red))
        Spacer(Modifier.height(18.dp))
        Text("清除全部数据", style = T.title)
        Spacer(Modifier.height(16.dp))
        Text("卸载 CopyCat 前请先在这里清除。守护进程以 root 运行，数据保存在 App 目录之外，直接卸载不会删除它们。", style = T.body)
        Spacer(Modifier.height(24.dp))
        WipeList("将删除", listOf("设备身份与密钥、设备组名册", "剪贴板历史与本机设置", "守护进程与开机自启"))
        WipeList("保留", listOf("收到的文件（Download/CopyCat）"))
        Spacer(Modifier.height(16.dp))
        Text("清除后无法恢复，本机退出同步。请在其他设备的“设备”页把这台手机移除。", style = T.bodyMute)
        Spacer(Modifier.height(28.dp))
        PrimaryButton("清除全部数据", onConfirm, icon = Icons.Trash)
        Spacer(Modifier.height(14.dp))
        TextAction("取消", Icons.X, onCancel)
        Spacer(Modifier.height(24.dp))
    }
}

@Composable
private fun WipeList(title: String, items: List<String>) {
    Row(Modifier.fillMaxWidth().padding(vertical = 10.dp)) { Text(title, style = T.label) }
    Rule(2.dp)
    for (item in items) {
        Text(item, style = T.body, modifier = Modifier.padding(vertical = 10.dp))
        Rule()
    }
    Spacer(Modifier.height(14.dp))
}

@Composable
private fun Main(model: AppModel, ov: Overview, initialPage: Page, openPage: Page?, onPageOpened: () -> Unit, onWipe: () -> Unit) {
    var page by rememberSaveable { mutableStateOf(initialPage) }

    LaunchedEffect(openPage) {
        if (openPage != null) {
            page = openPage
            onPageOpened()
        }
    }
    Column(Modifier.fillMaxSize()) {
        Header(page, ov)
        AnimatedContent(
            targetState = page,
            transitionSpec = {
                val dir = if (targetState.ordinal > initialState.ordinal) 1 else -1
                (slideInHorizontally(S.snappy()) { it / 5 * dir } + fadeIn(S.gentle())) togetherWith
                    (slideOutHorizontally(S.snappy()) { -it / 5 * dir } + fadeOut(S.snappy()))
            },
            modifier = Modifier.weight(1f).clipToBounds(),
            label = "page",
        ) { p ->
            when (p) {
                Page.Clipboard -> ClipboardPage(model, ov)
                Page.Devices -> DevicesPage(model, ov)
                Page.Transfer -> TransferPage(model, ov)
                Page.Settings -> SettingsPage(model, ov, onWipe)
            }
        }
        BottomNav(page) { page = it }
    }
}

@Composable
private fun Header(page: Page, ov: Overview) {
    val ctx = LocalContext.current
    val version = remember { runCatching { ctx.packageManager.getPackageInfo(ctx.packageName, 0).versionName }.getOrNull().orEmpty() }
    val (n, label, aside) = when (page) {
        Page.Clipboard -> Triple(ov.history.size, "条剪贴板", if (ov.settings.historyDays > 1) "保留 ${ov.settings.historyDays} 天" else "保留 24 H")
        Page.Devices -> Triple(ov.devices.size, "台设备", ov.relay?.addr?.ifEmpty { null }?.uppercase() ?: "仅局域网")
        Page.Transfer -> Triple(ov.transfers.size, "项传输", Notifier.shortDir(ov.receiveDir).uppercase())
        Page.Settings -> Triple(ov.history.size, "条剪贴板", "V$version")
    }
    Column(Modifier.padding(horizontal = Pad)) {
        Odometer(n, text(96.sp, 900, F.Grotesk, (-0.05).em), Modifier.padding(top = 2.dp))
        Rule(2.dp)
        Row(Modifier.fillMaxWidth().padding(top = 8.dp, bottom = 14.dp), verticalAlignment = Alignment.CenterVertically) {
            Text(label, style = text(15.sp, 900))
            Spacer(Modifier.weight(1f))
            Text(aside, style = T.mono, maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
    }
}

@Composable
private fun BottomNav(page: Page, onSelect: (Page) -> Unit) {
    Column(Modifier.windowInsetsPadding(WindowInsets.navigationBars)) {
        Rule(2.dp, modifier = Modifier.padding(horizontal = Pad))
        Row(Modifier.fillMaxWidth().padding(horizontal = Pad, vertical = 6.dp)) {
            for (p in Page.entries) {
                val on = p == page
                val sq by animateFloatAsState(if (on) 1f else 0f, S.bouncy(), label = "nav")
                Row(
                    Modifier.weight(1f).pressable({ onSelect(p) }).padding(vertical = 12.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Box(Modifier.size(9.dp).graphicsLayer { scaleX = sq; scaleY = sq }.background(C.Red))
                    Spacer(Modifier.width(8.dp))
                    Text(p.label, style = text(18.sp, 900, color = if (on) C.Ink else C.Mute))
                }
            }
        }
    }
}

@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun ClipboardPage(model: AppModel, ov: Overview) {
    var query by rememberSaveable { mutableStateOf("") }
    val haptic = LocalHapticFeedback.current
    val hero = ov.history.firstOrNull()
    val rest = ov.history.drop(1).filter { query.isBlank() || (it.text ?: "").contains(query, ignoreCase = true) || it.device.contains(query) }
    LazyColumn(Modifier.fillMaxSize().padding(horizontal = Pad)) {
        item(key = "hero") { Hero(hero, onCopy = { haptic.performHapticFeedback(HapticFeedbackType.Confirm); model.copy(it.id) }, onDelete = { model.forget(it.id) }) }
        stickyHeader(key = "head") {
            Row(Modifier.fillMaxWidth().background(C.Paper).padding(top = 18.dp, bottom = 10.dp), verticalAlignment = Alignment.CenterVertically) {
                Text("NO.", style = T.mono, modifier = Modifier.width(40.dp))
                Icon(Icons.Search, 15.dp, C.Mute)
                Spacer(Modifier.width(8.dp))
                Box(Modifier.weight(1f)) {
                    if (query.isEmpty()) Text("搜索历史", style = text(14.sp, color = C.Mute))
                    BasicTextField(query, { query = it }, singleLine = true, textStyle = text(14.sp), cursorBrush = SolidColor(C.Red), modifier = Modifier.fillMaxWidth())
                }
                Text("时间", style = text(12.sp, 700))
            }
            Rule(2.dp)
        }
        itemsIndexed(rest, key = { _, c -> c.id }) { i, c ->
            Box(Modifier.animateItem(fadeInSpec = S.gentle(), placementSpec = S.snappy(), fadeOutSpec = S.snappy())) {
                SwipeRow(onRemove = { model.forget(c.id) }, onTap = { haptic.performHapticFeedback(HapticFeedbackType.Confirm); model.copy(c.id) }) {
                    ClipRow(i + 2, c)
                }
            }
        }
        item(key = "tail") {
            Text(
                if (ov.history.size <= 1) "更早的内容会出现在这里" else "轻点复制 · 左滑删除",
                style = text(13.sp, color = C.Mute),
                modifier = Modifier.padding(top = 18.dp, bottom = 28.dp),
            )
        }
    }
}

@Composable
private fun Hero(c: Clip?, onCopy: (Clip) -> Unit, onDelete: (Clip) -> Unit) {
    AnimatedContent(
        targetState = c,
        contentKey = { it?.id },
        transitionSpec = {
            (slideInVertically(S.bouncy()) { -it / 4 } + fadeIn(S.gentle())) togetherWith (slideOutVertically(S.snappy()) { it / 5 } + fadeOut(S.snappy()))
        },
        label = "hero",
    ) { clip ->
        Column(Modifier.fillMaxWidth()) {
            if (clip == null) {
                Text("还没有内容", style = text(40.sp, 900, line = 44.sp))
                Spacer(Modifier.height(10.dp))
                Text("在这台手机或其他设备上复制点什么，它会马上出现在这里。", style = T.bodyMute)
                Spacer(Modifier.height(18.dp))
                Rule(4.dp, C.Red)
                return@Column
            }
            Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                Text(if (clip.mine) "本机复制" else "来自 ${clip.device.uppercase()}", style = T.label, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
                Text("${kindLabel(clip.kind)} · ${fmtTime(clip.time)}", style = T.mono)
            }
            Spacer(Modifier.height(10.dp))
            val body = clip.text ?: if (clip.hasImage) "图片" else "（无法预览）"
            val style = when (clip.kind) {
                "otp" -> text(78.sp, 900, F.Grotesk, (-0.04).em, 80.sp)
                "code" -> text(22.sp, 600, F.Mono, line = 30.sp)
                "link" -> text(26.sp, 800, F.Grotesk, (-0.02).em, 31.sp)
                else -> text(30.sp, 900, line = 38.sp, spacing = (-0.01).em)
            }
            Text(body.trim(), style = style, maxLines = if (clip.kind == "otp") 1 else 4, overflow = TextOverflow.Ellipsis)
            Spacer(Modifier.height(16.dp))
            val wipe = remember { Animatable(0f) }
            LaunchedEffect(clip.id) { wipe.animateTo(1f, S.rule()) }
            Box(Modifier.fillMaxWidth(wipe.value.coerceAtLeast(0.001f)).height(4.dp).background(C.Red))
            Row(Modifier.fillMaxWidth().padding(top = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                TextAction("复制", Icons.Copy, { onCopy(clip) })
                Spacer(Modifier.width(22.dp))
                TextAction("删除", Icons.Trash, { onDelete(clip) })
                Spacer(Modifier.weight(1f))
                Text("${clip.chars} 字", style = T.mono.copy(color = C.Mute))
            }
        }
    }
}

@Composable
private fun ClipRow(n: Int, c: Clip) {
    Column {
        Row(Modifier.fillMaxWidth().padding(vertical = 13.dp), verticalAlignment = Alignment.Top) {
            Text("%02d".format(n), style = T.mono, modifier = Modifier.width(40.dp).padding(top = 3.dp))
            Column(Modifier.weight(1f)) {
                val kind = if (c.kind == "otp" || c.kind == "link" || c.kind == "code") kindLabel(c.kind) + "  " else ""
                Row(verticalAlignment = Alignment.CenterVertically) {
                    if (kind.isNotEmpty()) Text(kind.trim(), style = T.monoSmall.copy(color = C.Red), modifier = Modifier.padding(end = 8.dp))
                    Text(
                        (c.text ?: if (c.hasImage) "图片" else "（无法预览）").replace('\n', ' '),
                        style = if (c.kind == "code") text(15.sp, 500, F.Mono) else T.row,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                }
                Spacer(Modifier.height(4.dp))
                Text(if (c.mine) "本机" else c.device, style = text(12.sp, 700))
            }
            Text(fmtTime(c.time), style = T.mono, modifier = Modifier.padding(start = 12.dp, top = 3.dp))
        }
        Rule()
    }
}

@Composable
private fun DevicesPage(model: AppModel, ov: Overview) {
    var invite by remember { mutableStateOf<Invite?>(null) }
    val scope = rememberCoroutineScope()
    BackHandler(invite != null) { invite = null }
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(horizontal = Pad)) {
        Text("设备之间的所有内容都经过端到端加密；中转服务器只转发无法解读的密文。被移除的设备无法用原来的身份重新加入。", style = T.bodyMute)
        Spacer(Modifier.height(16.dp))
        Rule(2.dp)
        ov.devices.forEachIndexed { i, d -> DeviceRow(i + 1, d) { model.removeDevice(d.id) } }
        Spacer(Modifier.height(22.dp))
        AnimatedContent(
            targetState = invite,
            transitionSpec = { (slideInVertically(S.bouncy()) { it / 3 } + fadeIn(S.gentle())) togetherWith fadeOut(S.snappy()) },
            label = "invite",
        ) { inv ->
            if (inv == null) {
                PrimaryButton("添加设备", { scope.launch { invite = model.invite() } }, icon = Icons.Plus)
            } else {
                InvitePanel(inv) { invite = null }
            }
        }
        Spacer(Modifier.height(32.dp))
    }
}

@Composable
private fun DeviceRow(n: Int, d: Device, onRemove: () -> Unit) {
    var confirm by remember { mutableStateOf(false) }
    LaunchedEffect(confirm) {
        if (confirm) {
            delay(3000)
            confirm = false
        }
    }
    Column {
        Row(Modifier.fillMaxWidth().padding(vertical = 16.dp), verticalAlignment = Alignment.Top) {
            Text("%02d".format(n), style = T.mono, modifier = Modifier.width(36.dp).padding(top = 8.dp))
            Column(Modifier.weight(1f)) {
                Text(d.name, style = text(26.sp, 900, line = 30.sp), maxLines = 1, overflow = TextOverflow.Ellipsis)
                Spacer(Modifier.height(4.dp))
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Icon(if (d.platform == "android") Icons.Phone else Icons.Monitor, 13.dp, C.Mute)
                    Spacer(Modifier.width(6.dp))
                    Text("${d.platform.uppercase()} · ${d.short}", style = T.mono.copy(color = C.Mute))
                }
                Spacer(Modifier.height(10.dp))
                Row(verticalAlignment = Alignment.CenterVertically) {
                    val (label, color) = when {
                        d.me -> "本机" to C.Mute
                        d.lan -> "局域网直连" to C.Red
                        d.relay -> "经中转" to C.Ink
                        else -> "离线" to C.Faint
                    }
                    Box(Modifier.size(9.dp).background(color))
                    Spacer(Modifier.width(8.dp))
                    Text(label, style = text(14.sp, 700, color = if (d.lan) C.Red else C.Ink))
                    Spacer(Modifier.weight(1f))
                    if (!d.me) {
                        TextAction(if (confirm) "确认移除" else "移除", Icons.Trash, {
                            if (confirm) onRemove() else confirm = true
                        }, if (confirm) C.Red else C.Ink)
                    }
                }
            }
        }
        Rule()
    }
}

@Composable
private fun InvitePanel(inv: Invite, onClose: () -> Unit) {
    val ctx = LocalContext.current
    val ttl = remember { Animatable(1f) }
    LaunchedEffect(inv.uri) {
        ttl.snapTo(1f)
        ttl.animateTo(0f, tween(120_000, easing = LinearEasing))
        onClose()
    }
    Column(Modifier.fillMaxWidth()) {
        Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
            Text("添加设备", style = text(22.sp, 900), modifier = Modifier.weight(1f))
            Box(Modifier.pressable(onClose).padding(6.dp)) { Icon(Icons.X, 20.dp) }
        }
        Spacer(Modifier.height(10.dp))
        Text("在新设备上扫描二维码或粘贴配对码。两台设备会显示同一个 6 位确认码，核对一致后在这里同意。", style = T.bodyMute)
        Spacer(Modifier.height(18.dp))
        BoxWithConstraints(Modifier.fillMaxWidth()) {
            QrMatrix(inv.matrix, Modifier.size(minOf(maxWidth, 280.dp)))
        }
        Spacer(Modifier.height(12.dp))
        Box(Modifier.fillMaxWidth().height(3.dp).background(C.Faint)) {
            Box(Modifier.fillMaxWidth(ttl.value.coerceAtLeast(0.001f)).height(3.dp).background(C.Red))
        }
        Row(Modifier.fillMaxWidth().padding(top = 6.dp)) {
            Text("单次有效", style = T.mono)
            Spacer(Modifier.weight(1f))
            Text("${(ttl.value * 120).toInt()} S", style = T.mono)
        }
        Spacer(Modifier.height(16.dp))
        Text(inv.uri, style = text(12.sp, 400, F.Mono, line = 18.sp, color = Color(0xFF555555)), maxLines = 3, overflow = TextOverflow.Ellipsis)
        Spacer(Modifier.height(16.dp))
        Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            PrimaryButton("复制配对码", { copySensitive(ctx, inv.uri) }, icon = Icons.Copy)
            GhostButton("发送", {
                val send = Intent(Intent.ACTION_SEND).setType("text/plain").putExtra(Intent.EXTRA_TEXT, inv.uri)
                ctx.startActivity(Intent.createChooser(send, "发送配对码"))
            }, icon = Icons.ArrowRight)
        }
    }
}

fun transferStatus(t: Transfer, online: Boolean): Pair<String, String> {
    val pct = (t.progress * 100).toInt()
    return when (t.state) {
        "preparing" -> "准备中" to "计算校验值"
        "waiting" -> if (t.outgoing) (if (online) "等待对方" else "等对方上线") to (if (t.done > 0) "$pct%" else "") else "已暂停" to "$pct%"
        "asking" -> "等待确认" to ""
        "active" -> "$pct%" to (if (t.rate > 0) "${fmtSize(t.rate)}/s" else "")
        "done" -> (if (t.outgoing) "已送达" else if (t.text != null) "已收到" else "已保存") to fmtTime(t.finished ?: t.time)
        "failed" -> "失败" to (t.error ?: "")
        else -> "已取消" to fmtTime(t.finished ?: t.time)
    }
}

@Composable
private fun TransferPage(model: AppModel, ov: Overview) {
    LazyColumn(Modifier.fillMaxSize().padding(horizontal = Pad)) {
        item(key = "intro") {
            Text(
                "在其他应用里点“分享”，选择“发送到电脑”即可发送文件、链接或文字。电脑发来的文件自动保存到 ${Notifier.shortDir(ov.receiveDir)}，锁屏时也能接收。",
                style = T.bodyMute,
            )
            Spacer(Modifier.height(16.dp))
            Rule(2.dp)
        }
        itemsIndexed(ov.transfers, key = { _, t -> t.id }) { i, t ->
            Box(Modifier.animateItem(fadeInSpec = S.gentle(), placementSpec = S.snappy(), fadeOutSpec = S.snappy())) {
                val online = ov.devices.any { it.id == t.deviceId && it.online }
                TransferRow(i + 1, t, online, onCancel = { model.cancelTransfer(t.id) })
            }
        }
        item(key = "tail") {
            if (ov.transfers.isEmpty()) {
                Text("还没有传输记录", style = text(13.sp, color = C.Mute), modifier = Modifier.padding(top = 18.dp, bottom = 28.dp))
            } else if (ov.transfers.any { !it.running }) {
                Box(Modifier.padding(top = 14.dp, bottom = 28.dp)) { TextAction("清除已结束的记录", Icons.Trash, model::clearTransfers) }
            }
        }
    }
}

@Composable
private fun TransferRow(n: Int, t: Transfer, online: Boolean, onCancel: () -> Unit) {
    val (main, sub) = transferStatus(t, online)
    val ctx = LocalContext.current

    val openable = !t.outgoing && t.state == "done" && t.saved.isNotEmpty()
    var expanded by remember { mutableStateOf(false) }
    val tap: (() -> Unit)? = when {
        !openable -> null
        t.saved.size == 1 -> { { FilesProvider.open(ctx, t.saved[0], t.savedDirs.firstOrNull() ?: false) } }
        else -> { { expanded = !expanded } }
    }
    Column {
        Row(
            Modifier.fillMaxWidth().then(if (tap != null) Modifier.pressable(tap) else Modifier).padding(vertical = 13.dp),
            verticalAlignment = Alignment.Top,
        ) {
            Text("%02d".format(n), style = T.mono, modifier = Modifier.width(40.dp).padding(top = 3.dp))
            Column(Modifier.weight(1f)) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Text(if (t.kind == "screenshot") "截图" else if (t.text != null) "分享" else if (t.outgoing) "发送" else "接收", style = T.monoSmall.copy(color = C.Red), modifier = Modifier.padding(end = 8.dp))
                    Text(t.title, style = T.row, maxLines = 1, overflow = TextOverflow.Ellipsis)
                }
                Spacer(Modifier.height(4.dp))
                val size = if (t.text != null) "${t.text.length} 字" else (if (t.count > 1) "${t.count} 个文件 · " else "") + fmtSize(t.total)
                Text("${if (t.outgoing) "发给" else "来自"} ${t.device} · $size", style = text(12.sp, 700), maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
            Column(Modifier.padding(start = 12.dp).width(96.dp), horizontalAlignment = Alignment.End) {
                Text(main, style = text(16.sp, 900, color = if (t.state == "failed") C.Red else if (t.state == "cancelled") C.Mute else C.Ink))
                if (sub.isNotEmpty()) Text(sub, style = T.monoSmall.copy(color = C.Mute), maxLines = 2, overflow = TextOverflow.Ellipsis)
                if (t.running) {
                    Spacer(Modifier.height(4.dp))
                    Text(if (t.state == "asking") "拒绝" else "取消", style = text(13.sp, 700), modifier = Modifier.pressable(onCancel).padding(vertical = 4.dp))
                }
            }
        }
        if (t.running) {
            val p by animateFloatAsState(t.progress, S.gentle(), label = "progress")
            Box(Modifier.fillMaxWidth(p.coerceIn(0.001f, 1f)).height(4.dp).background(C.Red))
        }
        if (expanded) {
            t.saved.forEachIndexed { i, path ->
                val dir = t.savedDirs.getOrNull(i) ?: false
                Row(
                    Modifier.fillMaxWidth().pressable({ FilesProvider.open(ctx, path, dir) }).padding(start = 40.dp, top = 8.dp, bottom = 8.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Text(if (dir) "文件夹" else "文件", style = T.monoSmall.copy(color = C.Mute), modifier = Modifier.width(52.dp))
                    Text(path.substringAfterLast('/'), style = text(14.sp, 700), maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
                    Text("打开", style = text(13.sp, 700, color = C.Red))
                }
            }
            Spacer(Modifier.height(8.dp))
        }
        Rule()
    }
}

@Composable
private fun SettingsPage(model: AppModel, ov: Overview, onWipe: () -> Unit) {
    val boot by model.bootOn.collectAsState()
    val pid by model.daemonPid.collectAsState()
    val s = ov.settings
    var clearConfirm by remember { mutableStateOf(false) }
    LaunchedEffect(clearConfirm) {
        if (clearConfirm) {
            delay(3000)
            clearConfirm = false
        }
    }
    var clearAllConfirm by remember { mutableStateOf(false) }
    LaunchedEffect(clearAllConfirm) {
        if (clearAllConfirm) {
            delay(3000)
            clearAllConfirm = false
        }
    }
    var leaveConfirm by remember { mutableStateOf(false) }
    LaunchedEffect(leaveConfirm) {
        if (leaveConfirm) {
            delay(3000)
            leaveConfirm = false
        }
    }
    Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(horizontal = Pad)) {
        val relay = ov.relay
        val configured = relay?.configured == true
        Section("连接", if (!configured) "LAN ONLY" else if (relay.connected) "ONLINE" else "OFFLINE")
        SettingRow(
            "中转服务器",
            if (configured) "跨网络同步经过这台服务器转发，它只能看到密文。"
            else "未配置，只在同一局域网内同步。需要跨网络同步时，在电脑端“设置 → 中转服务器”添加，配置会自动同步到手机。",
            below = {
                if (configured) {
                    Row(Modifier.fillMaxWidth().padding(top = 10.dp), verticalAlignment = Alignment.CenterVertically) {
                        Text(relay.addr, style = T.mono.copy(color = C.Red), maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
                        Text(if (relay.connected) "已连接" else if (relay.enabled) "连接中" else "本机已关闭", style = text(12.sp, 700))
                    }
                }
            },
        ) {}
        if (configured) {
            SettingRow("本机使用中转服务器", "只影响本机：关闭后只在同一局域网内同步。") { Toggle(s.relayEnabled, model::setRelayEnabled) }
        }
        SettingRow("局域网直连", "同一网络下设备直接连接，延迟更低。重启守护进程后生效。") {
            Toggle(s.lanEnabled, { model.saveSettings(s.copy(lanEnabled = it)) })
        }
        SettingRow("离线信箱", "对方离线时，把小于 256 KB 的内容加密暂存在中转服务器，最长 24 小时。重启守护进程后生效。") {
            Toggle(s.mailbox, { model.saveSettings(s.copy(mailbox = it)) })
        }

        Section("同步", if (ov.paused) "PAUSED" else "ACTIVE")
        SettingRow("暂停同步", "暂停期间既不发送本机复制的内容，也不写入收到的内容。") { Toggle(ov.paused, model::setPaused) }
        SettingRow("排除的应用", "这些应用复制的内容不会同步（按包名）。密码管理器标记为敏感的内容始终不同步。") {}
        Text(s.excludedApps.joinToString("\n"), style = text(12.sp, 400, F.Mono, line = 19.sp, color = Color(0xFF555555)), modifier = Modifier.padding(bottom = 16.dp))
        Rule()

        Section("文件传输", "${ov.transfers.size} 项")
        SettingRow("保存位置", "电脑发来的文件直接保存在这里，锁屏时也能接收。", below = {
            Text(ov.receiveDir.ifEmpty { "—" }, style = T.mono.copy(color = C.Red), maxLines = 2, overflow = TextOverflow.Ellipsis)
        }) {}
        val mb = s.relayConfirmMb.takeIf { it > 0 } ?: 50
        SettingRow("经中转发送前确认", "不在同一局域网、只能经中转服务器发送时，超过 $mb MB 先确认，避免耗费服务器流量。") {
            Toggle(s.relayConfirmMb > 0, { model.saveSettings(s.copy(relayConfirmMb = if (it) mb else 0)) })
        }

        Section("电脑截图", if (s.screenshots) "ON" else "OFF")
        SettingRow(
            "接收电脑截图",
            "电脑上也打开“截图自动发到手机”后，电脑截的图会自动保存到 ${Notifier.shortDir(ov.receiveDir)}/screenshot，并出现在相册中。",
        ) {
            Toggle(s.screenshots, { model.saveSettings(s.copy(screenshots = it)) })
        }
        SettingRow("自动清理", "只删除 CopyCat 保存在 screenshot 目录中的截图。下面三项满足任何一项，就从最旧的开始删除。") {}
        SettingRow("保留时间", "超过这个时间的截图删除。", below = {
            Box(Modifier.padding(top = 12.dp)) {
                Segmented(listOf("不限" to 0, "1 天" to 1, "7 天" to 7, "30 天" to 30), s.screenshotKeepDays) { model.saveSettings(s.copy(screenshotKeepDays = it)) }
            }
        }) {}
        SettingRow("最多张数", "超过这个数量时删除较旧的截图。", below = {
            Box(Modifier.padding(top = 12.dp)) {
                Segmented(listOf("不限" to 0, "50" to 50, "200" to 200, "1000" to 1000), s.screenshotMaxCount) { model.saveSettings(s.copy(screenshotMaxCount = it)) }
            }
        }) {}
        SettingRow("最多占用", "截图总大小超过这个值时删除较旧的截图，最新的一张总会保留。", below = {
            Box(Modifier.padding(top = 12.dp)) {
                Segmented(listOf("不限" to 0, "200 MB" to 200, "1 GB" to 1024, "5 GB" to 5120), s.screenshotMaxMb) { model.saveSettings(s.copy(screenshotMaxMb = it)) }
            }
        }) {}

        Section("守护进程", if (pid != null) "PID $pid" else "—")
        SettingRow("开机自启", "由 KernelSU 在开机后拉起守护进程（/data/adb/service.d/copycat.sh，删除即可取消）。") {
            Toggle(boot == true, model::setBoot, enabled = boot != null)
        }
        SettingRow("重启守护进程", "修改局域网直连或离线信箱后需要重启。") {
            GhostButton("重启", model::restartDaemon, icon = Icons.Power)
        }

        Section("历史", "${ov.history.size} 条")
        SettingRow("保留时间", "超过这个时间的记录自动删除，每台设备最多保留 50、200 或 500 条。", below = {
            Box(Modifier.padding(top = 12.dp)) {
                Segmented(listOf("1 天" to 1, "7 天" to 7, "30 天" to 30), s.historyDays) { model.saveSettings(s.copy(historyDays = it)) }
            }
        }) {}
        SettingRow("清空所有设备的历史", "同时请在线的其他设备删除全部记录，例如误复制了密码时。离线的设备不受影响。") {
            GhostButton(if (clearAllConfirm) "确认清空" else "全部清空", {
                if (clearAllConfirm) {
                    model.clearHistoryEverywhere()
                    clearAllConfirm = false
                } else {
                    clearAllConfirm = true
                }
            }, if (clearAllConfirm) C.Red else C.Ink)
        }
        SettingRow("清空历史", "删除本机保存的全部剪贴板记录，不影响其他设备。") {
            GhostButton(if (clearConfirm) "确认清空" else "清空", {
                if (clearConfirm) {
                    model.clearHistory()
                    clearConfirm = false
                } else {
                    clearConfirm = true
                }
            }, if (clearConfirm) C.Red else C.Ink)
        }

        Section("设备组", "${ov.devices.size} 台")
        SettingRow("退出设备组", "通知其他设备把本机移出，然后删除本机的设备身份与剪贴板历史，回到初始设置。本机设置与收到的文件保留。") {
            GhostButton(if (leaveConfirm) "确认退出" else "退出", {
                if (leaveConfirm) {
                    model.leaveGroup()
                    leaveConfirm = false
                } else {
                    leaveConfirm = true
                }
            }, if (leaveConfirm) C.Red else C.Ink)
        }

        Section("卸载", "UNINSTALL")
        SettingRow(
            "清除全部数据",
            "卸载前请先在这里清除：守护进程的数据保存在 App 目录之外，直接卸载不会删除。已经直接卸载的，重新安装后再来这里清除。",
        ) {
            GhostButton("清除", onWipe, C.Red, icon = Icons.Trash)
        }
        Spacer(Modifier.height(40.dp))
    }
}

@Composable
private fun Section(title: String, aside: String) {
    Spacer(Modifier.height(10.dp))
    Row(Modifier.fillMaxWidth().padding(vertical = 10.dp)) {
        Text(title, style = T.label)
        Spacer(Modifier.weight(1f))
        Text(aside, style = T.mono)
    }
    Rule(2.dp)
}

@Composable
private fun SettingRow(title: String, desc: String, below: (@Composable () -> Unit)? = null, control: @Composable () -> Unit) {
    Column {
        Row(Modifier.fillMaxWidth().padding(top = 16.dp, bottom = if (below == null) 16.dp else 0.dp), verticalAlignment = Alignment.CenterVertically) {
            Column(Modifier.weight(1f).padding(end = 16.dp)) {
                Text(title, style = text(17.sp, 900))
                Spacer(Modifier.height(4.dp))
                Text(desc, style = T.bodyMute)
            }
            control()
        }
        if (below != null) Box(Modifier.padding(bottom = 16.dp)) { below() }
        Rule()
    }
}

@Composable
private fun PairSheet(model: AppModel, req: PairRequest) {
    val y = remember { Animatable(600f) }
    val dim = remember { Animatable(0f) }
    val scope = rememberCoroutineScope()
    var busy by remember { mutableStateOf(false) }
    LaunchedEffect(req) {
        launch { dim.animateTo(1f, S.gentle()) }
        y.animateTo(0f, S.bouncy())
    }
    fun close(accept: Boolean) {
        busy = true
        scope.launch {
            launch { dim.animateTo(0f, S.snappy()) }
            y.animateTo(700f, S.snappy())
            model.respondPairing(accept, req.name)
        }
    }
    BackHandler { if (!busy) close(false) }
    Box(Modifier.fillMaxSize().graphicsLayer { alpha = dim.value }.background(C.Ink.copy(alpha = 0.35f)))
    Box(Modifier.fillMaxSize(), contentAlignment = Alignment.BottomCenter) {
        Column(
            Modifier
                .graphicsLayer { translationY = y.value * density }
                .fillMaxWidth()
                .background(C.Paper)
                .windowInsetsPadding(WindowInsets.navigationBars)
                .padding(Pad)
        ) {
            Row(Modifier.fillMaxWidth()) {
                Text("配对请求", style = T.label)
                Spacer(Modifier.weight(1f))
                Text(if (req.platform == "android") "ANDROID" else "WINDOWS", style = T.mono)
            }
            Spacer(Modifier.height(4.dp))
            Rule(2.dp)
            Spacer(Modifier.height(14.dp))
            Text("“${req.name}”请求加入", style = text(28.sp, 900, line = 34.sp))
            Spacer(Modifier.height(10.dp))
            Text("请确认对方屏幕上显示的确认码与下方完全一致。不一致时请拒绝——这可能意味着有人在冒充。", style = T.bodyMute)
            Spacer(Modifier.height(18.dp))
            Text("${req.sas.take(3)} ${req.sas.drop(3)}", style = text(64.sp, 600, F.Mono, (-0.02).em, color = C.Red))
            Spacer(Modifier.height(22.dp))
            PrimaryButton("确认一致，同意加入", { if (!busy) close(true) }, enabled = !busy, modifier = Modifier.fillMaxWidth())
            Spacer(Modifier.height(12.dp))
            GhostButton("拒绝", { if (!busy) close(false) }, modifier = Modifier.fillMaxWidth())
        }
    }
}

@Composable
private fun JoinWaiting(sas: String?) {
    Column(Modifier.fillMaxWidth()) {
        AnimatedContent(
            targetState = sas,
            transitionSpec = { (slideInVertically(S.bouncy()) { it / 3 } + fadeIn(S.gentle())) togetherWith fadeOut(S.snappy()) },
            label = "sas",
        ) { code ->
            Column(Modifier.fillMaxWidth()) {
                if (code == null) {
                    Text("正在连接邀请方…", style = text(22.sp, 900))
                    Spacer(Modifier.height(10.dp))
                    Text("同一局域网内直接连接，否则经中转服务器，通常几秒钟。", style = T.bodyMute)
                } else {
                    Text("确认码", style = T.label)
                    Spacer(Modifier.height(6.dp))
                    Text("${code.take(3)} ${code.drop(3)}", style = text(72.sp, 600, F.Mono, (-0.02).em, 80.sp, C.Red))
                    Spacer(Modifier.height(12.dp))
                    Text("请在邀请方设备上核对这 6 位数字，一致后点“同意加入”。不一致请在那边拒绝。", style = T.body)
                }
            }
        }
        Spacer(Modifier.height(18.dp))
        val w = remember { Animatable(0f) }
        LaunchedEffect(Unit) {
            while (true) {
                w.animateTo(1f, S.rule())
                w.animateTo(0f, S.rule())
            }
        }
        Box(Modifier.fillMaxWidth().height(4.dp).background(C.Faint)) {
            Box(Modifier.fillMaxWidth(w.value.coerceIn(0.02f, 1f)).height(4.dp).background(C.Red))
        }
    }
}

@Composable
private fun Onboarding(model: AppModel, ov: Overview, invite: String?, onInviteConsumed: () -> Unit, onScan: () -> Unit, onWipe: () -> Unit) {
    val ctx = LocalContext.current
    val scope = rememberCoroutineScope()
    val sas by model.joinSas.collectAsState()
    var join by rememberSaveable { mutableStateOf(true) }
    var name by rememberSaveable { mutableStateOf(ov.settings.deviceName) }
    var uri by rememberSaveable { mutableStateOf("") }
    var busy by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }
    val shake = remember { Animatable(0f) }

    fun submit() {
        busy = true
        error = null
        scope.launch {
            error = if (join) model.joinGroup(uri.trim(), name.trim()) else model.createGroup(name.trim())
            busy = false
        }
    }

    LaunchedEffect(invite) {
        if (invite != null) {
            uri = invite
            join = true
            if (name.isNotBlank() && !busy) submit()
            onInviteConsumed()
        }
    }
    LaunchedEffect(error) {
        if (error != null) {
            shake.snapTo(18f)
            shake.animateTo(0f, S.wobbly())
        }
    }

    Column(Modifier.fillMaxSize().imePadding().verticalScroll(rememberScrollState()).padding(horizontal = Pad)) {
        Row(Modifier.fillMaxWidth().padding(top = 8.dp)) {
            Text("欢迎", style = T.label)
            Spacer(Modifier.weight(1f))
            Text("01 / 设置", style = T.mono)
        }
        Spacer(Modifier.height(14.dp))
        Text("CopyCat", style = text(68.sp, 900, F.Grotesk, (-0.05).em, 70.sp))
        Spacer(Modifier.height(8.dp))
        Box(Modifier.fillMaxWidth(0.72f).height(8.dp).background(C.Red))
        Spacer(Modifier.height(16.dp))
        Text("在所有设备之间，\n同步剪贴板。", style = text(24.sp, 900, line = 31.sp))
        Spacer(Modifier.height(30.dp))

        Row(Modifier.fillMaxWidth()) {
            for ((label, j) in listOf("加入设备组" to true, "创建设备组" to false)) {
                val on = join == j
                val bar by animateFloatAsState(if (on) 1f else 0f, S.bouncy(), label = "tab")
                Column(Modifier.pressable({ if (!busy) join = j }).padding(end = 22.dp)) {
                    Text(label, style = text(18.sp, 900, color = if (on) C.Ink else C.Mute))
                    Spacer(Modifier.height(8.dp))
                    Box(Modifier.width(90.dp * bar).height(4.dp).background(C.Red))
                }
            }
        }
        Rule(2.dp)
        Spacer(Modifier.height(22.dp))

        if (join && busy) {
            JoinWaiting(sas)
        } else {
            Column(Modifier.graphicsLayer { translationX = shake.value * density }) {
                Field("本机名称", name, { name = it }, "例如：我的手机")
                if (join) {
                    GhostButton(
                        "扫描电脑上的二维码", { if (name.isNotBlank()) onScan() else error = "请先填写本机名称" },
                        icon = Icons.Scan, modifier = Modifier.fillMaxWidth(),
                    )
                    Spacer(Modifier.height(10.dp))
                    Text("在已加入的设备上点“添加设备”，扫码后会自动发起加入。", style = T.bodyMute)
                    Spacer(Modifier.height(26.dp))
                    Field("或粘贴配对码", uri, { uri = it }, "copycat://pair?…", mono = true, minLines = 3)
                    TextAction("从剪贴板粘贴", Icons.Paste, {
                        val cm = ctx.getSystemService(ClipboardManager::class.java)
                        cm.primaryClip?.getItemAt(0)?.text?.toString()?.let { uri = it.trim() }
                    })
                } else {
                    Text(
                        "在本机创建设备组，不需要服务器。之后在“设备”页添加设备，同一局域网内的电脑或手机扫码即可加入。" +
                            "通常在电脑上创建更方便：需要跨网络同步时，在电脑端生成中转服务器的部署密钥。",
                        style = T.body,
                    )
                }
            }

        }
        if (error != null) {
            Spacer(Modifier.height(12.dp))
            Text(error!!, style = text(14.sp, 700, color = C.Red, line = 20.sp))
        }
        Spacer(Modifier.height(22.dp))
        val ready = name.isNotBlank() && (!join || uri.isNotBlank())
        PrimaryButton(
            if (busy) (if (join) "等待对方同意…" else "正在创建…") else if (join) "加入" else "创建设备组",
            ::submit,
            enabled = ready && !busy,
            icon = Icons.ArrowRight,
        )
        Spacer(Modifier.height(36.dp))
        Rule()
        Spacer(Modifier.height(6.dp))
        TextAction("卸载前清除本机数据", Icons.Trash, { if (!busy) onWipe() })
        Spacer(Modifier.height(32.dp))
    }
}

@Composable
internal fun JoinWaitingPreview(sas: String?) {
    Box(Modifier.fillMaxSize().background(C.Paper).padding(Pad)) { JoinWaiting(sas) }
}
