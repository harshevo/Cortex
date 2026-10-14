package dev.cortex.remote

import android.content.Intent
import android.net.Uri
import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.activity.viewModels
import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.browser.customtabs.CustomTabsIntent
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.ui.Modifier
import androidx.lifecycle.lifecycleScope
import dev.cortex.remote.ui.CortexTheme
import dev.cortex.remote.ui.HomeScreen
import dev.cortex.remote.ui.WelcomeScreen
import dev.cortex.remote.ui.colors
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import uniffi.cortex_mobile.Answer

class MainActivity : ComponentActivity() {
    private val model: AppModel by viewModels()

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)

        enableEdgeToEdge()

        setContent {
            CortexTheme(model.look.theme, model.look.textScale) {
                Box(Modifier.fillMaxSize().background(colors.background)) {
                    // The sign-in page opens over the app, which goes on
                    // running under it and hears when it is done.
                    LaunchedEffect(model.signInAddress) {
                        model.signInAddress?.let { address ->
                            model.signInAddress = null

                            CustomTabsIntent.Builder().build().launchUrl(this@MainActivity, Uri.parse(address))
                        }
                    }

                    // A phone that works with neither its own agent nor a
                    // desktop's has nothing to show but how to begin.
                    AnimatedContent(
                        targetState = model.state.hosts.isEmpty() && !model.state.onPhone,
                        transitionSpec = { fadeIn() togetherWith fadeOut() },
                        label = "root",
                    ) { unpaired ->
                        if (unpaired) WelcomeScreen(model) else HomeScreen(model)
                    }
                }
            }
        }

        RestWorker.schedule(this)

        rehearse(intent)
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)

        rehearse(intent)
    }

    // A connection lost while the app was away is made again at once.
    override fun onResume() {
        super.onResume()

        model.core.foreground()
    }

    /**
     * Driving the app without a person, to try it:
     *
     *     adb shell am start -n dev.cortex.remote/.MainActivity \
     *         --es pair <code> --es submit <text> --ez allow true --es stage chat
     *
     * `--es stage` is one of chat, memory, capabilities, phone, settings, trading, insights,
 * themes or desktops; `--es theme <name>` puts a theme in use.
 * `--ez phone true` works with the agent on the phone instead of pairing,
     * and `--es address <url> --es model <name> --es key <key>` points it at
     * a hosted model, such as a stand-in on the computer running the emulator.
     *
     * None of it runs unless asked for.
     */
    private fun rehearse(intent: Intent) {
        intent.getStringExtra("theme")?.let { model.look.choose(it) }
        intent.getStringExtra("stage")?.let { model.stage = it }

        if (intent.getBooleanExtra("phone", false)) {
            model.core.usePhone()
        }

        // `--ez signin true` starts the ChatGPT sign-in.
        if (intent.getBooleanExtra("signin", false)) {
            model.core.signIn()
        }

        val code = intent.getStringExtra("pair")

        if (code != null) {
            model.core.pair(code)
        }

        val instruction = intent.getStringExtra("submit")
        val allow = intent.getBooleanExtra("allow", false)

        val address = intent.getStringExtra("address")

        if (instruction == null && !allow && address == null) return

        lifecycleScope.launch {
            var sent = instruction == null

            if (address != null) {
                // The agent on the phone has to be there to be told.
                while (model.state.chats.isEmpty()) delay(150)

                val chat = model.state.chats.first().id

                model.core.setAddress(address)
                model.core.setKey("hosted", intent.getStringExtra("key") ?: "")
                model.core.setModel(chat, "hosted", intent.getStringExtra("model") ?: "stand-in")

                // The agent is built again with it.
                delay(1500)
            }

            while (true) {
                delay(150)

                val chat = model.state.chats.firstOrNull() ?: continue

                if (!sent && chat.ready) {
                    sent = true

                    model.core.submit(chat.id, instruction!!, emptyList())
                    model.stage = "chat"
                }

                if (allow) {
                    model.feed(chat.id).approvals.firstOrNull()?.let { approval ->
                        delay(1500)

                        model.core.answer(chat.id, approval.id, Answer.ALLOW)
                    }
                }
            }
        }
    }
}
