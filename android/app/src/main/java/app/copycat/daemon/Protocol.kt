package app.copycat.daemon

object Protocol {
    const val DESCRIPTOR = "app.copycat.IDaemon"
    const val CALLBACK_DESCRIPTOR = "app.copycat.IDaemonCallback"

    const val TX_REQUEST = android.os.IBinder.FIRST_CALL_TRANSACTION

    const val TX_SUBSCRIBE = android.os.IBinder.FIRST_CALL_TRANSACTION + 1

    const val TX_SEND_FILES = android.os.IBinder.FIRST_CALL_TRANSACTION + 2

    const val TX_OPEN_FILE = android.os.IBinder.FIRST_CALL_TRANSACTION + 3

    const val TX_EVENT = android.os.IBinder.FIRST_CALL_TRANSACTION

    const val AUTHORITY = "app.copycat.daemon"
    const val METHOD_ATTACH = "attach"
    const val EXTRA_BINDER = "binder"

    const val METHOD_TRANSFER = "transfer"
    const val EXTRA_JSON = "json"

    const val METHOD_REMOVED = "removed"
    const val EXTRA_PATHS = "paths"

    const val CLIP_MARK = "app.copycat.origin"

    const val PACKAGE = "app.copycat"

    const val RECEIVE_ROOT = "/storage/emulated/0/Download/CopyCat"
    const val DATA_DIR = "/data/local/copycat"
    const val ROOT_DIR = "/data/adb/copycat"
}
