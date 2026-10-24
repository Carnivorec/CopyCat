package app.copycat

import android.graphics.Bitmap
import com.google.zxing.BarcodeFormat
import com.google.zxing.EncodeHintType
import com.google.zxing.RGBLuminanceSource
import com.google.zxing.qrcode.QRCodeWriter
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.GraphicsMode

@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
class QrTest {

    private val invite = Qr.PREFIX + "v=1&relay=clipboard-relay.example.com%3A8443&fp=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef" +
        "&t=exampleticket0123456789abcdef012&pk=abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789&n=%E5%8F%B0%E5%BC%8F%E6%9C%BA"

    private fun pixels(text: String, scale: Int = 6, dark: Int = 0xFF0A0A0A.toInt(), light: Int = -1): Triple<Int, Int, IntArray> {
        val m = QRCodeWriter().encode(text, BarcodeFormat.QR_CODE, 0, 0, mapOf(EncodeHintType.MARGIN to 4))
        val w = m.width * scale
        val h = m.height * scale
        val px = IntArray(w * h) { i -> if (m.get((i % w) / scale, (i / w) / scale)) dark else light }
        return Triple(w, h, px)
    }

    @Test fun decodesInvite() {
        val (w, h, px) = pixels(invite)
        val text = Qr.decode(RGBLuminanceSource(w, h, px))
        assertEquals(invite, text)
        assertTrue(Qr.isInvite(text!!))
    }

    @Test fun decodesInvertedCode() {
        val (w, h, px) = pixels(invite, dark = -1, light = 0xFF0A0A0A.toInt())
        assertEquals(invite, Qr.decode(RGBLuminanceSource(w, h, px)))
    }

    @Test fun decodesBitmap() {
        val (w, h, px) = pixels(invite, scale = 9)
        val bmp = Bitmap.createBitmap(px, w, h, Bitmap.Config.ARGB_8888)
        assertEquals(invite, Qr.decode(bmp))
    }

    @Test fun blankImageHasNoCode() {
        assertNull(Qr.decode(RGBLuminanceSource(200, 200, IntArray(200 * 200) { -1 })))
    }

    @Test fun otherCodesAreNotInvites() {
        val (w, h, px) = pixels("https://example.com")
        assertEquals(false, Qr.isInvite(Qr.decode(RGBLuminanceSource(w, h, px))!!))
    }
}
