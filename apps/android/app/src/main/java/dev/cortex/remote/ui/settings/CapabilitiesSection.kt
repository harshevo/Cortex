package dev.cortex.remote.ui.settings

import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.padding
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.Extension
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import dev.cortex.remote.AppModel
import dev.cortex.remote.ui.SectionHeading
import dev.cortex.remote.ui.Space
import uniffi.cortex_mobile.CapabilityView
import uniffi.cortex_mobile.SettingsView

/**
 * Every optional capability of the engine, each as a switch, in the
 * groups the engine lists them in, with a line of help each.
 */
@Composable
fun capabilitiesSection(model: AppModel, chat: String?, settings: SettingsView?): Section {
    val capabilities = settings?.capabilities ?: emptyList()

    return Section(
        id = "capabilities",
        title = "Capabilities",
        icon = Icons.Rounded.Extension,
        summary = if (settings == null) "No chat open" else "${capabilities.count { it.on }} of ${capabilities.size} on",
        terms = capabilities.flatMap { listOf(it.label, it.name, it.help, it.group) },
    ) { query ->
        val shown = capabilities.filter { matches(query, it.label, it.name, it.help, it.group, "capabilities") }

        if (chat == null || settings == null) {
            Note("Connect to the agent on this phone or a computer to choose.")
        }

        // The groups in the order the engine lists them.
        for ((group, members) in shown.groupBy { it.group }) {
            SectionHeading(
                "$group · ${members.count { it.on }} of ${members.size}",
                Modifier.padding(start = 14.dp, top = Space.m, bottom = Space.xs),
            )

            Column {
                members.forEachIndexed { index, capability ->
                    CapabilityRow(capability) { on -> chat?.let { model.core.setCapability(it, capability.name, on) } }

                    if (index < members.lastIndex) Divider()
                }
            }
        }
    }
}

/** The dendritic and the living network are one network with two switches. */
private fun helpOf(capability: CapabilityView): String {
    val same = when (capability.name) {
        "brain" -> "The same network as the Living network: this switch lets it advise."
        "living" -> "The same network as the Dendritic network: this switch only lets it watch."
        else -> return capability.help
    }

    val said = listOf("same network as", "same dendritic network").any { capability.help.contains(it, ignoreCase = true) }

    return if (said) capability.help else "$same ${capability.help}"
}

@Composable
private fun CapabilityRow(capability: CapabilityView, set: (Boolean) -> Unit) {
    SwitchRow(
        title = capability.label,
        help = helpOf(capability),
        on = capability.on,
        name = capability.name,
        // The memory screen shows the agent's memory, so it is always kept.
        enabled = capability.name != "memory",
        set = set,
    )
}
