package dev.cortex.remote.ui.settings

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.Close
import androidx.compose.material.icons.rounded.Search
import androidx.compose.material.icons.rounded.SearchOff
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.unit.dp
import dev.cortex.remote.AppModel
import dev.cortex.remote.ui.EmptyState
import dev.cortex.remote.ui.Gutter
import dev.cortex.remote.ui.Line
import dev.cortex.remote.ui.Shapes
import dev.cortex.remote.ui.Space
import dev.cortex.remote.ui.TopBar
import dev.cortex.remote.ui.Type
import dev.cortex.remote.ui.colors
import dev.cortex.remote.ui.measure
import dev.cortex.remote.ui.pressable

/**
 * Every setting, in one screen of folding cards. Each card is made in a
 * file of its own; a search box above them finds a setting in any of
 * them and unfolds the cards it is in.
 *
 * [chat] is the chat whose model and capabilities are shown, when one is
 * open; otherwise the first. [focus] is a card to start unfolded.
 */
@Composable
fun SettingsScreen(model: AppModel, chat: String?, focus: String?, back: () -> Unit) {
    val target = chat ?: model.state.chats.firstOrNull()?.id
    val settings = target?.let { model.settings[it] ?: remember(it) { model.core.settings(it) } }

    var query by rememberSaveable { mutableStateOf("") }

    val open = remember { mutableStateMapOf<String, Boolean>() }

    // Opened at a card, as from the chat's model chip: that card unfolds.
    LaunchedEffect(focus) {
        focus?.let { open[it] = true }
    }

    val sections = listOf(
        accountSection(model, target),
        modelSection(model, target, settings),
        appearanceSection(model),
        permissionsSection(model, target, settings),
        capabilitiesSection(model, target, settings),
        desktopsSection(model),
        aboutSection(model),
    )

    val shown = sections.filter { it.matches(query) }

    Column(
        Modifier
            .fillMaxSize()
            .background(colors.background)
            .navigationBarsPadding()
            .imePadding(),
    ) {
        TopBar("Settings", model.chat(target ?: "")?.title?.let { "For $it" }, back = back)

        LazyColumn(
            Modifier.fillMaxSize(),
            contentPadding = PaddingValues(bottom = Space.xxl),
            verticalArrangement = Arrangement.spacedBy(Space.s),
        ) {
            item("search") {
                SearchField(query, Modifier.measure().padding(horizontal = Gutter, vertical = Space.xs)) { query = it }
            }

            items(shown, key = { it.id }) { section ->
                Box(Modifier.measure().padding(horizontal = Gutter)) {
                    SectionCard(
                        section,
                        open = query.isNotBlank() || open[section.id] == true,
                        toggle = { open[section.id] = open[section.id] != true },
                        query = query,
                    )
                }
            }

            if (shown.isEmpty()) {
                item("nothing") {
                    EmptyState(Icons.Rounded.SearchOff, "Nothing found", "No setting mentions “${query.trim()}”.")
                }
            }
        }
    }
}

@Composable
private fun SearchField(value: String, modifier: Modifier, change: (String) -> Unit) {
    Line(
        modifier
            .fillMaxWidth()
            .heightIn(min = 44.dp)
            .background(colors.sunken, Shapes.field)
            .padding(horizontal = Space.m),
    ) {
        Icon(Icons.Rounded.Search, null, tint = colors.textMuted, modifier = Modifier.size(18.dp))

        Box(Modifier.weight(1f)) {
            if (value.isEmpty()) {
                Text("Search settings", style = Type.body, color = colors.textMuted)
            }

            BasicTextField(
                value = value,
                onValueChange = change,
                singleLine = true,
                textStyle = Type.body.copy(color = colors.text),
                cursorBrush = SolidColor(colors.accent),
                modifier = Modifier.fillMaxWidth(),
            )
        }

        if (value.isNotEmpty()) {
            Icon(
                Icons.Rounded.Close,
                "Clear",
                tint = colors.textMuted,
                modifier = Modifier.size(18.dp).pressable { change("") },
            )
        }
    }
}
