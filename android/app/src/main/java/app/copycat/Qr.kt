package app.copycat

import android.content.ContentResolver
import android.graphics.Bitmap
import android.graphics.ImageDecoder
import android.net.Uri
import androidx.camera.core.ImageProxy
import com.google.zxing.BarcodeFormat
import com.google.zxing.BinaryBitmap
import com.google.zxing.DecodeHintType
import com.google.zxing.LuminanceSource
import com.google.zxing.PlanarYUVLuminanceSource
import com.google.zxing.RGBLuminanceSource
import com.google.zxing.ReaderException
import com.google.zxing.common.HybridBinarizer
import com.google.zxing.qrcode.QRCodeReader

object Qr {
    const val PREFIX = "copycat://pair?"

    fun isInvite(text: String) = text.startsWith(PREFIX)

    fun decode(src: LuminanceSource, hard: Boolean = false): String? {
        val hints = buildMap<DecodeHintType, Any> {
            put(DecodeHintType.POSSIBLE_FORMATS, listOf(BarcodeFormat.QR_CODE))
            put(DecodeHintType.CHARACTER_SET, "UTF-8")
            if (hard) put(DecodeHintType.TRY_HARDER, true)
        }
        for (s in listOf(src, src.invert())) {
            try {
                return QRCodeReader().decode(BinaryBitmap(HybridBinarizer(s)), hints).text
            } catch (_: ReaderException) {
            }
        }
        return null
    }

    fun decode(img: ImageProxy): String? {
        val plane = img.planes[0]
        val stride = plane.rowStride
        val buf = plane.buffer.apply { rewind() }

        val data = ByteArray(stride * img.height)
        buf.get(data, 0, minOf(buf.remaining(), data.size))
        return decode(PlanarYUVLuminanceSource(data, stride, img.height, 0, 0, img.width, img.height, false))
    }

    fun decode(bitmap: Bitmap): String? {
        val w = bitmap.width
        val h = bitmap.height
        val px = IntArray(w * h)
        bitmap.getPixels(px, 0, w, 0, 0, w, h)
        return decode(RGBLuminanceSource(w, h, px), hard = true)
    }

    fun decode(cr: ContentResolver, uri: Uri): String? {
        val bitmap = ImageDecoder.decodeBitmap(ImageDecoder.createSource(cr, uri)) { d, info, _ ->
            d.allocator = ImageDecoder.ALLOCATOR_SOFTWARE
            val long = maxOf(info.size.width, info.size.height)
            if (long > 2000) d.setTargetSampleSize((long + 1999) / 2000)
        }
        return decode(bitmap)
    }
}
