package app.copycat.ui

import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectHorizontalDragGestures
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsPressedAsState
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.wrapContentHeight
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.graphics.vector.rememberVectorPainter
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.input.pointer.util.VelocityTracker
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.foundation.Image
import androidx.compose.ui.graphics.ColorFilter
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.foundation.text.BasicText
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.launch
import kotlin.math.roundToInt

@Composable
fun Text(text: String, style: TextStyle = T.body, modifier: Modifier = Modifier, maxLines: Int = Int.MAX_VALUE, overflow: TextOverflow = TextOverflow.Clip) {
    BasicText(text, modifier, style, overflow = overflow, maxLines = maxLines)
}

@Composable
fun Icon(v: ImageVector, size: Dp = 18.dp, tint: Color = C.Ink, modifier: Modifier = Modifier) {
    Image(rememberVectorPainter(v), null, modifier.size(size), colorFilter = ColorFilter.tint(tint))
}

@Composable
fun Modifier.pressable(onClick: () -> Unit, enabled: Boolean = true): Modifier {
    val src = remember { MutableInteractionSource() }
    val pressed by src.collectIsPressedAsState()
    val scale by animateFloatAsState(if (pressed) 0.96f else 1f, if (pressed) S.snappy() else S.wobbly(), label = "press")
    return this
        .graphicsLayer { scaleX = scale; scaleY = scale }
        .clickable(src, null, enabled = enabled, onClick = onClick)
}

@Composable
fun Rule(thickness: Dp = 1.dp, color: Color = C.Ink, modifier: Modifier = Modifier) {
    Box(modifier.fillMaxWidth().height(thickness).background(color))
}

@Composable
fun Odometer(value: Int, style: TextStyle, modifier: Modifier = Modifier) {
    val digits = value.coerceAtLeast(0).toString()
    val h = with(LocalDensity.current) { (style.fontSize * 1.06f).toDp() }
    Row(modifier.clipToBounds()) {
        digits.forEachIndexed { i, ch ->
            key(digits.length - i) { Digit(ch - '0', style, h) }
        }
    }
}

@Composable
private fun key(k: Int, content: @Composable () -> Unit) = androidx.compose.runtime.key(k) { content() }

@Composable
private fun Digit(d: Int, style: TextStyle, h: Dp) {
    val pos = remember { Animatable(d.toFloat()) }
    LaunchedEffect(d) { pos.animateTo(d.toFloat(), S.bouncy()) }
    Box(Modifier.height(h).clipToBounds()) {

        Column(Modifier.wrapContentHeight(Alignment.Top, unbounded = true).graphicsLayer { translationY = -pos.value * h.toPx() }) {
            for (n in 0..9) Text("$n", style = style.copy(lineHeight = style.fontSize * 1.06f), modifier = Modifier.height(h), maxLines = 1)
        }
    }
}

@Composable
fun Segmented(options: List<Pair<String, Int>>, selected: Int, onSelect: (Int) -> Unit) {
    Row(Modifier.border(2.dp, C.Ink)) {
        options.forEachIndexed { i, (label, value) ->
            if (i > 0) Box(Modifier.width(2.dp).height(36.dp).background(C.Ink))
            val on = value == selected
            Box(
                Modifier.height(36.dp).background(if (on) C.Ink else Color.White).pressable({ if (!on) onSelect(value) }).padding(horizontal = 12.dp),
                contentAlignment = Alignment.Center,
            ) {
                Text(label, style = T.strong.copy(color = if (on) Color.White else C.Ink))
            }
        }
    }
}

@Composable
fun Toggle(checked: Boolean, onChange: (Boolean) -> Unit, enabled: Boolean = true) {
    val x by animateFloatAsState(if (checked) 1f else 0f, S.bouncy(), label = "toggle")
    Box(
        Modifier
            .size(46.dp, 26.dp)
            .border(2.dp, if (enabled) C.Ink else C.Disabled)
            .pressable({ onChange(!checked) }, enabled)
            .padding(4.dp)
    ) {
        Box(
            Modifier
                .offset { IntOffset((x * 20.dp.toPx()).roundToInt(), 0) }
                .size(18.dp)
                .background(if (!enabled) C.Disabled else if (checked) C.Red else C.Ink)
        )
    }
}

@Composable
fun PrimaryButton(label: String, onClick: () -> Unit, enabled: Boolean = true, icon: ImageVector? = null, modifier: Modifier = Modifier) {
    val trailing = icon == Icons.ArrowRight
    Row(
        modifier
            .pressable(onClick, enabled)
            .background(if (enabled) C.Ink else C.Disabled)
            .padding(horizontal = 22.dp, vertical = 15.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        if (icon != null && !trailing) Icon(icon, 18.dp, C.Paper)
        Text(label, style = T.strong.copy(color = C.Paper))
        if (icon != null && trailing) Icon(icon, 18.dp, C.Paper)
    }
}

@Composable
fun GhostButton(label: String, onClick: () -> Unit, color: Color = C.Ink, icon: ImageVector? = null, modifier: Modifier = Modifier) {
    val trailing = icon == Icons.ArrowRight
    Row(
        modifier.pressable(onClick).border(2.dp, color).padding(horizontal = 20.dp, vertical = 13.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        if (icon != null && !trailing) Icon(icon, 18.dp, color)
        Text(label, style = T.strong.copy(color = color))
        if (icon != null && trailing) Icon(icon, 18.dp, color)
    }
}

@Composable
fun TextAction(label: String, icon: ImageVector, onClick: () -> Unit, color: Color = C.Ink) {
    Row(Modifier.pressable(onClick).padding(vertical = 8.dp), verticalAlignment = Alignment.CenterVertically) {
        Icon(icon, 17.dp, color)
        Spacer(Modifier.width(7.dp))
        Text(label, style = text(14.sp, 700, color = color))
    }
}

@Composable
fun Field(
    label: String,
    value: String,
    onChange: (String) -> Unit,
    hint: String = "",
    mono: Boolean = false,
    minLines: Int = 1,
    aside: String? = null,
    keyboard: KeyboardType = KeyboardType.Text,
) {
    Column(Modifier.fillMaxWidth().padding(bottom = 22.dp)) {
        Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.Bottom) {
            Text(label, style = T.label, modifier = Modifier.weight(1f))
            if (aside != null) Text(aside, style = text(11.sp, color = C.Mute))
        }
        Spacer(Modifier.height(8.dp))
        val style = if (mono) text(14.sp, 500, F.Mono, line = 21.sp) else text(17.sp, 500, line = 24.sp)
        Box {
            if (value.isEmpty()) Text(hint, style = style.copy(color = C.Mute), maxLines = minLines)
            BasicTextField(
                value, onChange,
                textStyle = style,
                minLines = minLines,
                maxLines = if (minLines > 1) 6 else 1,
                singleLine = minLines == 1,
                cursorBrush = SolidColor(C.Red),
                keyboardOptions = KeyboardOptions(keyboardType = keyboard),
                modifier = Modifier.fillMaxWidth(),
            )
        }
        Spacer(Modifier.height(8.dp))
        Rule(2.dp)
    }
}

@Composable
fun SwipeRow(onRemove: () -> Unit, onTap: () -> Unit, content: @Composable () -> Unit) {
    val x = remember { Animatable(0f) }
    val scope = rememberCoroutineScope()
    var width by remember { mutableStateOf(1f) }
    val pressScale = remember { Animatable(1f) }
    Box(Modifier.fillMaxWidth().onSizeChanged { width = it.width.toFloat() }) {
        Box(Modifier.align(Alignment.CenterEnd).padding(end = 6.dp).graphicsLayer { alpha = (-x.value / 240f).coerceIn(0f, 1f) }) {
            Icon(Icons.Trash, 20.dp, C.Red)
        }
        Box(
            Modifier
                .graphicsLayer { translationX = x.value; scaleX = pressScale.value; scaleY = pressScale.value }
                .background(C.Paper)
                .pointerInput(Unit) {
                    val tracker = VelocityTracker()
                    detectHorizontalDragGestures(
                        onDragStart = { tracker.resetTracking() },
                        onHorizontalDrag = { change, dx ->
                            tracker.addPosition(change.uptimeMillis, change.position)
                            val next = x.value + if (x.value + dx > 0) dx * 0.3f else dx
                            scope.launch { x.snapTo(next) }
                        },
                        onDragEnd = {
                            val v = tracker.calculateVelocity().x
                            scope.launch {
                                if (x.value < -width * 0.32f || v < -1500f) {
                                    x.animateTo(-width - 80f, S.snappy(), initialVelocity = minOf(v, -1600f))
                                    onRemove()
                                } else {
                                    x.animateTo(0f, S.bouncy(), initialVelocity = v)
                                }
                            }
                        },
                        onDragCancel = { scope.launch { x.animateTo(0f, S.bouncy()) } },
                    )
                }
                .pointerInput(Unit) {
                    detectTapGestures(
                        onPress = {
                            scope.launch { pressScale.animateTo(0.985f, S.snappy()) }
                            tryAwaitRelease()
                            scope.launch { pressScale.animateTo(1f, S.bouncy()) }
                        },
                        onTap = { onTap() },
                    )
                }
        ) { content() }
    }
}

@Composable
fun QrMatrix(matrix: List<String>, modifier: Modifier = Modifier) {
    Canvas(modifier) {
        val n = matrix.size
        if (n == 0) return@Canvas
        val cell = size.minDimension / (n + 4)
        drawRect(Color.White)
        matrix.forEachIndexed { y, row ->
            row.forEachIndexed { x, c ->
                if (c == '1') drawRect(C.Ink, Offset((x + 2) * cell, (y + 2) * cell), Size(cell + 0.5f, cell + 0.5f))
            }
        }
    }
}

@Composable
fun ToastHost(messages: Flow<String>, modifier: Modifier = Modifier) {
    var msg by remember { mutableStateOf("") }
    val y = remember { Animatable(160f) }
    LaunchedEffect(Unit) {
        messages.collect { m ->
            msg = m
            launch {
                y.animateTo(0f, S.bouncy())
                kotlinx.coroutines.delay(1600)
                y.animateTo(160f, S.snappy())
            }
        }
    }
    Row(
        modifier
            .graphicsLayer { translationY = y.value * density }
            .background(C.Ink)
            .padding(horizontal = 18.dp, vertical = 13.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Icon(Icons.Check, 16.dp, C.Red)
        Spacer(Modifier.width(10.dp))
        Text(msg, style = text(14.sp, 700, color = C.Paper), maxLines = 2, overflow = TextOverflow.Ellipsis)
    }
}
