package app.copycat

import android.content.ContentProvider
import android.content.ContentValues
import android.database.Cursor
import android.media.MediaScannerConnection
import android.net.Uri
import android.os.Binder
import android.os.Bundle
import app.copycat.daemon.Protocol

class DaemonProvider : ContentProvider() {
    override fun call(method: String, arg: String?, extras: Bundle?): Bundle? {
        val caller = Binder.getCallingUid()
        if (caller != 2000 && caller != 0) throw SecurityException("仅接受 CopyCat 守护进程调用")
        when (method) {
            Protocol.METHOD_ATTACH -> extras?.getBinder(Protocol.EXTRA_BINDER)?.let(DaemonClient::onAttached)
            Protocol.METHOD_TRANSFER -> extras?.getString(Protocol.EXTRA_JSON)?.let { Notifier.transfer(context!!, it) }

            Protocol.METHOD_REMOVED -> extras?.getStringArray(Protocol.EXTRA_PATHS)?.let { MediaScannerConnection.scanFile(context!!.applicationContext, it, null, null) }
        }
        return null
    }

    override fun onCreate() = true
    override fun query(uri: Uri, projection: Array<out String>?, selection: String?, selectionArgs: Array<out String>?, sortOrder: String?): Cursor? = null
    override fun getType(uri: Uri): String? = null
    override fun insert(uri: Uri, values: ContentValues?): Uri? = null
    override fun delete(uri: Uri, selection: String?, selectionArgs: Array<out String>?) = 0
    override fun update(uri: Uri, values: ContentValues?, selection: String?, selectionArgs: Array<out String>?) = 0
}
