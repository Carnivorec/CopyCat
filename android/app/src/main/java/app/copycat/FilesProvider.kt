package app.copycat

import android.content.ActivityNotFoundException
import android.content.ContentProvider
import android.content.ContentValues
import android.content.Context
import android.content.Intent
import android.database.Cursor
import android.database.MatrixCursor
import android.net.Uri
import android.os.ParcelFileDescriptor
import android.provider.DocumentsContract
import android.provider.OpenableColumns
import android.webkit.MimeTypeMap
import android.widget.Toast
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withTimeout
import java.io.File
import java.io.FileNotFoundException

class FilesProvider : ContentProvider() {
    override fun onCreate() = true

    override fun getType(uri: Uri): String = mimeOf(pathOf(uri) ?: "")

    override fun openFile(uri: Uri, mode: String): ParcelFileDescriptor {
        if (mode != "r") throw SecurityException("只读")
        val path = pathOf(uri) ?: throw FileNotFoundException("无效的链接")
        return open(path).second
    }

    override fun query(uri: Uri, projection: Array<out String>?, selection: String?, args: Array<out String>?, sort: String?): Cursor {
        val path = pathOf(uri) ?: throw FileNotFoundException("无效的链接")
        val cols = projection ?: arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE)
        val size by lazy { runCatching { open(path).let { (s, fd) -> fd.close(); s } }.getOrNull() }
        val row = cols.map {
            when (it) {
                OpenableColumns.DISPLAY_NAME -> File(path).name
                OpenableColumns.SIZE -> size
                else -> null
            }
        }
        return MatrixCursor(cols).apply { addRow(row) }
    }

    private fun open(path: String): Pair<Long, ParcelFileDescriptor> = runBlocking {
        val ctx = context ?: throw FileNotFoundException("未就绪")
        try {
            withTimeout(15_000) {
                if (DaemonClient.ensure(ctx) !is DaemonClient.Ensure.Ok) throw FileNotFoundException("守护进程未运行")
                DaemonClient.openFile(path)
            }
        } catch (e: FileNotFoundException) {
            throw e
        } catch (e: Exception) {
            throw FileNotFoundException(e.message ?: "无法打开")
        }
    }

    override fun insert(uri: Uri, values: ContentValues?): Uri? = null
    override fun delete(uri: Uri, selection: String?, args: Array<out String>?) = 0
    override fun update(uri: Uri, values: ContentValues?, selection: String?, args: Array<out String>?) = 0

    companion object {
        private const val AUTHORITY = "app.copycat.files"

        fun uriOf(path: String): Uri = Uri.Builder().scheme("content").authority(AUTHORITY).appendPath("f").appendPath(path).build()

        private fun pathOf(uri: Uri): String? = uri.pathSegments.takeIf { it.size == 2 && it[0] == "f" }?.get(1)

        fun mimeOf(path: String): String {
            val ext = path.substringAfterLast('.', "").lowercase()
            return MimeTypeMap.getSingleton().getMimeTypeFromExtension(ext) ?: "application/octet-stream"
        }

        fun open(ctx: Context, path: String, dir: Boolean) {
            try {
                ctx.startActivity(viewIntent(path, dir))
            } catch (e: ActivityNotFoundException) {
                Toast.makeText(ctx, if (dir) "没有可以浏览文件夹的应用" else "没有可以打开这种文件的应用", Toast.LENGTH_SHORT).show()
            }
        }

        fun viewIntent(path: String, dir: Boolean): Intent = if (dir) {

            val rel = path.removePrefix("/storage/emulated/0/").trimEnd('/')
            Intent(Intent.ACTION_VIEW)
                .setDataAndType(DocumentsContract.buildDocumentUri("com.android.externalstorage.documents", "primary:$rel"), DocumentsContract.Document.MIME_TYPE_DIR)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        } else {
            Intent(Intent.ACTION_VIEW)
                .setDataAndType(uriOf(path), mimeOf(path))
                .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_ACTIVITY_NEW_TASK)
        }
    }
}
