package app.copycat

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.media.MediaScannerConnection
import org.json.JSONObject

object Notifier {
    private const val CHANNEL = "transfers"
    const val EXTRA_PAGE = "page"

    fun transfer(ctx: Context, json: String) {
        val t = runCatching { Parse.transfer(JSONObject(json)) }.getOrNull() ?: return
        val what = if (t.count == 1) t.title else "${t.count} 个文件"
        val (title, body) = when {
            t.kind == "screenshot" && !t.outgoing && t.state == "done" -> "收到电脑截图" to "来自 ${t.device} · 保存在 ${shortDir(t.dir)}"
            !t.outgoing && t.state == "done" -> "已收到 $what" to "来自 ${t.device} · 保存在 ${shortDir(t.dir)}"
            t.outgoing && t.state == "done" -> (if (t.text != null) "已分享给 ${t.device}" else "已发送给 ${t.device}") to t.title
            t.state == "failed" -> "${if (t.outgoing) "发送给" else "接收自"} ${t.device} 失败" to (t.error ?: "传输中断")
            else -> return
        }
        if (!t.outgoing && t.state == "done" && t.saved.isNotEmpty()) {
            MediaScannerConnection.scanFile(ctx.applicationContext, t.saved.toTypedArray(), null, null)
            val open = t.saved.singleOrNull()?.let { FilesProvider.viewIntent(it, t.savedDirs.firstOrNull() ?: false) }
            post(ctx, t.id, title, body, open)
        } else {
            post(ctx, t.id, title, body, null)
        }
    }

    fun shortDir(dir: String?): String = dir?.removePrefix("/storage/emulated/0/")?.removePrefix("/sdcard/") ?: "下载目录"

    private fun post(ctx: Context, id: String, title: String, body: String, target: Intent?) {
        val nm = ctx.getSystemService(NotificationManager::class.java)
        nm.createNotificationChannel(
            NotificationChannel(CHANNEL, "文件传输", NotificationManager.IMPORTANCE_DEFAULT).apply { description = "文件收发完成或失败时提醒" },
        )
        val open = target
            ?: Intent(ctx, MainActivity::class.java).putExtra(EXTRA_PAGE, "transfer").addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP)
        val pi = PendingIntent.getActivity(ctx, id.hashCode(), open, PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)
        val n = Notification.Builder(ctx, CHANNEL)
            .setSmallIcon(R.drawable.ic_notify)
            .setColor(0xFFE1251B.toInt())
            .setContentTitle(title)
            .setContentText(body)
            .setStyle(Notification.BigTextStyle().bigText(body))
            .setContentIntent(pi)
            .setAutoCancel(true)

            .setVisibility(Notification.VISIBILITY_PRIVATE)
            .setPublicVersion(
                Notification.Builder(ctx, CHANNEL)
                    .setSmallIcon(R.drawable.ic_notify)
                    .setColor(0xFFE1251B.toInt())
                    .setContentTitle("CopyCat")
                    .setContentText("文件传输有新消息")
                    .build(),
            )
            .build()
        runCatching { nm.notify(id.hashCode(), n) }
    }
}
