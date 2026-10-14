package dev.cortex.remote.ui.settings

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.Key
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import dev.cortex.remote.AppModel
import dev.cortex.remote.ui.Space
import dev.cortex.remote.ui.colors

/**
 * Who the model of the agent on this phone is reached as: the provider,
 * the ChatGPT sign-in or an API key, and a hosted service's address.
 * Keys stay on the phone.
 */
@Composable
fun accountSection(model: AppModel, chat: String?): Section {
    val account = model.account

    val summary = when {
        account.provider == "codex" && account.signedIn -> "ChatGPT · signed in"
        account.provider == "codex" -> "ChatGPT · not signed in"
        account.hasKey -> "${label(model, account.provider)} · key kept on this phone"
        else -> "${label(model, account.provider)} · no key yet"
    }

    return Section(
        id = "account",
        title = "Account",
        icon = Icons.Rounded.Key,
        summary = summary,
        terms = listOf("provider", "sign in", "chatgpt", "api key", "address", "hosted", "anthropic", "sign out"),
    ) { _ -> AccountRows(model, chat) }
}

private fun label(model: AppModel, provider: String) =
    model.account.providers.firstOrNull { it.name == provider }?.label ?: provider

@Composable
private fun AccountRows(model: AppModel, chat: String?) {
    val account = model.account

    var key by remember(account.provider) { mutableStateOf("") }
    var address by remember(account.provider, account.address) { mutableStateOf(account.address) }

    Column(Modifier.padding(vertical = Space.s), verticalArrangement = Arrangement.spacedBy(Space.s)) {
        Note(
            if (model.state.onPhone) {
                "For the agent on this phone. Only the model is elsewhere."
            } else {
                "For the agent on this phone. A paired computer uses its own account."
            },
        )

        if (!model.state.onPhone) {
            SmallButton("Work on this phone", Modifier.padding(horizontal = 14.dp).fillMaxWidth(), strong = true) {
                model.core.usePhone()
            }
        }

        Segments(
            options = account.providers.map { it.name },
            selected = account.provider,
            name = { label(model, it) },
            modifier = Modifier.padding(horizontal = 14.dp),
        ) { provider ->
            // A provider is chosen by choosing one of its models.
            val offered = chat?.let { model.settings[it] }?.models?.firstOrNull { it.provider == provider }?.model

            if (chat != null && offered != null) model.core.setModel(chat, provider, offered)
        }

        Note("Model: ${account.model.ifEmpty { "none chosen" }}")

        when (account.provider) {
            "codex" -> {
                when {
                    account.signingIn -> Note("Waiting for the sign-in page…", color = colors.textSecondary)

                    account.signedIn -> Row(Modifier.padding(horizontal = 14.dp), horizontalArrangement = Arrangement.spacedBy(Space.s)) {
                        Note("Signed in to ChatGPT.", Modifier.weight(1f), color = colors.good)
                        SmallButton("Sign out") { model.core.signOut() }
                    }

                    else -> SmallButton("Sign in to ChatGPT", Modifier.padding(horizontal = 14.dp).fillMaxWidth(), strong = true) {
                        model.core.signIn()
                    }
                }

                account.signInProblem?.let { Note(it, color = colors.bad) }
            }

            else -> Column(Modifier.padding(horizontal = 14.dp), verticalArrangement = Arrangement.spacedBy(Space.s)) {
                if (account.provider == "hosted") {
                    Field(address, "https://…/v1") { address = it }

                    SmallButton("Save the address", Modifier.fillMaxWidth(), enabled = address.isNotBlank() && address != account.address) {
                        model.core.setAddress(address.trim())
                    }
                }

                Field(key, if (account.hasKey) "A key is kept · type to replace it" else "API key", secret = true) { key = it }

                SmallButton("Save the key", Modifier.fillMaxWidth(), strong = true, enabled = key.isNotBlank()) {
                    model.core.setKey(account.provider, key.trim())
                    key = ""
                }
            }
        }

        if (model.state.onPhone) {
            model.state.chats.firstOrNull()?.problem?.let { Note(it, color = colors.bad) }
        }
    }
}
