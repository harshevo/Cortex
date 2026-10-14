package dev.cortex.remote.ui.settings

import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.PanTool
import androidx.compose.runtime.Composable
import dev.cortex.remote.AppModel
import dev.cortex.remote.ui.colors
import uniffi.cortex_mobile.SettingsView

/** Whether the agent asks before it changes anything. */
@Composable
fun permissionsSection(model: AppModel, chat: String?, settings: SettingsView?): Section = Section(
    id = "permissions",
    title = "Permissions",
    icon = Icons.Rounded.PanTool,
    summary = when {
        settings == null -> "No chat open"
        settings.approveEverything -> "Acts without asking"
        else -> "Asks before changing anything"
    },
    terms = listOf("approve", "ask", "permission", "allow", "without asking"),
) { _ ->
    if (chat == null || settings == null) {
        Note("Connect to the agent on this phone or a computer to choose.")
    } else {
        SwitchRow(
            title = "Act without asking",
            help = "Run commands and edit files without waiting for an answer. Off, every change waits for Allow, Always or Deny.",
            on = settings.approveEverything,
        ) { model.core.setApproveEverything(chat, it) }

        if (settings.approveEverything) {
            Note("Everything the agent does is done at once, in this chat's project.", color = colors.warning)
        }
    }
}
