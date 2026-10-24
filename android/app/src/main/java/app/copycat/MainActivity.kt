package app.copycat

import android.Manifest
import android.content.Intent
import android.content.pm.PackageManager
import android.graphics.Color
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.SystemBarStyle
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.result.contract.ActivityResultContracts
import androidx.activity.viewModels
import androidx.compose.runtime.mutableStateOf
import app.copycat.ui.Page
import app.copycat.ui.Root

class MainActivity : ComponentActivity() {
    private val model: AppModel by viewModels()
    private val pendingInvite = mutableStateOf<String?>(null)
    private val openPage = mutableStateOf<Page?>(null)
    private val askNotify = registerForActivityResult(ActivityResultContracts.RequestPermission()) {}

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val bars = SystemBarStyle.light(Color.TRANSPARENT, Color.TRANSPARENT)
        enableEdgeToEdge(bars, bars)
        handle(intent)
        setContent {
            Root(model, pendingInvite.value, { pendingInvite.value = null }, openPage.value, { openPage.value = null })
        }

        if (savedInstanceState == null && checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED) {
            askNotify.launch(Manifest.permission.POST_NOTIFICATIONS)
        }
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        handle(intent)
    }

    override fun onResume() {
        super.onResume()
        if (model.phase.value == Phase.Ready) model.refresh()
    }

    private fun handle(intent: Intent?) {
        if (intent?.getStringExtra(Notifier.EXTRA_PAGE) == "transfer") openPage.value = Page.Transfer
        val uri = intent?.data ?: return
        if (uri.scheme != "copycat") return
        if (model.overview.value?.initialized == true) {
            model.toast("本机已经加入设备组")
        } else {
            pendingInvite.value = uri.toString()
        }
    }
}
