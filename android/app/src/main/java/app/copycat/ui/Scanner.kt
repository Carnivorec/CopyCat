package app.copycat.ui

import android.Manifest
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.provider.Settings
import android.util.Size as PixelSize
import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.PickVisualMediaRequest
import androidx.activity.result.contract.ActivityResultContracts
import androidx.camera.compose.CameraXViewfinder
import androidx.camera.core.CameraSelector
import androidx.camera.core.ImageAnalysis
import androidx.camera.core.Preview
import androidx.camera.core.SurfaceRequest
import androidx.camera.core.resolutionselector.ResolutionSelector
import androidx.camera.core.resolutionselector.ResolutionStrategy
import androidx.camera.lifecycle.ProcessCameraProvider
import androidx.camera.lifecycle.awaitInstance
import androidx.camera.viewfinder.core.ImplementationMode
import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.background
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
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.systemBars
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.zIndex
import androidx.core.content.ContextCompat
import androidx.lifecycle.compose.LifecycleResumeEffect
import androidx.lifecycle.compose.LocalLifecycleOwner
import app.copycat.Qr
import java.util.concurrent.Executors
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.awaitCancellation
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

internal enum class Cam { Asking, Denied, On }

private val Dim = Color(0xFFB5B5B5)

@Composable
internal fun ScanSheet(onResult: (String) -> Unit, onClose: () -> Unit) {
    val ctx = LocalContext.current
    val owner = LocalLifecycleOwner.current
    val haptic = LocalHapticFeedback.current
    val scope = rememberCoroutineScope()
    fun granted() = ContextCompat.checkSelfPermission(ctx, Manifest.permission.CAMERA) == PackageManager.PERMISSION_GRANTED

    var cam by remember { mutableStateOf(if (granted()) Cam.On else Cam.Asking) }
    var request by remember { mutableStateOf<SurfaceRequest?>(null) }
    var hint by remember { mutableStateOf<String?>(null) }
    var done by remember { mutableStateOf(false) }
    val y = remember { Animatable(1f) }
    val lock = remember { Animatable(0f) }
    val found = remember { Channel<String>(Channel.CONFLATED) }

    val ask = rememberLauncherForActivityResult(ActivityResultContracts.RequestPermission()) { ok ->
        cam = if (ok) Cam.On else Cam.Denied
    }
    val pick = rememberLauncherForActivityResult(ActivityResultContracts.PickVisualMedia()) { uri ->
        if (uri != null) scope.launch {
            val text = withContext(Dispatchers.Default) { runCatching { Qr.decode(ctx.contentResolver, uri) }.getOrNull() }
            if (text == null) hint = "图片里没有找到二维码" else found.trySend(text)
        }
    }

    fun close(result: String?) {
        done = true
        scope.launch {
            y.animateTo(1f, S.snappy())
            if (result != null) onResult(result) else onClose()
        }
    }

    LaunchedEffect(Unit) {
        if (cam == Cam.Asking) ask.launch(Manifest.permission.CAMERA)
        y.animateTo(0f, S.gentle())
    }

    LifecycleResumeEffect(Unit) {
        if (cam == Cam.Denied && granted()) cam = Cam.On
        onPauseOrDispose {}
    }
    LaunchedEffect(hint) {
        if (hint != null) {
            delay(2600)
            hint = null
        }
    }
    LaunchedEffect(Unit) {
        for (text in found) {
            if (done) continue
            if (!Qr.isInvite(text)) {
                hint = "这不是 CopyCat 配对码"
                continue
            }
            done = true
            haptic.performHapticFeedback(HapticFeedbackType.Confirm)
            lock.animateTo(1f, S.bouncy())
            close(text)
        }
    }
    LaunchedEffect(cam) {
        if (cam != Cam.On) return@LaunchedEffect
        val provider = ProcessCameraProvider.awaitInstance(ctx)
        val res = ResolutionSelector.Builder()
            .setResolutionStrategy(ResolutionStrategy(PixelSize(1280, 720), ResolutionStrategy.FALLBACK_RULE_CLOSEST_HIGHER_THEN_LOWER))
            .build()
        val preview = Preview.Builder().setResolutionSelector(res).build().also { it.setSurfaceProvider { r -> request = r } }
        val analysis = ImageAnalysis.Builder()
            .setResolutionSelector(res)
            .setBackpressureStrategy(ImageAnalysis.STRATEGY_KEEP_ONLY_LATEST)
            .build()
        val exec = Executors.newSingleThreadExecutor()
        analysis.setAnalyzer(exec) { img -> img.use { Qr.decode(it) }?.let { found.trySend(it) } }
        try {
            provider.bindToLifecycle(owner, CameraSelector.DEFAULT_BACK_CAMERA, preview, analysis)
            awaitCancellation()
        } catch (e: Exception) {
            if (e is CancellationException) throw e
            hint = "无法打开相机，可以改用相册里的截图"
        } finally {
            provider.unbind(preview, analysis)
            analysis.clearAnalyzer()
            exec.shutdown()
        }
    }

    BackHandler { if (!done) close(null) }
    ScanFrame(
        cam = cam,
        hint = hint,
        lock = lock.value,
        modifier = Modifier.graphicsLayer { translationY = y.value * size.height },
        onClose = { if (!done) close(null) },
        onGallery = { pick.launch(PickVisualMediaRequest(ActivityResultContracts.PickVisualMedia.ImageOnly)) },
        onSettings = {
            ctx.startActivity(Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.fromParts("package", ctx.packageName, null)))
        },
    ) {
        request?.let { CameraXViewfinder(it, Modifier.fillMaxSize(), implementationMode = ImplementationMode.EMBEDDED) }
    }
}

@Composable
internal fun ScanFrame(
    cam: Cam,
    hint: String?,
    lock: Float,
    modifier: Modifier = Modifier,
    onClose: () -> Unit = {},
    onGallery: () -> Unit = {},
    onSettings: () -> Unit = {},
    camera: @Composable () -> Unit,
) {
    BoxWithConstraints(modifier.fillMaxSize().background(C.Ink)) {
        val win = minOf(maxWidth * 0.74f, 300.dp)
        if (cam == Cam.On) camera()
        Column(Modifier.fillMaxSize().windowInsetsPadding(WindowInsets.systemBars)) {
            Row(Modifier.fillMaxWidth().padding(start = Pad, top = 6.dp, end = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                Box(Modifier.size(11.dp).background(C.Red))
                Spacer(Modifier.width(9.dp))
                Text("扫描配对码", style = text(15.sp, 800, color = C.Paper))
                Spacer(Modifier.weight(1f))
                Box(Modifier.pressable(onClose).padding(14.dp)) { Icon(Icons.X, 22.dp, C.Paper) }
            }
            Spacer(Modifier.height(14.dp))
            Text("对准电脑上的\n二维码", style = text(30.sp, 900, line = 36.sp, color = C.Paper), modifier = Modifier.padding(horizontal = Pad))
            Spacer(Modifier.height(10.dp))
            Text("在电脑端“设备”页点“添加设备”即可显示。", style = text(13.sp, line = 20.sp, color = Dim), modifier = Modifier.padding(horizontal = Pad))
            Spacer(Modifier.weight(1f))
            Viewfinder(cam, lock, Modifier.align(Alignment.CenterHorizontally).size(win).zIndex(-1f), onSettings)
            Spacer(Modifier.height(18.dp))
            Box(Modifier.align(Alignment.CenterHorizontally).height(28.dp)) {
                AnimatedContent(
                    targetState = hint,
                    transitionSpec = { (slideInVertically(S.bouncy()) { it / 2 } + fadeIn(S.gentle())) togetherWith fadeOut(S.snappy()) },
                    label = "hint",
                ) { h -> Text(h ?: "", style = text(14.sp, 700, color = C.Red)) }
            }
            Spacer(Modifier.weight(1.2f))
            Row(Modifier.fillMaxWidth().padding(horizontal = Pad).padding(bottom = 18.dp)) {
                TextAction("从相册选择截图", Icons.Image, onGallery, color = C.Paper)
            }
        }
    }
}

@Composable
private fun Viewfinder(cam: Cam, lock: Float, modifier: Modifier, onSettings: () -> Unit) {
    val sweep = remember { Animatable(0f) }
    LaunchedEffect(cam) {
        if (cam != Cam.On) return@LaunchedEffect
        while (true) {
            sweep.animateTo(1f, S.rule())
            sweep.animateTo(0f, S.rule())
        }
    }
    BoxWithConstraints(
        modifier.drawBehind {
            val mask = Color.Black.copy(alpha = 0.62f)
            val far = 10_000f
            drawRect(mask, Offset(-far, -far), Size(far * 2 + size.width, far))
            drawRect(mask, Offset(-far, size.height), Size(far * 2 + size.width, far))
            drawRect(mask, Offset(-far, 0f), Size(far, size.height))
            drawRect(mask, Offset(size.width, 0f), Size(far, size.height))

            val t = 4.dp.toPx()
            val len = 30.dp.toPx()
            val inset = lock * 12.dp.toPx()
            for ((x, y) in listOf(0f to 0f, 1f to 0f, 0f to 1f, 1f to 1f)) {
                val cx = if (x == 0f) inset else size.width - inset
                val cy = if (y == 0f) inset else size.height - inset
                val sx = if (x == 0f) 1f else -1f
                val sy = if (y == 0f) 1f else -1f
                drawRect(C.Red, Offset(minOf(cx, cx + sx * len), minOf(cy, cy + sy * t)), Size(len, t))
                drawRect(C.Red, Offset(minOf(cx, cx + sx * t), minOf(cy, cy + sy * len)), Size(t, len))
            }
        }
    ) {
        when (cam) {
            Cam.On -> if (lock == 0f) {
                val margin = 14.dp
                Box(
                    Modifier
                        .padding(horizontal = margin)
                        .graphicsLayer { translationY = margin.toPx() + sweep.value * (maxHeight - margin * 2).toPx() }
                        .fillMaxWidth()
                        .height(2.dp)
                        .background(C.Red)
                )
            }
            Cam.Denied -> Column(Modifier.fillMaxSize().padding(22.dp), verticalArrangement = Arrangement.Center) {
                Text("需要相机权限", style = text(20.sp, 900, color = C.Paper))
                Spacer(Modifier.height(8.dp))
                Text("只用于扫描配对二维码。也可以改用相册里的截图，或返回粘贴配对码。", style = text(13.sp, line = 20.sp, color = Dim))
                Spacer(Modifier.height(16.dp))
                GhostButton("去系统设置开启", onSettings, color = C.Paper)
            }
            Cam.Asking -> {}
        }
    }
}
