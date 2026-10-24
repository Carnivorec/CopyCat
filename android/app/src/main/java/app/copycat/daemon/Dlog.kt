package app.copycat.daemon

import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale

object Dlog {
    private val fmt = SimpleDateFormat("MM-dd HH:mm:ss", Locale.ROOT)

    @Synchronized
    fun p(msg: String) {
        System.err.println("${fmt.format(Date())} [java] $msg")
    }
}
