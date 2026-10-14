package dev.cortex.remote.ui.settings

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.CheckCircle
import androidx.compose.material.icons.rounded.Palette
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import dev.cortex.remote.AppModel
import dev.cortex.remote.TextSize
import dev.cortex.remote.ui.Dark
import dev.cortex.remote.ui.FOLLOW_SYSTEM
import dev.cortex.remote.ui.Light
import dev.cortex.remote.ui.Palette
import dev.cortex.remote.ui.SectionHeading
import dev.cortex.remote.ui.Shapes
import dev.cortex.remote.ui.Space
import dev.cortex.remote.ui.Themes
import dev.cortex.remote.ui.Type
import dev.cortex.remote.ui.colors
import dev.cortex.remote.ui.pressable

/** The theme and the size of the text. Both apply the moment they are chosen. */
@Composable
fun appearanceSection(model: AppModel): Section {
    val look = model.look

    return Section(
        id = "appearance",
        title = "Appearance",
        icon = Icons.Rounded.Palette,
        summary = "${if (look.theme == FOLLOW_SYSTEM) "Follows the system" else look.theme} · ${look.textSize.label} text",
        terms = listOf("theme", "dark", "light", "colour", "color", "text size", "font") + Themes.map { it.name },
    ) { query ->
        Column(Modifier.padding(14.dp), verticalArrangement = Arrangement.spacedBy(Space.m)) {
            if (matches(query, "text size", "font", "appearance", "larger", "smaller")) {
                SectionHeading("Text size")

                Segments(TextSize.entries.map { it.name }, look.textSize.name, { TextSize.valueOf(it).label }) {
                    look.choose(TextSize.valueOf(it))
                }
            }

            SectionHeading("Theme")

            Swatches(query, look.theme) { look.choose(it) }
        }
    }
}

/** A small card for every theme, drawn in its own colours. */
@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun Swatches(query: String, chosen: String, choose: (String) -> Unit) {
    val choices = listOf(FOLLOW_SYSTEM) + Themes.map { it.name }

    val shown = choices.filter { name ->
        matches(query, name, "theme", "appearance", "dark", "light") ||
            (name == FOLLOW_SYSTEM && matches(query, "system", "follow"))
    }

    BoxWithConstraints(Modifier.fillMaxWidth()) {
        val gap = Space.s
        val columns = (maxWidth / 150.dp).toInt().coerceAtLeast(2)
        // A little under the exact share, so that rounding to whole dots never pushes a card to the next line.
        val width = (maxWidth - gap * (columns - 1)) / columns - 1.dp

        FlowRow(horizontalArrangement = Arrangement.spacedBy(gap), verticalArrangement = Arrangement.spacedBy(gap)) {
            for (name in shown) {
                Swatch(name, name == chosen, width) { choose(name) }
            }
        }
    }
}

@Composable
private fun Swatch(name: String, chosen: Boolean, width: Dp, choose: () -> Unit) {
    Column(
        Modifier
            .width(width)
            .pressable(onClick = choose)
            .clip(Shapes.field)
            .border(if (chosen) 2.dp else 1.dp, if (chosen) colors.accent else colors.border, Shapes.field)
            .padding(6.dp),
        verticalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Box(Modifier.fillMaxWidth().height(64.dp).clip(Shapes.small)) {
            if (name == FOLLOW_SYSTEM) {
                Row(Modifier.fillMaxSize()) {
                    Box(Modifier.weight(1f).fillMaxHeight()) { Preview(Light) }
                    Box(Modifier.weight(1f).fillMaxHeight()) { Preview(Dark) }
                }
            } else {
                Preview(Themes.first { it.name == name })
            }
        }

        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(Space.xs)) {
            Text(
                if (name == FOLLOW_SYSTEM) "Follow system" else name,
                style = Type.caption.copy(fontWeight = if (chosen) FontWeight.SemiBold else FontWeight.Normal),
                color = colors.text,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.weight(1f),
            )

            if (chosen) Icon(Icons.Rounded.CheckCircle, "In use", tint = colors.accent, modifier = Modifier.size(14.dp))
        }
    }
}

/** A theme in small: its ground, a card on it with a line of text and a quieter one, the accent, good and bad. */
@Composable
private fun Preview(palette: Palette) {
    Box(Modifier.fillMaxSize().background(palette.background).padding(6.dp)) {
        Column(
            Modifier
                .fillMaxWidth()
                .clip(Shapes.small)
                .background(palette.surface)
                .border(1.dp, palette.border, Shapes.small)
                .padding(5.dp),
            verticalArrangement = Arrangement.spacedBy(3.dp),
        ) {
            Box(Modifier.fillMaxWidth(0.8f).height(4.dp).clip(Shapes.pill).background(palette.text))
            Box(Modifier.fillMaxWidth(0.55f).height(4.dp).clip(Shapes.pill).background(palette.textMuted))

            Row(horizontalArrangement = Arrangement.spacedBy(3.dp), verticalAlignment = Alignment.CenterVertically) {
                Box(Modifier.width(18.dp).height(8.dp).clip(Shapes.pill).background(palette.accent))
                Box(Modifier.size(6.dp).clip(Shapes.pill).background(palette.good))
                Box(Modifier.size(6.dp).clip(Shapes.pill).background(palette.bad))
                Box(Modifier.size(6.dp).clip(Shapes.pill).background(palette.model))
            }
        }
    }
}
