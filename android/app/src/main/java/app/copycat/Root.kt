package app.copycat

import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import java.util.concurrent.TimeUnit

object Root {
    class Result(val code: Int, val out: String)

    suspend fun run(cmd: String, timeoutSec: Long = 20): Result = withContext(Dispatchers.IO) {
        try {
            val p = ProcessBuilder("su", "-c", cmd).redirectErrorStream(true).start()
            val out = p.inputStream.bufferedReader().readText()
            if (!p.waitFor(timeoutSec, TimeUnit.SECONDS)) {
                p.destroy()
                Result(-1, "执行超时")
            } else {
                Result(p.exitValue(), out.trim())
            }
        } catch (e: Exception) {
            Result(-1, e.message ?: "无法执行 su")
        }
    }

    suspend fun available(): Boolean = run("id -u", 15).let { it.code == 0 && it.out.lines().lastOrNull() == "0" }
}
