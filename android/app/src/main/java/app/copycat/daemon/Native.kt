package app.copycat.daemon

object Native {

    @JvmStatic external fun start(dataDir: String, key: ByteArray, deviceName: String): String?

    @JvmStatic external fun localClip(text: String?, html: String?, source: String?)

    @JvmStatic external fun request(json: String): String

    @JvmStatic external fun sendFiles(device: String, names: Array<String>, fds: IntArray): String

    @JvmStatic external fun nextAction(): String?

    @JvmStatic external fun stop()
}
