package dev.cortex.remote.ui

import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.PickVisualMediaRequest
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.animateContentSize
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.scaleIn
import androidx.compose.animation.scaleOut
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.rounded.Reply
import androidx.compose.material.icons.rounded.Add
import androidx.compose.material.icons.rounded.ArrowUpward
import androidx.compose.material.icons.rounded.AutoAwesome
import androidx.compose.material.icons.rounded.Bolt
import androidx.compose.material.icons.rounded.Check
import androidx.compose.material.icons.rounded.Close
import androidx.compose.material.icons.rounded.ExpandMore
import androidx.compose.material.icons.rounded.Memory
import androidx.compose.material.icons.rounded.PanTool
import androidx.compose.material.icons.rounded.Refresh
import androidx.compose.material.icons.rounded.Speed
import androidx.compose.material.icons.rounded.Stop
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.dp
import dev.cortex.remote.AppModel
import dev.cortex.remote.ChatFeed
import dev.cortex.remote.Pictures
import kotlinx.coroutines.launch
import uniffi.cortex_mobile.Picture

private enum class Sending { IDLE, SEND, STEER, STOP }

/** The most lines the composer grows to before its text scrolls. */
private const val MOST_LINES = 7

/**
 * Where an instruction is written and sent, and a run is stopped: the
 * field grows with the text, the send button turns into stop while a run
 * works, what the mind expects to be asked next waits as faint text in
 * the field, and the model, its effort and whether it asks first sit in
 * a row of chips under it.
 */
@Composable
fun Composer(model: AppModel, chat: String, feed: ChatFeed, ready: Boolean, openSettings: (String?) -> Unit) {
    val context = LocalContext.current
    val view = LocalView.current
    val scope = rememberCoroutineScope()

    var draft by rememberSaveable(chat) { mutableStateOf("") }
    var focused by remember { mutableStateOf(false) }

    val pictures = remember(chat) { mutableStateListOf<Picture>() }

    val picker = rememberLauncherForActivityResult(ActivityResultContracts.PickMultipleVisualMedia(4)) { uris ->
        scope.launch {
            pictures.clear()
            pictures.addAll(uris.mapNotNull { Pictures.forSending(context, it) })
        }
    }

    val running = feed.running
    val hasText = draft.isNotBlank()

    val mode = when {
        running && hasText -> Sending.STEER
        running -> Sending.STOP
        hasText && ready -> Sending.SEND
        else -> Sending.IDLE
    }

    fun act() {
        val text = draft.trim()

        when (mode) {
            Sending.SEND -> {
                model.core.submit(chat, text, pictures.toList())

                draft = ""
                pictures.clear()
            }

            Sending.STEER -> {
                model.core.steer(chat, text)

                draft = ""
            }

            Sending.STOP -> {
                Haptics.warn(view)
                model.core.stop(chat)
            }

            Sending.IDLE -> {}
        }
    }

    // What the mind expects to be asked next, while it still fits what is typed.
    val suggestion = feed.suggestion?.takeIf { !running && it.startsWith(draft, ignoreCase = true) && it.length > draft.length }

    Column(verticalArrangement = Arrangement.spacedBy(Space.s)) {
        AnimatedVisibility(feed.canContinue && !running) {
            Chip(Icons.Rounded.Refresh, "Continue where it stopped", colors.accent, colors.accent.copy(alpha = 0.12f)) {
                model.core.carryOn(chat)
            }
        }

        if (pictures.isNotEmpty()) {
            Row(horizontalArrangement = Arrangement.spacedBy(Space.s)) {
                pictures.forEachIndexed { index, picture -> Chosen(picture) { pictures.removeAt(index) } }
            }
        }

        val outline by animateColorAsState(
            if (focused) colors.accent.copy(alpha = 0.6f) else colors.border,
            label = "outline",
        )

        Column(
            Modifier
                .fillMaxWidth()
                .clip(Shapes.bubble)
                .background(colors.surface)
                .border(1.dp, outline, Shapes.bubble)
                .animateContentSize(settle())
                .padding(horizontal = Space.s, vertical = 5.dp),
        ) {
            Row(verticalAlignment = Alignment.Bottom, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                Box(
                    Modifier.size(38.dp).pressable(!running) {
                        picker.launch(PickVisualMediaRequest(ActivityResultContracts.PickVisualMedia.ImageOnly))
                    },
                    contentAlignment = Alignment.Center,
                ) {
                    Icon(Icons.Rounded.Add, "Attach a picture", tint = colors.textSecondary, modifier = Modifier.size(22.dp))
                }

                Box(
                    Modifier
                        .weight(1f)
                        .heightIn(min = 38.dp)
                        .padding(vertical = 7.dp),
                    contentAlignment = Alignment.CenterStart,
                ) {
                    when {
                        // The rest of the expected request, after what is typed, in faint letters.
                        suggestion != null -> Text(
                            buildAnnotatedString {
                                withStyle(SpanStyle(color = Color.Transparent)) { append(suggestion.take(draft.length)) }
                                withStyle(SpanStyle(color = colors.textMuted)) { append(suggestion.drop(draft.length)) }
                            },
                            style = Type.body,
                            maxLines = MOST_LINES,
                        )

                        draft.isEmpty() -> Text(
                            when {
                                !ready -> "Cortex is starting…"
                                running -> "Tell Cortex something while it works"
                                else -> "Ask Cortex to change something"
                            },
                            style = Type.body,
                            color = colors.textMuted,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                    }

                    BasicTextField(
                        value = draft,
                        onValueChange = { draft = it },
                        textStyle = Type.body.copy(color = colors.text),
                        cursorBrush = SolidColor(colors.accent),
                        maxLines = MOST_LINES,
                        modifier = Modifier.fillMaxWidth().onFocusChanged { focused = it.isFocused },
                    )
                }

                SendButton(mode, ::act)
            }

            if (suggestion != null) {
                Line(
                    Modifier
                        .padding(start = 44.dp, bottom = Space.xs)
                        .pressable {
                            Haptics.tap(view)
                            draft = suggestion
                        }
                        .clip(Shapes.pill)
                        .background(colors.model.copy(alpha = 0.14f))
                        .padding(horizontal = 10.dp, vertical = 5.dp),
                    gap = 5.dp,
                ) {
                    Icon(Icons.Rounded.AutoAwesome, null, tint = colors.model, modifier = Modifier.size(13.dp))
                    Text("Tap to use the suggestion", style = Type.caption, color = colors.textSecondary)
                }
            }
        }

        ChipsRow(model, chat, feed, openSettings)
    }
}

/** A chosen picture waiting to be sent, made small for showing. */
@Composable
private fun Chosen(picture: Picture, remove: () -> Unit) {
    val side = with(LocalDensity.current) { 56.dp.roundToPx() }
    val bitmap = remember(picture) { Pictures.decode(picture.data, side)?.asImageBitmap() }

    Box {
        if (bitmap != null) {
            Image(bitmap, null, contentScale = ContentScale.Crop, modifier = Modifier.size(56.dp).clip(Shapes.small))
        }

        Box(
            Modifier
                .align(Alignment.TopEnd)
                .size(20.dp)
                .pressable(onClick = remove)
                .clip(Shapes.pill)
                .background(colors.text.copy(alpha = 0.7f)),
            contentAlignment = Alignment.Center,
        ) {
            Icon(Icons.Rounded.Close, "Remove", tint = colors.background, modifier = Modifier.size(12.dp))
        }
    }
}

/** The model, its effort and whether it asks first, each a chip that opens its choices; and the run's numbers. */
@Composable
private fun ChipsRow(model: AppModel, chat: String, feed: ChatFeed, openSettings: (String?) -> Unit) {
    val settings = model.settings[chat] ?: return
    val meter = feed.meter

    Row(verticalAlignment = Alignment.CenterVertically) {
        Row(
            Modifier.weight(1f).horizontalScroll(rememberScrollState()),
            horizontalArrangement = Arrangement.spacedBy(6.dp),
        ) {
            Choice(
                icon = Icons.Rounded.Memory,
                label = settings.model.ifEmpty { "Model" },
                tint = colors.model,
                options = settings.models.map { "${it.providerLabel}\u0000${it.model}" },
                chosen = "${settings.models.firstOrNull { it.model == settings.model && it.provider == settings.provider }?.providerLabel}\u0000${settings.model}",
                name = { it.substringAfter('\u0000') },
                group = { it.substringBefore('\u0000') },
                more = "All models and accounts…" to { openSettings("model") },
            ) { option ->
                settings.models.firstOrNull { "${it.providerLabel}\u0000${it.model}" == option }?.let {
                    model.core.setModel(chat, it.provider, it.model)
                }
            }

            if (settings.takesEffort && settings.efforts.isNotEmpty()) {
                Choice(
                    icon = Icons.Rounded.Speed,
                    label = effortName(settings.effort),
                    tint = colors.textSecondary,
                    options = settings.efforts,
                    chosen = settings.effort,
                    name = ::effortName,
                ) { model.core.setEffort(chat, it) }
            }

            Choice(
                icon = if (settings.approveEverything) Icons.Rounded.Bolt else Icons.Rounded.PanTool,
                label = if (settings.approveEverything) "Acts without asking" else "Asks first",
                tint = if (settings.approveEverything) colors.warning else colors.textSecondary,
                options = listOf("ask", "act"),
                chosen = if (settings.approveEverything) "act" else "ask",
                name = { if (it == "act") "Act without asking" else "Ask before changing anything" },
            ) { model.core.setApproveEverything(chat, it == "act") }
        }

        val numbers = buildList {
            meter.speed?.let { add("$it tok/s") }

            if (meter.inputTokens + meter.outputTokens > 0uL) {
                add("${compact(meter.inputTokens)}↑ ${compact(meter.outputTokens)}↓")
            }
        }

        if (numbers.isNotEmpty()) {
            Text(
                numbers.joinToString("  "),
                style = Type.monoSmall,
                color = colors.textMuted,
                maxLines = 1,
                modifier = Modifier.padding(start = Space.s),
            )
        }
    }
}

fun effortName(effort: String) = if (effort == "xhigh") "Extra high" else effort.replaceFirstChar(Char::uppercase)

/** A chip that shows a choice and opens the list of the others. */
@Composable
private fun Choice(
    icon: ImageVector,
    label: String,
    tint: Color,
    options: List<String>,
    chosen: String,
    name: (String) -> String,
    group: ((String) -> String)? = null,
    more: Pair<String, () -> Unit>? = null,
    pick: (String) -> Unit,
) {
    var open by remember { mutableStateOf(false) }

    Box {
        Line(
            Modifier
                .pressable { open = true }
                .clip(Shapes.pill)
                .background(colors.surface)
                .border(1.dp, colors.border.copy(alpha = 0.7f), Shapes.pill)
                .heightIn(min = 30.dp)
                .padding(start = 10.dp, end = 6.dp),
            gap = 5.dp,
        ) {
            Icon(icon, null, tint = tint, modifier = Modifier.size(14.dp))
            Text(label, style = Type.caption.copy(fontWeight = FontWeight.Medium), color = colors.textSecondary, maxLines = 1)
            Icon(Icons.Rounded.ExpandMore, null, tint = colors.textMuted, modifier = Modifier.size(14.dp))
        }

        DropdownMenu(open, { open = false }, containerColor = colors.raised) {
            var heading: String? = null

            for (option in options) {
                val within = group?.invoke(option)

                if (within != null && within != heading) {
                    heading = within

                    Text(
                        within.uppercase(),
                        style = Type.heading,
                        color = colors.textMuted,
                        modifier = Modifier.padding(horizontal = 14.dp, vertical = Space.s),
                    )
                }

                DropdownMenuItem(
                    text = { Text(name(option), style = Type.callout, color = colors.text) },
                    trailingIcon = {
                        if (option == chosen) Icon(Icons.Rounded.Check, null, tint = colors.accent, modifier = Modifier.size(18.dp))
                    },
                    onClick = {
                        open = false
                        pick(option)
                    },
                )
            }

            more?.let { (text, action) ->
                DropdownMenuItem(
                    text = { Text(text, style = Type.callout, color = colors.accent) },
                    onClick = {
                        open = false
                        action()
                    },
                )
            }
        }
    }
}

@Composable
private fun Chip(icon: ImageVector, text: String, ink: Color, fill: Color, iconTint: Color = ink, onClick: () -> Unit) {
    Line(
        Modifier
            .pressable(onClick = onClick)
            .clip(Shapes.pill)
            .background(fill)
            .border(1.dp, colors.border.copy(alpha = 0.6f), Shapes.pill)
            .heightIn(min = 36.dp)
            .padding(horizontal = Space.m),
        gap = 6.dp,
    ) {
        Icon(icon, null, tint = iconTint, modifier = Modifier.size(15.dp))
        Text(text, style = Type.footnote, color = ink, maxLines = 1, overflow = TextOverflow.Ellipsis)
    }
}

/** One button that sends, steers or stops, and changes to say which. */
@Composable
private fun SendButton(mode: Sending, onClick: () -> Unit) {
    val fill by animateColorAsState(
        when (mode) {
            Sending.IDLE -> colors.raised
            Sending.SEND, Sending.STEER -> colors.accent
            Sending.STOP -> colors.bad
        },
        label = "send",
    )

    Box(
        Modifier
            .size(38.dp)
            .pressable(mode != Sending.IDLE, onClick)
            .clip(Shapes.pill)
            .background(fill),
        contentAlignment = Alignment.Center,
    ) {
        AnimatedContent(
            targetState = mode,
            transitionSpec = { (scaleIn() + fadeIn()) togetherWith (scaleOut() + fadeOut()) },
            label = "send-icon",
        ) { mode ->
            Icon(
                when (mode) {
                    Sending.IDLE, Sending.SEND -> Icons.Rounded.ArrowUpward
                    Sending.STEER -> Icons.AutoMirrored.Rounded.Reply
                    Sending.STOP -> Icons.Rounded.Stop
                },
                when (mode) {
                    Sending.STOP -> "Stop"
                    Sending.STEER -> "Tell the run"
                    else -> "Send"
                },
                tint = when (mode) {
                    Sending.IDLE -> colors.textMuted
                    Sending.STOP -> colors.background
                    else -> colors.onAccent
                },
                modifier = Modifier.size(20.dp),
            )
        }
    }
}
