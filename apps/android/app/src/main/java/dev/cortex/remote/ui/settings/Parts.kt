package dev.cortex.remote.ui.settings

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.animateContentSize
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.expandVertically
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.shrinkVertically
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.rounded.KeyboardArrowRight
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.Switch
import androidx.compose.material3.SwitchDefaults
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.rotate
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.input.VisualTransformation
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import dev.cortex.remote.ui.Haptics
import dev.cortex.remote.ui.Line
import dev.cortex.remote.ui.Shapes
import dev.cortex.remote.ui.Space
import dev.cortex.remote.ui.Type
import dev.cortex.remote.ui.card
import dev.cortex.remote.ui.colors
import dev.cortex.remote.ui.inline
import dev.cortex.remote.ui.pressable
import dev.cortex.remote.ui.settle

/**
 * One part of the settings: a card with a heading that folds, and a line
 * that says how things stand while it is folded. Each lives in a file of
 * its own and is made by a function that returns one of these.
 */
class Section(
    val id: String,
    val title: String,
    val icon: ImageVector,
    /** How things stand, in a few words, shown while the card is folded. */
    val summary: String,
    /** Words a search finds the section by, besides its title. */
    val terms: List<String>,
    /** The rows. Handed what is being searched for, to leave out rows that do not match. */
    val content: @Composable (query: String) -> Unit,
)

/** Whether any of [words] holds what is searched for. Everything matches an empty search. */
fun matches(query: String, vararg words: String): Boolean =
    query.isBlank() || words.any { it.contains(query.trim(), ignoreCase = true) }

fun Section.matches(query: String): Boolean = matches(query, title, *terms.toTypedArray())

@Composable
fun SectionCard(section: Section, open: Boolean, toggle: () -> Unit, query: String) {
    val turn by animateFloatAsState(if (open) 90f else 0f, settle(), label = "section")

    Column(Modifier.fillMaxWidth().card(0.dp)) {
        Line(
            Modifier
                .fillMaxWidth()
                .heightIn(min = 56.dp)
                .clickable(onClick = toggle)
                .padding(horizontal = 14.dp),
            gap = Space.m,
        ) {
            Box(
                Modifier.size(30.dp).clip(Shapes.small).background(colors.accent.copy(alpha = 0.14f)),
                contentAlignment = Alignment.Center,
            ) {
                Icon(section.icon, null, tint = colors.accent, modifier = Modifier.size(17.dp))
            }

            Column(Modifier.weight(1f)) {
                Text(section.title, style = Type.bodyStrong, color = colors.text)

                if (!open && section.summary.isNotEmpty()) {
                    Text(section.summary, style = Type.caption, color = colors.textMuted, maxLines = 1, overflow = TextOverflow.Ellipsis)
                }
            }

            Icon(
                Icons.AutoMirrored.Rounded.KeyboardArrowRight,
                null,
                tint = colors.textMuted,
                modifier = Modifier.size(18.dp).rotate(turn),
            )
        }

        AnimatedVisibility(open, enter = expandVertically(settle()) + fadeIn(), exit = shrinkVertically(settle()) + fadeOut()) {
            Column {
                HorizontalDivider(color = colors.border.copy(alpha = 0.6f))

                section.content(query)
            }
        }
    }
}

/** A thin line between rows of a card. */
@Composable
fun Divider() {
    HorizontalDivider(Modifier.padding(start = 14.dp), color = colors.border.copy(alpha = 0.5f))
}

/** A row with words on the left and a switch on the right. Its help shows one line until tapped. */
@Composable
fun SwitchRow(
    title: String,
    help: String,
    on: Boolean,
    name: String? = null,
    enabled: Boolean = true,
    set: (Boolean) -> Unit,
) {
    val view = LocalView.current

    var expanded by remember { mutableStateOf(false) }

    Row(
        Modifier
            .fillMaxWidth()
            .clickable(indication = null, interactionSource = null) { expanded = !expanded }
            .animateContentSize(settle())
            .padding(horizontal = 14.dp, vertical = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(Space.m),
    ) {
        Column(Modifier.weight(1f), verticalArrangement = Arrangement.spacedBy(2.dp)) {
            Line(gap = 6.dp) {
                Text(title, style = Type.callout.copy(fontWeight = FontWeight.Medium), color = colors.text)

                if (name != null) {
                    Text(name, style = Type.monoSmall, color = colors.textMuted)
                }
            }

            if (help.isNotEmpty()) {
                Text(
                    inline(help),
                    style = Type.footnote,
                    color = colors.textSecondary,
                    maxLines = if (expanded) Int.MAX_VALUE else 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
        }

        Switch(
            checked = on,
            enabled = enabled,
            onCheckedChange = {
                Haptics.tap(view)
                set(it)
            },
            colors = SwitchDefaults.colors(
                checkedThumbColor = colors.onAccent,
                checkedTrackColor = colors.accent,
                uncheckedThumbColor = colors.textMuted,
                uncheckedTrackColor = colors.raised,
                uncheckedBorderColor = colors.border,
                disabledCheckedTrackColor = colors.accent.copy(alpha = 0.45f),
                disabledCheckedThumbColor = colors.onAccent.copy(alpha = 0.8f),
            ),
        )
    }
}

/** A row that names a thing and says what it is, and does something when tapped. */
@Composable
fun ValueRow(title: String, value: String, icon: ImageVector? = null, onClick: (() -> Unit)? = null) {
    Line(
        Modifier
            .fillMaxWidth()
            .heightIn(min = 48.dp)
            .then(if (onClick != null) Modifier.pressable(onClick = onClick) else Modifier)
            .padding(horizontal = 14.dp),
        gap = Space.m,
    ) {
        if (icon != null) Icon(icon, null, tint = colors.textSecondary, modifier = Modifier.size(18.dp))

        Text(title, style = Type.callout, color = colors.text, modifier = Modifier.weight(1f))

        Text(value, style = Type.footnote, color = colors.textMuted, maxLines = 1, overflow = TextOverflow.Ellipsis)

        if (onClick != null) {
            Icon(Icons.AutoMirrored.Rounded.KeyboardArrowRight, null, tint = colors.textMuted, modifier = Modifier.size(16.dp))
        }
    }
}

/** A short note under a group of rows. */
@Composable
fun Note(text: String, modifier: Modifier = Modifier, color: androidx.compose.ui.graphics.Color = colors.textMuted) {
    Text(text, style = Type.footnote, color = color, modifier = modifier.padding(horizontal = 14.dp, vertical = Space.s))
}

/** A field to type into, with a hint while it is empty. [secret] hides what is typed. */
@Composable
fun Field(value: String, hint: String, modifier: Modifier = Modifier, secret: Boolean = false, change: (String) -> Unit) {
    Box(
        modifier
            .fillMaxWidth()
            .clip(Shapes.small)
            .background(colors.sunken)
            .padding(horizontal = Space.m, vertical = 11.dp),
    ) {
        if (value.isEmpty()) {
            Text(hint, style = Type.mono, color = colors.textMuted)
        }

        BasicTextField(
            value = value,
            onValueChange = change,
            textStyle = Type.mono.copy(color = colors.text),
            cursorBrush = SolidColor(colors.accent),
            singleLine = true,
            visualTransformation = if (secret) PasswordVisualTransformation() else VisualTransformation.None,
            modifier = Modifier.fillMaxWidth(),
        )
    }
}

/** A small button inside a card. */
@Composable
fun SmallButton(text: String, modifier: Modifier = Modifier, strong: Boolean = false, enabled: Boolean = true, onClick: () -> Unit) {
    Box(
        modifier
            .heightIn(min = 40.dp)
            .pressable(enabled, onClick)
            .clip(Shapes.field)
            .background(if (strong) colors.accent else colors.raised)
            .padding(horizontal = 14.dp),
        contentAlignment = Alignment.Center,
    ) {
        Text(
            text,
            style = Type.footnote.copy(fontWeight = FontWeight.SemiBold),
            color = if (strong) colors.onAccent else colors.text,
            maxLines = 1,
        )
    }
}

/** A row of choices, one of them picked. */
@Composable
fun Segments(options: List<String>, selected: String, name: (String) -> String, modifier: Modifier = Modifier, pick: (String) -> Unit) {
    Row(
        modifier
            .fillMaxWidth()
            .clip(Shapes.small)
            .background(colors.sunken)
            .padding(3.dp),
        horizontalArrangement = Arrangement.spacedBy(2.dp),
    ) {
        for (option in options) {
            val on = option == selected

            Box(
                Modifier
                    .weight(1f)
                    .heightIn(min = 34.dp)
                    .pressable { pick(option) }
                    .clip(Shapes.small)
                    .background(if (on) colors.accent else colors.sunken),
                contentAlignment = Alignment.Center,
            ) {
                Text(
                    name(option),
                    style = Type.footnote.copy(fontWeight = if (on) FontWeight.SemiBold else FontWeight.Normal),
                    color = if (on) colors.onAccent else colors.textSecondary,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
        }
    }
}
