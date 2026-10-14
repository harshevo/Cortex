package dev.cortex.remote

import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.net.Uri
import android.util.LruCache
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import uniffi.cortex_mobile.Cortex
import uniffi.cortex_mobile.Picture
import java.io.ByteArrayOutputStream

/**
 * The pictures sent with instructions, as the conversation shows them.
 *
 * A photo is many megapixels and its row shows it a few dozen dots wide,
 * so each is decoded at about the size it is drawn, off the main thread,
 * and the decoded ones are kept up to a fixed amount of memory.
 */
class Pictures(private val core: Cortex) {
    private val kept = object : LruCache<String, ImageBitmap>(KEPT_BYTES) {
        override fun sizeOf(key: String, value: ImageBitmap) = value.width * value.height * 4
    }

    /** A picture already decoded, without waiting. */
    fun kept(key: String): ImageBitmap? = kept.get(key)

    /**
     * Picture [index] of the instruction whose id is [item], at least
     * [side] pixels on its shorter side. [text] tells a session's items
     * apart from another's at the same place.
     */
    suspend fun load(chat: String, item: ULong, index: Int, text: String, side: Int): ImageBitmap? {
        val key = key(chat, item, index, text)

        kept.get(key)?.let { return it }

        return withContext(Dispatchers.IO) {
            val bytes = core.picture(chat, item, index.toUInt()) ?: return@withContext null

            decode(bytes, side)?.asImageBitmap()?.also { kept.put(key, it) }
        }
    }

    companion object {
        /** About two screens of thumbnails. */
        private const val KEPT_BYTES = 8 * 1024 * 1024

        /** The longest side of a picture sent to a model. */
        private const val SENT_SIDE = 1600

        fun key(chat: String, item: ULong, index: Int, text: String) = "$chat/$item/$index/${text.hashCode()}"

        /** Decodes at the smallest power-of-two reduction that keeps the shorter side at least [side]. */
        fun decode(bytes: ByteArray, side: Int): Bitmap? {
            val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }

            BitmapFactory.decodeByteArray(bytes, 0, bytes.size, bounds)

            if (bounds.outWidth <= 0 || bounds.outHeight <= 0) return null

            var sample = 1

            while (minOf(bounds.outWidth, bounds.outHeight) / (sample * 2) >= side) sample *= 2

            val options = BitmapFactory.Options().apply { inSampleSize = sample }

            return BitmapFactory.decodeByteArray(bytes, 0, bytes.size, options)
        }

        /**
         * Reads a chosen picture, made small enough to send, as JPEG. It is
         * never decoded at full size. Runs off the main thread.
         */
        suspend fun forSending(context: Context, uri: Uri): Picture? = withContext(Dispatchers.IO) {
            val bytes = context.contentResolver.openInputStream(uri)?.use { it.readBytes() } ?: return@withContext null

            val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }

            BitmapFactory.decodeByteArray(bytes, 0, bytes.size, bounds)

            var sample = 1

            while (maxOf(bounds.outWidth, bounds.outHeight) / (sample * 2) >= SENT_SIDE) sample *= 2

            val bitmap = BitmapFactory.decodeByteArray(
                bytes,
                0,
                bytes.size,
                BitmapFactory.Options().apply { inSampleSize = sample },
            ) ?: return@withContext null

            val longest = maxOf(bitmap.width, bitmap.height)
            val scale = minOf(1f, SENT_SIDE.toFloat() / longest)

            val small = if (scale < 1f) {
                Bitmap.createScaledBitmap(bitmap, (bitmap.width * scale).toInt(), (bitmap.height * scale).toInt(), true)
            } else {
                bitmap
            }

            val out = ByteArrayOutputStream()

            small.compress(Bitmap.CompressFormat.JPEG, 80, out)

            if (small !== bitmap) small.recycle()

            bitmap.recycle()

            Picture("image/jpeg", out.toByteArray())
        }
    }
}
