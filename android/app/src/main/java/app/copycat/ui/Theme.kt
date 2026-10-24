package app.copycat.ui

import androidx.compose.animation.core.SpringSpec
import androidx.compose.animation.core.spring
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.graphics.vector.addPathNodes
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.Font
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontVariation
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.TextUnit
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.em
import androidx.compose.ui.unit.sp
import app.copycat.R
import kotlin.math.sqrt

object C {
    val Ink = Color(0xFF0A0A0A)
    val Paper = Color(0xFFFFFFFF)
    val Red = Color(0xFFE1251B)
    val Mute = Color(0xFF9A9A9A)
    val Faint = Color(0xFFD0D0D0)
    val Disabled = Color(0xFFBDBDBD)
}

@OptIn(androidx.compose.ui.text.ExperimentalTextApi::class)
private fun grotesk(w: Int) = Font(R.font.schibsted_grotesk, FontWeight(w), variationSettings = FontVariation.Settings(FontVariation.weight(w)))

object F {
    @OptIn(androidx.compose.ui.text.ExperimentalTextApi::class)
    val Grotesk = FontFamily(grotesk(400), grotesk(500), grotesk(600), grotesk(700), grotesk(800), grotesk(900))
    val Mono = FontFamily(
        Font(R.font.plex_mono_regular, FontWeight.Normal),
        Font(R.font.plex_mono_medium, FontWeight.Medium),
        Font(R.font.plex_mono_semibold, FontWeight.SemiBold),
    )

    val Cjk = FontFamily.Default
}

fun text(size: TextUnit, weight: Int = 400, family: FontFamily = F.Cjk, spacing: TextUnit = 0.sp, line: TextUnit = TextUnit.Unspecified, color: Color = C.Ink) =
    TextStyle(fontSize = size, fontWeight = FontWeight(weight), fontFamily = family, letterSpacing = spacing, lineHeight = line, color = color)

object T {
    val label = text(12.sp, 700)
    val mono = text(12.sp, 500, F.Mono, 0.04.em)
    val monoSmall = text(11.sp, 500, F.Mono, 0.06.em)
    val body = text(15.sp, 400, line = 22.sp)
    val bodyMute = text(13.sp, 400, line = 20.sp, color = Color(0xFF555555))
    val row = text(17.sp, 500)
    val title = text(46.sp, 900, line = 50.sp, spacing = (-0.02).em)
    val strong = text(16.sp, 700)
}

object S {
    private fun <T> kc(k: Float, c: Float): SpringSpec<T> = spring(dampingRatio = c / (2 * sqrt(k)), stiffness = k)
    fun <T> snappy() = kc<T>(520f, 34f)
    fun <T> bouncy() = kc<T>(380f, 17f)
    fun <T> gentle() = kc<T>(170f, 22f)
    fun <T> wobbly() = kc<T>(260f, 11f)
    fun <T> rule() = kc<T>(160f, 20f)
}

val Pad = 22.dp

object Icons {
    private fun icon(name: String, vararg paths: String) = ImageVector.Builder(name, 24.dp, 24.dp, 24f, 24f).apply {
        for (d in paths) {
            addPath(
                pathData = addPathNodes(d),
                stroke = SolidColor(Color.Black),
                strokeLineWidth = 2f,
                strokeLineCap = StrokeCap.Round,
                strokeLineJoin = StrokeJoin.Round,
            )
        }
    }.build()

    val Copy = icon("copy", "M10 8h10a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2H10a2 2 0 0 1-2-2V10a2 2 0 0 1 2-2z", "M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2")
    val Trash = icon("trash", "M10 11v6", "M14 11v6", "M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6", "M3 6h18", "M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2")
    val Search = icon("search", "m21 21-4.34-4.34", "M19 11a8 8 0 1 1-16 0 8 8 0 0 1 16 0z")
    val Check = icon("check", "M20 6 9 17l-5-5")
    val Plus = icon("plus", "M5 12h14", "M12 5v14")
    val X = icon("x", "M18 6 6 18", "m6 6 12 12")
    val Phone = icon("smartphone", "M7 2h10a2 2 0 0 1 2 2v16a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2z", "M12 18h.01")
    val Monitor = icon("monitor", "M4 3h16a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2z", "M8 21h8", "M12 17v4")
    val ArrowRight = icon("arrow-right", "M5 12h14", "m12 5 7 7-7 7")
    val Power = icon("power", "M12 2v10", "M18.4 6.6a9 9 0 1 1-12.77.04")
    val Paste = icon("clipboard-paste", "M11 14h10", "M16 4h2a2 2 0 0 1 2 2v1.344", "m17 18 4-4-4-4", "M8 4H6a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h12a2 2 0 0 0 1.793-1.113", "M9 2h6a1 1 0 0 1 1 1v2a1 1 0 0 1-1 1H9a1 1 0 0 1-1-1V3a1 1 0 0 1 1-1z")
    val Scan = icon("scan-line", "M3 7V5a2 2 0 0 1 2-2h2", "M17 3h2a2 2 0 0 1 2 2v2", "M21 17v2a2 2 0 0 1-2 2h-2", "M7 21H5a2 2 0 0 1-2-2v-2", "M7 12h10")
    val Image = icon("image", "M5 3h14a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2z", "M11 9a2 2 0 1 1-4 0 2 2 0 0 1 4 0z", "m21 15-3.086-3.086a2 2 0 0 0-2.828 0L6 21")
}
