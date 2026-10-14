package dev.cortex.remote.ui.settings

import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.padding
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.Info
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import dev.cortex.remote.AppModel
import dev.cortex.remote.RestWorker
import dev.cortex.remote.ui.Space

/** What this app is, and what its networks did while the phone was at rest. */
@Composable
fun aboutSection(model: AppModel): Section {
    val context = LocalContext.current

    val version = remember {
        runCatching { context.packageManager.getPackageInfo(context.packageName, 0).versionName }.getOrNull() ?: "?"
    }

    val rest = remember { RestWorker.last(context) }

    return Section(
        id = "about",
        title = "About",
        icon = Icons.Rounded.Info,
        summary = "Cortex $version",
        terms = listOf("version", "rest", "living network", "about", "encrypted"),
    ) { _ ->
        Column(Modifier.padding(vertical = Space.xs)) {
            ValueRow("Version", version)
            Divider()
            ValueRow("Agent", if (model.state.onPhone) "On this phone" else "On a computer")
            Divider()

            Note(rest ?: "While the phone is idle, the living network goes over what it met. Nothing yet.")

            Note(
                "On this phone the agent, its memory and its networks run here and only the model is elsewhere. " +
                    "Paired, they stay on your computer and the connection is encrypted end to end.",
            )
        }
    }
}
