package dev.cortex.remote.ui.settings

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.Check
import androidx.compose.material.icons.rounded.Memory
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import dev.cortex.remote.AppModel
import dev.cortex.remote.ui.Space
import dev.cortex.remote.ui.Type
import dev.cortex.remote.ui.colors
import dev.cortex.remote.ui.effortName
import uniffi.cortex_mobile.SettingsView

/** The model a chat uses, and how hard it thinks. */
@Composable
fun modelSection(model: AppModel, chat: String?, settings: SettingsView?): Section = Section(
    id = "model",
    title = "Model and effort",
    icon = Icons.Rounded.Memory,
    summary = settings?.let { listOf(it.model.ifEmpty { "No model" }, if (it.takesEffort) effortName(it.effort) else null).filterNotNull().joinToString(" · ") }
        ?: "No chat open",
    terms = listOf("model", "effort", "reasoning", "thinking") + (settings?.models?.map { it.model } ?: emptyList()),
) { _ ->
    if (chat == null || settings == null) {
        Note("Connect to the agent on this phone or a computer to choose.")
    } else {
        ModelRows(model, chat, settings)
    }
}

@Composable
private fun ModelRows(model: AppModel, chat: String, settings: SettingsView) {
    var choosing by remember { mutableStateOf(false) }

    Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
        Box {
            ValueRow("Model", settings.model.ifEmpty { "None chosen" }, Icons.Rounded.Memory) { choosing = true }

            DropdownMenu(choosing, { choosing = false }, containerColor = colors.raised) {
                for ((provider, choices) in settings.models.groupBy { it.providerLabel }) {
                    Text(
                        provider.uppercase(),
                        style = Type.heading,
                        color = colors.textMuted,
                        modifier = Modifier.padding(horizontal = 14.dp, vertical = Space.s),
                    )

                    for (choice in choices) {
                        val current = choice.model == settings.model && choice.provider == settings.provider

                        DropdownMenuItem(
                            text = { Text(choice.model, style = Type.callout, color = colors.text) },
                            trailingIcon = {
                                if (current) Icon(Icons.Rounded.Check, null, tint = colors.accent, modifier = Modifier.size(18.dp))
                            },
                            onClick = {
                                choosing = false

                                model.core.setModel(chat, choice.provider, choice.model)
                            },
                        )
                    }
                }
            }
        }

        if (settings.takesEffort && settings.efforts.isNotEmpty()) {
            Divider()

            Column(Modifier.padding(horizontal = 14.dp, vertical = 10.dp), verticalArrangement = Arrangement.spacedBy(Space.s)) {
                Text("Effort", style = Type.callout, color = colors.text)

                Segments(settings.efforts, settings.effort, ::effortName) { model.core.setEffort(chat, it) }
            }
        }

        if (model.feed(chat).running) {
            Note("Changes take effect when the run in progress has finished.", color = colors.warning)
        }
    }
}
