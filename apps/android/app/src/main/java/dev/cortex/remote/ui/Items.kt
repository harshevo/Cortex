package dev.cortex.remote.ui

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.expandVertically
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.shrinkVertically
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.rounded.KeyboardArrowRight
import androidx.compose.material.icons.rounded.AutoAwesome
import androidx.compose.material.icons.rounded.Build
import androidx.compose.material.icons.rounded.Cancel
import androidx.compose.material.icons.rounded.Check
import androidx.compose.material.icons.rounded.CheckCircle
import androidx.compose.material.icons.rounded.Checklist
import androidx.compose.material.icons.rounded.Close
import androidx.compose.material.icons.rounded.ContentCopy
import androidx.compose.material.icons.rounded.Error
import androidx.compose.material.icons.rounded.Psychology
import androidx.compose.material.icons.rounded.RadioButtonUnchecked
import androidx.compose.material.icons.rounded.Verified
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.rotate
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import dev.cortex.remote.AppModel
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import uniffi.cortex_mobile.ItemView
import uniffi.cortex_mobile.SummaryTone
import uniffi.cortex_mobile.ToolDetailView
import uniffi.cortex_mobile.ToolView

/** What a row needs from the screen around it. */
class RowContext(
    val model: AppModel,
    val chat: String,
    /** The name of the model the chat uses, for the line under a response. */
    val modelName: String,
    /** Shows a text whole, to select from and copy. */
    val select: (String) -> Unit,
)

/** One row of the conversation. */
@Composable
fun RowView(row: Row, context: RowContext) {
    when (row) {
        is UserRow -> UserBubble(row, context)

        is ThoughtRow -> ThoughtFold(row.text, row.live)

        is BlockRow -> BlockView(
            row.block,
            Modifier.fillMaxWidth().padding(top = if (row.first) Space.s else 0.dp),
            onLongPress = row.source?.let { source -> { context.select(source) } },
        )

        is MetaRow -> MetaLine(row, context.modelName)

        is ItemRow -> ItemRowView(row.item, context)
    }
}

@Composable
private fun ItemRowView(item: ItemView, context: RowContext) {
    when (item) {
        is ItemView.Briefing -> Fold(Icons.Rounded.AutoAwesome, "Context from ${item.source}", colors.textSecondary) {
            Text(item.text, style = Type.footnote, color = colors.textSecondary)
        }

        is ItemView.Tools -> ToolGroup(item, context)

        is ItemView.Checks -> Fold(
            icon = if (item.passed) Icons.Rounded.Verified else Icons.Rounded.Cancel,
            title = if (item.passed) "The project's checks pass" else "The project's checks fail",
            tint = if (item.passed) colors.good else colors.bad,
            startsOpen = !item.passed,
        ) {
            Column(verticalArrangement = Arrangement.spacedBy(Space.s)) {
                for (check in item.checks) {
                    Text(
                        (if (check.passed) "✓ " else "✗ ") + check.command,
                        style = Type.mono,
                        color = if (check.passed) colors.good else colors.bad,
                    )

                    if (check.output.isNotEmpty()) {
                        CodeBlock(check.output, language = "output", lines = 30)
                    }
                }
            }
        }

        is ItemView.Review -> Fold(
            icon = Icons.Rounded.Checklist,
            title = if (item.sentBack) "Reviewed: sent back" else "Reviewed against the request",
            tint = if (item.sentBack) colors.warning else colors.good,
        ) {
            Column(verticalArrangement = Arrangement.spacedBy(Space.s)) {
                for (requirement in item.requirements) {
                    Row(horizontalArrangement = Arrangement.spacedBy(Space.s)) {
                        Icon(
                            if (requirement.met) Icons.Rounded.CheckCircle else Icons.Rounded.RadioButtonUnchecked,
                            null,
                            tint = if (requirement.met) colors.good else colors.warning,
                            modifier = Modifier.padding(top = 1.dp).size(16.dp),
                        )

                        Column {
                            Text(requirement.text, style = Type.footnote, color = colors.text)

                            if (requirement.why.isNotEmpty()) {
                                Text(requirement.why, style = Type.footnote, color = colors.textMuted)
                            }
                        }
                    }
                }
            }
        }

        is ItemView.Notice -> Text(
            item.text,
            style = Type.footnote,
            color = if (item.bad) colors.bad else colors.textMuted,
            modifier = Modifier.fillMaxWidth().padding(vertical = Space.xs),
        )

        is ItemView.Summary -> SummaryLine(item)

        // Drawn by rows of their own.
        is ItemView.User, is ItemView.Assistant -> {}
    }
}

/**
 * The person's instruction, in a bubble as narrow as its words allow:
 * the width is found by measuring, so a two-line message is set as two
 * even lines instead of a full one and a word.
 */
@Composable
private fun UserBubble(row: UserRow, context: RowContext) {
    Column(
        Modifier.fillMaxWidth().padding(start = 48.dp, top = Space.m, bottom = Space.xs),
        horizontalAlignment = Alignment.End,
        verticalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        if (row.images > 0) {
            Row(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                repeat(row.images) { index -> Thumbnail(row, index, context) }
            }
        }

        if (row.text.isNotEmpty()) {
            ProseText(
                AnnotatedString(row.text),
                Type.body.copy(color = colors.text),
                Modifier
                    .clip(Shapes.bubble)
                    .background(colors.raised)
                    .padding(horizontal = 14.dp, vertical = 10.dp),
                fitted = true,
                onLongPress = { context.select(row.text) },
            )
        }
    }
}

@Composable
private fun Thumbnail(row: UserRow, index: Int, context: RowContext) {
    val side = with(LocalDensity.current) { 72.dp.roundToPx() }
    val pictures = context.model.pictures

    val picture by produceState(pictures.kept(dev.cortex.remote.Pictures.key(context.chat, row.id, index, row.text)), row, index) {
        if (value == null) value = pictures.load(context.chat, row.id, index, row.text, side)
    }

    Box(Modifier.size(72.dp).clip(Shapes.field).background(colors.sunken)) {
        picture?.let { Image(it, null, contentScale = ContentScale.Crop, modifier = Modifier.size(72.dp)) }
    }
}

/** The model's reasoning: a line that unfolds. While it is written, its last words show. */
@Composable
private fun ThoughtFold(text: String, live: Boolean) {
    val detail = if (live) text.takeLast(90).substringAfter(' ').replace('\n', ' ') else null

    Fold(
        icon = Icons.Rounded.Psychology,
        title = if (live) "Thinking" else "Thought",
        tint = colors.model,
        detail = detail,
    ) {
        Text(text, style = Type.footnote, color = colors.textSecondary)
    }
}

/** The quiet line under a response: the model, how long it took and how fast it came, and a copy button. */
@Composable
private fun MetaLine(row: MetaRow, modelName: String) {
    val clipboard = LocalClipboardManager.current
    val view = LocalView.current

    var copied by remember { mutableStateOf(false) }

    val words = buildList {
        if (modelName.isNotEmpty()) add(modelName)
        if (row.seconds > 0f) add("%.1f s".format(row.seconds))
        row.speed?.let { add("${it.toInt()} tok/s") }
    }

    Line(Modifier.fillMaxWidth().padding(top = Space.xs, bottom = Space.xs), gap = Space.s) {
        Text(words.joinToString(" · "), style = Type.caption, color = colors.textMuted, maxLines = 1, modifier = Modifier.weight(1f, fill = false))

        Box(
            Modifier
                .size(28.dp)
                .pressable {
                    clipboard.setText(AnnotatedString(row.source))
                    Haptics.confirm(view)

                    copied = true
                },
            contentAlignment = Alignment.Center,
        ) {
            Icon(
                if (copied) Icons.Rounded.Check else Icons.Rounded.ContentCopy,
                "Copy the response",
                tint = if (copied) colors.good else colors.textMuted,
                modifier = Modifier.size(14.dp),
            )
        }
    }
}

/** A line that unfolds to show what is behind it. */
@Composable
fun Fold(
    icon: ImageVector,
    title: String,
    tint: Color,
    detail: String? = null,
    startsOpen: Boolean = false,
    busy: Boolean = false,
    content: @Composable () -> Unit,
) {
    var open by rememberSaveable { mutableStateOf(startsOpen) }

    val turn by animateFloatAsState(if (open) 90f else 0f, settle(), label = "fold")

    Column(Modifier.fillMaxWidth()) {
        Line(
            Modifier
                .fillMaxWidth()
                .heightIn(min = 34.dp)
                .pressable { open = !open },
        ) {
            Box(Modifier.size(17.dp), contentAlignment = Alignment.Center) {
                if (busy) {
                    CircularProgressIndicator(Modifier.size(13.dp), color = tint, strokeWidth = 1.5.dp)
                } else {
                    Icon(icon, null, tint = tint, modifier = Modifier.size(17.dp))
                }
            }

            Text(title, style = Type.subhead, color = colors.textSecondary, maxLines = 1, overflow = TextOverflow.Ellipsis)

            if (detail != null) {
                Text(
                    detail,
                    style = Type.caption,
                    color = colors.textMuted,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.weight(1f, fill = false),
                )
            }

            Icon(
                Icons.AutoMirrored.Rounded.KeyboardArrowRight,
                null,
                tint = colors.textMuted,
                modifier = Modifier.size(16.dp).rotate(turn),
            )
        }

        AnimatedVisibility(open, enter = expandVertically(settle()) + fadeIn(), exit = shrinkVertically(settle()) + fadeOut()) {
            Box(Modifier.padding(start = 25.dp, top = Space.xs, bottom = Space.s)) {
                content()
            }
        }
    }
}

/** Tool calls that came one after another, behind one line that says what they did. */
@Composable
private fun ToolGroup(item: ItemView.Tools, context: RowContext) {
    val tools = item.tools
    val failed = tools.count { it.failed }
    val running = tools.firstOrNull { it.output == null }

    Fold(
        icon = Icons.Rounded.Build,
        title = toolsTitle(tools),
        tint = if (failed > 0) colors.warning else colors.accent,
        detail = when {
            running != null -> running.summary
            failed > 0 -> "$failed failed"
            else -> null
        },
        busy = running != null,
    ) {
        Column(verticalArrangement = Arrangement.spacedBy(2.dp)) {
            tools.forEachIndexed { index, tool ->
                ToolRow(tool, item.id, index, context)
            }
        }
    }
}

/** What was done, in the words a person would use. */
private fun toolsTitle(tools: List<ToolView>): String {
    fun kind(name: String) = when (name) {
        "read_file", "outline", "read_symbol", "list_files" -> "read"
        "search", "semantic_search", "find_references", "file_dependencies", "recall" -> "search"
        "edit_file", "write_file" -> "edit"
        "shell", "checks" -> "run"
        "browser", "browser_view" -> "browse"
        else -> "other"
    }

    fun s(count: Int) = if (count == 1) "" else "s"

    val line = tools.groupingBy { kind(it.name) }.eachCount().map { (kind, count) ->
        when (kind) {
            "read" -> "read $count file${s(count)}"
            "search" -> "searched $count time${s(count)}"
            "edit" -> "made $count edit${s(count)}"
            "run" -> "ran $count command${s(count)}"
            "browse" -> "used the browser $count time${s(count)}"
            else -> "used $count tool${s(count)}"
        }
    }.joinToString(", ")

    return line.replaceFirstChar { it.uppercase() }
}

/**
 * One call: its name and what it was about in a line. Unfolded, what it
 * was handed and what it answered are fetched then, and not before.
 */
@Composable
private fun ToolRow(tool: ToolView, item: ULong, index: Int, context: RowContext) {
    var open by rememberSaveable { mutableStateOf(false) }

    Column {
        Line(
            Modifier
                .fillMaxWidth()
                .heightIn(min = 32.dp)
                .pressable { open = !open },
        ) {
            Box(Modifier.width(14.dp), contentAlignment = Alignment.Center) {
                when {
                    tool.output == null -> CircularProgressIndicator(
                        Modifier.size(11.dp),
                        color = colors.textSecondary,
                        strokeWidth = 1.5.dp,
                    )

                    tool.failed -> Icon(Icons.Rounded.Close, null, tint = colors.bad, modifier = Modifier.size(13.dp))
                    else -> Icon(Icons.Rounded.Check, null, tint = colors.good, modifier = Modifier.size(13.dp))
                }
            }

            Text(tool.name, style = Type.monoSmall.copy(fontWeight = FontWeight.SemiBold), color = colors.textSecondary)

            Text(
                tool.summary,
                style = Type.monoSmall,
                color = colors.textMuted,
                maxLines = 1,
                overflow = TextOverflow.MiddleEllipsis,
                modifier = Modifier.weight(1f),
            )

            if (tool.seconds >= 0.1f) {
                Text("%.1fs".format(tool.seconds), style = Type.monoSmall, color = colors.textMuted)
            }
        }

        AnimatedVisibility(open) {
            val detail by produceState<ToolDetailView?>(null, item, index, tool.output != null) {
                value = withContext(Dispatchers.IO) { context.model.core.toolDetail(context.chat, item, index.toUInt()) }
            }

            Column(Modifier.padding(vertical = Space.xs), verticalArrangement = Arrangement.spacedBy(Space.s)) {
                when (val shown = detail) {
                    null -> Text("…", style = Type.monoSmall, color = colors.textMuted)

                    else -> {
                        CodeBlock(shown.input, language = "input", lines = 20)

                        shown.output?.takeIf { it.isNotEmpty() }?.let { CodeBlock(it, language = "output", lines = 30) }
                    }
                }
            }
        }
    }
}

@Composable
private fun SummaryLine(item: ItemView.Summary) {
    val (tint, icon) = when (item.tone) {
        SummaryTone.GOOD -> colors.good to Icons.Rounded.CheckCircle
        SummaryTone.WARNING -> colors.warning to Icons.Rounded.Error
        SummaryTone.BAD -> colors.bad to Icons.Rounded.Cancel
    }

    Column(Modifier.fillMaxWidth().padding(top = Space.s, bottom = Space.xs), verticalArrangement = Arrangement.spacedBy(6.dp)) {
        Line {
            Icon(icon, null, tint = tint, modifier = Modifier.size(18.dp))

            Text(item.headline, style = Type.subhead.copy(fontWeight = FontWeight.SemiBold), color = tint)
        }

        Text(
            "${item.steps} steps · ${compact(item.tokens)} tokens · ${"%.1f".format(item.seconds)} s",
            style = Type.caption,
            color = colors.textMuted,
        )

        HorizontalDivider(color = colors.border)
    }
}
