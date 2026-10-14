package dev.cortex.remote.ui.settings

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.Computer
import androidx.compose.material.icons.rounded.Smartphone
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
import dev.cortex.remote.ui.ConnectionNote
import dev.cortex.remote.ui.ConnectionPill
import dev.cortex.remote.ui.Line
import dev.cortex.remote.ui.PairingSheet
import dev.cortex.remote.ui.Space
import dev.cortex.remote.ui.Type
import dev.cortex.remote.ui.colors
import uniffi.cortex_mobile.Connection

/** Where the agent runs: this phone, or one of the computers it is paired with. */
@Composable
fun desktopsSection(model: AppModel): Section {
    val state = model.state

    val current = state.hosts.firstOrNull { it.id == state.host }?.name

    return Section(
        id = "desktops",
        title = "Where the agent runs",
        icon = Icons.Rounded.Computer,
        summary = when {
            state.onPhone -> "This phone"
            current != null -> current
            else -> "${state.hosts.size} computer${if (state.hosts.size == 1) "" else "s"} paired"
        },
        terms = listOf("desktop", "computer", "pair", "connect", "disconnect", "forget", "phone", "remote") + state.hosts.map { it.name },
    ) { _ -> Places(model) }
}

@Composable
private fun Places(model: AppModel) {
    val state = model.state

    var pairing by remember { mutableStateOf(false) }

    Column {
        Place(
            icon = Icons.Rounded.Smartphone,
            name = "This phone",
            detail = if (state.onPhone) "In use: the agent runs here" else "The agent runs here, on its own",
            current = state.onPhone,
        ) {
            if (!state.onPhone) SmallButton("Use") { model.core.usePhone() }
        }

        for (host in state.hosts) {
            val current = state.host == host.id

            Divider()

            Place(Icons.Rounded.Computer, host.name, null, current, connection = state.connection.takeIf { current }) {
                SmallButton(if (current) "Disconnect" else "Connect") {
                    if (current) model.core.disconnect() else model.core.connect(host.id)
                }

                SmallButton("Forget") { model.core.forget(host.id) }
            }
        }

        if (!state.onPhone) {
            ConnectionNote(state.connection, Modifier.fillMaxWidth().padding(horizontal = 14.dp, vertical = Space.xs))
        }

        SmallButton("Pair with a computer", Modifier.fillMaxWidth().padding(14.dp), strong = state.hosts.isEmpty()) { pairing = true }
    }

    if (pairing) {
        PairingSheet(model) { pairing = false }
    }
}

@Composable
private fun Place(
    icon: androidx.compose.ui.graphics.vector.ImageVector,
    name: String,
    detail: String?,
    current: Boolean,
    connection: Connection? = null,
    buttons: @Composable () -> Unit,
) {
    Line(Modifier.fillMaxWidth().padding(horizontal = 14.dp, vertical = 10.dp), gap = Space.m) {
        Icon(icon, null, tint = if (current) colors.accent else colors.textSecondary, modifier = Modifier.size(20.dp))

        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Text(name, style = Type.callout, color = colors.text)

            if (connection != null) ConnectionPill(connection)
            if (detail != null) Text(detail, style = Type.caption, color = colors.textMuted)
        }

        Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) { buttons() }
    }
}
