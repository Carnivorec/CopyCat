package app.copycat.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.size
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.unit.dp
import androidx.compose.ui.test.onNodeWithText
import androidx.compose.ui.test.onRoot
import androidx.compose.ui.test.hasScrollAction
import androidx.compose.ui.test.hasText
import androidx.compose.ui.test.performClick
import androidx.compose.ui.test.performScrollToNode
import androidx.compose.ui.test.performScrollTo
import androidx.compose.ui.test.performTouchInput
import androidx.compose.ui.test.swipeUp
import androidx.test.core.app.ApplicationProvider
import app.copycat.AppModel
import app.copycat.Clip
import app.copycat.Device
import app.copycat.Overview
import app.copycat.PairRequest
import app.copycat.Payload
import app.copycat.Phase
import app.copycat.Relay
import app.copycat.Settings
import app.copycat.Transfer
import android.net.Uri
import com.github.takahirom.roborazzi.captureRoboImage
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode

@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [36], qualifiers = "w411dp-h905dp-xxhdpi")
class ScreenshotTest {
    @get:Rule
    val compose = createComposeRule()

    private val now = System.currentTimeMillis()
    private val min = 60_000L

    private val settings = Settings(
        deviceName = "一加 13", syncImages = true, lanEnabled = true,
        mailbox = true, relayEnabled = true, excludedApps = listOf("com.x8bit.bitwarden", "keepass2android.keepass2android"), screenshots = true,
    )

    private val devices = listOf(
        Device("me", "q2kd8fha", "一加 13", "android", me = true, relay = true, lan = false),
        Device("pc", "c4y7imnr", "台式机", "windows", me = false, relay = true, lan = true),
        Device("nb", "m3n0pwla", "笔记本", "windows", me = false, relay = true, lan = false),
    )

    private fun clip(id: String, text: String?, kind: String, device: String, mine: Boolean, ago: Long, image: Boolean = false) =
        Clip(id, device, if (mine) "android" else "windows", mine, now - ago * min, kind, text, text?.length ?: 0, image, (text?.length ?: 1_200_000).toLong())

    private val history = listOf(
        clip("01", "482913", "otp", "台式机", false, 1),
        clip("02", "会议纪要：周五 14:00，三楼会议室，记得带上投影转接头和打印好的季度报表", "text", "一加 13", true, 7),
        clip("03", "https://github.com/Carnivorec/CopyCat/releases", "link", "笔记本", false, 14),
        clip("04", "cargo test --workspace", "code", "台式机", false, 21),
        clip("05", null, "image", "一加 13", true, 28, image = true),
        clip("06", "Zürich · Basel · Bern", "text", "笔记本", false, 35),
        clip("07", "今晚七点，老地方见", "text", "一加 13", true, 42),
        clip("08", "快递取件码 6-3-2107，菜鸟驿站东门", "text", "一加 13", true, 49),
    )

    private val mb = 1024L * 1024

    private fun transfer(id: String, outgoing: Boolean, state: String, files: List<Pair<String, Long>>, done: Long, ago: Long, text: String? = null, error: String? = null, kind: String = "files") =
        Transfer(
            id, if (outgoing) "pc" else "pc", "台式机", outgoing, kind, files, files.size, text, files.sumOf { it.second } + (text?.length ?: 0),
            done, if (state == "active") 9 * mb else 0, state, error, lan = true, dir = "/storage/emulated/0/Download/CopyCat",
            saved = emptyList(), time = now - ago * min, finished = if (state in setOf("done", "failed", "cancelled")) now - ago * min else null,
        )

    private val transfers = listOf(
        transfer("t1", false, "active", listOf("季度报告/正文.docx" to 2 * mb, "季度报告/附图.png" to 300 * mb), 121 * mb, 0),
        transfer("t2", true, "done", emptyList(), 40, 3, text = "看看这个 https://example.com/xYz 很有意思"),
        transfer("t5", false, "done", listOf("截图_20261005_141502.png" to 3 * mb), 3 * mb, 5, kind = "screenshot"),
        transfer("t6", false, "done", listOf("旅行照片/IMG_1.jpg" to 2 * mb, "报价单.pdf" to mb), 3 * mb, 7).copy(
            saved = listOf("/storage/emulated/0/Download/CopyCat/旅行照片", "/storage/emulated/0/Download/CopyCat/报价单.pdf"),
            savedDirs = listOf(true, false),
        ),
        transfer("t3", true, "done", listOf("IMG_20261005_141233.jpg" to 4 * mb), 4 * mb, 9),
        transfer("t4", false, "failed", listOf("安装包.exe" to 96 * mb), 12 * mb, 50, error = "对方取消了发送"),
    )

    private val ov = Overview(
        initialized = true, me = devices[0], relay = Relay("relay.example.com:8443", configured = true, connected = true, enabled = true),
        devices = devices, history = history, settings = settings, paused = false, lanPort = 47821,
        transfers = transfers, receiveDir = "/storage/emulated/0/Download/CopyCat",
    )

    private fun shot(
        name: String,
        phase: Phase = Phase.Ready,
        ov: Overview? = this.ov,
        pair: PairRequest? = null,
        page: Page = Page.Clipboard,
        revoked: Boolean = false,
        confirmWipe: Boolean = false,
    ) {
        val model = AppModel(ApplicationProvider.getApplicationContext())
        compose.setContent {
            RootContent(model, phase, ov, pair, revoked, pendingInvite = null, onInviteConsumed = {}, initialPage = page, confirmWipe = confirmWipe)
        }
        compose.waitForIdle()
        compose.onRoot().captureRoboImage("build/outputs/roborazzi/$name.png")
    }

    @Test fun clipboard() = shot("clipboard")
    @Test fun devices() = shot("devices", page = Page.Devices)
    @Test fun settings() = shot("settings", page = Page.Settings)

    @Test fun transfersExpanded() {
        val model = AppModel(ApplicationProvider.getApplicationContext())
        compose.setContent { RootContent(model, Phase.Ready, ov, null, revoked = false, pendingInvite = null, onInviteConsumed = {}, initialPage = Page.Transfer) }
        compose.onNode(hasScrollAction()).performScrollToNode(hasText("IMG_1.jpg", substring = true))
        compose.onNodeWithText("IMG_1.jpg", substring = true).performClick()
        compose.waitForIdle()
        compose.onRoot().captureRoboImage("build/outputs/roborazzi/transfers-expanded.png")
    }

    @Test fun settingsHistory() {
        val model = AppModel(ApplicationProvider.getApplicationContext())
        compose.setContent { RootContent(model, Phase.Ready, ov, null, revoked = false, pendingInvite = null, onInviteConsumed = {}, initialPage = Page.Settings) }
        compose.onNodeWithText("清空历史").performScrollTo()
        compose.waitForIdle()
        compose.onRoot().captureRoboImage("build/outputs/roborazzi/settings-history.png")
    }

    @Test fun settingsScreenshots() {
        val model = AppModel(ApplicationProvider.getApplicationContext())
        compose.setContent { RootContent(model, Phase.Ready, ov, null, revoked = false, pendingInvite = null, onInviteConsumed = {}, initialPage = Page.Settings) }
        compose.onNodeWithText("5 GB").performScrollTo()
        compose.waitForIdle()
        compose.onRoot().captureRoboImage("build/outputs/roborazzi/settings-screenshots.png")
    }

    @Config(qualifiers = "w360dp-h780dp-xxhdpi")
    @Test fun settingsScreenshotsNarrow() {
        val model = AppModel(ApplicationProvider.getApplicationContext())
        val o = ov.copy(settings = ov.settings.copy(screenshots = true, screenshotKeepDays = 1, screenshotMaxMb = 1024))
        compose.setContent { RootContent(model, Phase.Ready, o, null, revoked = false, pendingInvite = null, onInviteConsumed = {}, initialPage = Page.Settings) }
        compose.onNodeWithText("5 GB").performScrollTo()
        compose.waitForIdle()
        compose.onRoot().captureRoboImage("build/outputs/roborazzi/settings-screenshots-360.png")
    }
    @Test fun onboarding() = shot("onboarding", ov = ov.copy(initialized = false, devices = emptyList(), history = emptyList(), relay = null, me = null))

    private val lanOnly = ov.copy(relay = Relay("", configured = false, connected = false, enabled = true))
    @Test fun settingsLanOnly() = shot("settings-lanonly", ov = lanOnly, page = Page.Settings)
    @Test fun devicesLanOnly() = shot("devices-lanonly", ov = lanOnly, page = Page.Devices)

    @Test fun onboardingCreate() {
        val model = AppModel(ApplicationProvider.getApplicationContext())
        val o = ov.copy(initialized = false, devices = emptyList(), history = emptyList(), relay = null, me = null)
        compose.setContent { RootContent(model, Phase.Ready, o, null, revoked = false, pendingInvite = null, onInviteConsumed = {}, initialPage = Page.Clipboard) }
        compose.onNodeWithText("创建设备组").performClick()
        compose.waitForIdle()
        compose.onRoot().captureRoboImage("build/outputs/roborazzi/onboarding-create.png")
    }
    @Test fun emptyHistory() = shot("empty", ov = ov.copy(history = emptyList(), devices = devices.take(1)))
    @Test fun pairSheet() = shot("pair", pair = PairRequest("小米平板", "android", "305718"))
    @Test fun noRoot() = shot("noroot", phase = Phase.NoRoot)

    private fun scrolledShot(name: String, text: String, ov: Overview = this.ov, page: Page = Page.Settings) {
        val model = AppModel(ApplicationProvider.getApplicationContext())
        compose.setContent { RootContent(model, Phase.Ready, ov, null, revoked = false, pendingInvite = null, onInviteConsumed = {}, initialPage = page) }
        compose.onNodeWithText(text).performScrollTo()
        compose.waitForIdle()
        compose.onRoot().captureRoboImage("build/outputs/roborazzi/$name.png")
    }
    @Test fun settingsUninstall() = scrolledShot("settings-uninstall", "清除")
    @Config(qualifiers = "w360dp-h780dp-xxhdpi")
    @Test fun settingsUninstallNarrow() = scrolledShot("settings-uninstall-360", "清除")

    @Config(qualifiers = "w360dp-h780dp-xxhdpi")
    @Test fun settingsBottomNarrow() {
        val model = AppModel(ApplicationProvider.getApplicationContext())
        compose.setContent { RootContent(model, Phase.Ready, ov, null, revoked = false, pendingInvite = null, onInviteConsumed = {}, initialPage = Page.Settings) }
        repeat(12) { compose.onNode(hasScrollAction()).performTouchInput { swipeUp() } }
        compose.waitForIdle()
        compose.onRoot().captureRoboImage("build/outputs/roborazzi/settings-bottom-360.png")
    }
    @Test fun onboardingWipe() =
        scrolledShot("onboarding-wipe", "卸载前清除本机数据", ov.copy(initialized = false, devices = emptyList(), history = emptyList(), relay = null, me = null))
    @Test fun wipeConfirm() = shot("wipe-confirm", page = Page.Settings, confirmWipe = true)
    @Config(qualifiers = "w360dp-h780dp-xxhdpi")
    @Test fun wipeConfirmNarrow() = shot("wipe-confirm-360", page = Page.Settings, confirmWipe = true)
    @Test fun wiped() = shot("wiped", phase = Phase.Wiped, ov = null)
    @Config(qualifiers = "w360dp-h780dp-xxhdpi")
    @Test fun wipedNarrow() = shot("wiped-360", phase = Phase.Wiped, ov = null)
    @Test fun wipeFailed() = shot("wipe-failed", phase = Phase.WipeFailed("守护进程无法停止，没有删除任何文件"), ov = null)
    @Test fun daemonFailed() = shot("daemon-failed", phase = Phase.Failed("守护进程没有响应，请查看 /data/local/copycat/daemon.log"), ov = null)
    @Test fun revoked() = shot("revoked", revoked = true)
    @Config(qualifiers = "w360dp-h780dp-xxhdpi")
    @Test fun revokedNarrow() = shot("revoked-360", revoked = true)
    @Config(qualifiers = "w360dp-h780dp-xxhdpi")
    @Test fun daemonFailedNarrow() = shot("daemon-failed-360", phase = Phase.Failed("守护进程没有响应，请查看 /data/local/copycat/daemon.log"), ov = null)
    @Test fun transfers() = shot("transfers", page = Page.Transfer)
    @Test fun transfersEmpty() = shot("transfers-empty", ov = ov.copy(transfers = emptyList()), page = Page.Transfer)

    private val files = Payload.Files(
        listOf(
            Payload.Item(Uri.EMPTY, "IMG_20261005_141233.jpg", 4_210_000),
            Payload.Item(Uri.EMPTY, "VID_20261005_141502.mp4", 182_400_000),
            Payload.Item(Uri.EMPTY, "会议记录.pdf", 812_000),
            Payload.Item(Uri.EMPTY, "截图.png", 312_000),
        ),
    )

    private fun shareShot(name: String, payload: Payload, state: ShareState) {
        compose.setContent { ShareSheet(payload, state, onSend = {}, onClose = {}) }
        compose.waitForIdle()
        compose.onRoot().captureRoboImage("build/outputs/roborazzi/$name.png")
    }

    @Test fun shareFiles() = shareShot("share-files", files, ShareState.Pick(ov))
    @Test fun shareMulti() = shareShot("share-multi", files, ShareState.Pick(ov, selected = setOf("pc", "nb")))
    @Test fun shareConfirm() = shareShot("share-confirm", files, ShareState.Pick(ov, selected = setOf("nb"), confirming = true))
    @Test fun shareText() = shareShot("share-text", Payload.Text("【这个视频太好笑了】 https://example.com/xYz9aBc 复制此链接，打开哔哩哔哩"), ShareState.Pick(ov))
    @Test fun shareSent() = shareShot("share-sent", files, ShareState.Sent("台式机"))
    @Test fun shareError() = shareShot("share-error", files, ShareState.Error("这台手机还没有加入设备组，请先打开 CopyCat 完成配对"))

    @Config(qualifiers = "w360dp-h780dp-xxhdpi")
    @Test fun shareNarrow() = shareShot("share-files-360", files, ShareState.Pick(ov))

    @Config(qualifiers = "w360dp-h780dp-xxhdpi")
    @Test fun transfersNarrow() = shot("transfers-360", page = Page.Transfer)

    @Test fun joinWaiting() {

        compose.mainClock.autoAdvance = false
        compose.setContent { JoinWaitingPreview("719191") }
        compose.mainClock.advanceTimeBy(1500)
        compose.onRoot().captureRoboImage("build/outputs/roborazzi/join-sas.png")
    }

    private fun scanShot(name: String, cam: Cam, hint: String? = null, lock: Float = 0f) {
        compose.mainClock.autoAdvance = false
        compose.setContent {
            ScanFrame(cam, hint, lock) {
                Box(Modifier.fillMaxSize().background(Color(0xFF5E6266)), contentAlignment = Alignment.Center) {
                    QrMatrix(fakeQr, Modifier.size(250.dp).offset(y = 30.dp))
                }
            }
        }
        compose.mainClock.advanceTimeBy(700)
        compose.onRoot().captureRoboImage("build/outputs/roborazzi/$name.png")
    }

    private val fakeQr = List(29) { y -> String(CharArray(29) { x -> if ((x * 7 + y * 13 + x * y) % 5 < 2 || (x < 7 && y < 7) || (x > 21 && y < 7) || (x < 7 && y > 21)) '1' else '0' }) }

    @Test fun scan() = scanShot("scan", Cam.On)
    @Test fun scanHint() = scanShot("scan-hint", Cam.On, hint = "这不是 CopyCat 配对码")
    @Test fun scanLocked() = scanShot("scan-locked", Cam.On, lock = 1f)
    @Test fun scanDenied() = scanShot("scan-denied", Cam.Denied)

    @Config(qualifiers = "w360dp-h780dp-xxhdpi")
    @Test fun scanNarrow() = scanShot("scan-360", Cam.On)

    @Config(qualifiers = "w360dp-h780dp-xxhdpi")
    @Test fun scanDeniedNarrow() = scanShot("scan-denied-360", Cam.Denied)

    @Config(qualifiers = "w360dp-h780dp-xxhdpi")
    @Test fun clipboardNarrow() = shot("clipboard-360")

    @Config(qualifiers = "w360dp-h780dp-xxhdpi")
    @Test fun settingsNarrow() = shot("settings-360", page = Page.Settings)

    @Config(qualifiers = "w360dp-h780dp-xxhdpi")
    @Test fun onboardingNarrow() = shot("onboarding-360", ov = ov.copy(initialized = false, history = emptyList()))
}
