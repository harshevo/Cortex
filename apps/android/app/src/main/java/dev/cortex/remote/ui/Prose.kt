package dev.cortex.remote.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.Check
import androidx.compose.material.icons.rounded.CheckBox
import androidx.compose.material.icons.rounded.CheckBoxOutlineBlank
import androidx.compose.material.icons.rounded.ContentCopy
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.layout.Layout
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.Constraints
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.delay

/** A line of Markdown set as the app sets it: bold, italics, code and links. */
@Composable
fun inline(text: String): AnnotatedString = remember(text) { Markdown.inline(text) }.styled()

/**
 * One block of a response. [growing] is the block still being written,
 * the last of a response in progress.
 */
@Composable
fun BlockView(block: Block, modifier: Modifier = Modifier, growing: Boolean = false, onLongPress: (() -> Unit)? = null) {
    val body = Type.body.copy(color = colors.text)

    when (block) {
        is Block.Paragraph -> ProseText(
            block.text.styled(),
            body,
            modifier,
            growing = growing,
            caret = growing,
            onLongPress = onLongPress,
        )

        is Block.Heading -> ProseText(
            block.text.styled(),
            when (block.level) {
                1 -> Type.title(21).copy(color = colors.text)
                2 -> body.copy(fontSize = 18.sp, lineHeight = 25.sp, fontWeight = FontWeight.SemiBold)
                else -> body.copy(fontWeight = FontWeight.SemiBold)
            },
            modifier.padding(top = Space.s),
            growing = growing,
            onLongPress = onLongPress,
        )

        is Block.Entry -> Row(modifier.padding(start = (block.depth * 18).dp)) {
            when (block.checked) {
                null -> Text(
                    block.marker,
                    style = body.copy(color = colors.textMuted),
                    textAlign = TextAlign.End,
                    modifier = Modifier.width(if (block.marker == "•") 14.dp else 24.dp),
                )

                else -> Icon(
                    if (block.checked) Icons.Rounded.CheckBox else Icons.Rounded.CheckBoxOutlineBlank,
                    null,
                    tint = if (block.checked) colors.good else colors.textMuted,
                    modifier = Modifier.padding(top = 3.dp).size(18.dp),
                )
            }

            ProseText(
                block.text.styled(),
                body,
                Modifier.padding(start = Space.s),
                growing = growing,
                caret = growing,
                onLongPress = onLongPress,
            )
        }

        is Block.Quote -> {
            val bar = colors.accent.copy(alpha = 0.5f)

            ProseText(
                block.text.styled(),
                body.copy(color = colors.textSecondary),
                modifier
                    .drawBehind {
                        drawLine(bar, Offset(1.5.dp.toPx(), 0f), Offset(1.5.dp.toPx(), size.height), 3.dp.toPx())
                    }
                    .padding(start = 14.dp),
                growing = growing,
                caret = growing,
                onLongPress = onLongPress,
            )
        }

        is Block.Code -> CodeBlock(block.code, modifier, language = block.language)
        is Block.Table -> TableView(block, modifier)
        is Block.Rule -> HorizontalDivider(modifier.padding(vertical = Space.s), color = colors.border)
    }
}

/**
 * Code, in its own frame, scrolled sideways rather than wrapped, with
 * its language and a copy button on a bar above it. Past [lines] lines
 * the rest is folded away behind a row that shows it.
 */
@Composable
fun CodeBlock(code: String, modifier: Modifier = Modifier, language: String = "", lines: Int = Int.MAX_VALUE) {
    val clipboard = LocalClipboardManager.current
    val view = LocalView.current

    var copied by remember { mutableStateOf(false) }
    var whole by rememberSaveable { mutableStateOf(false) }

    LaunchedEffect(copied) {
        if (copied) {
            delay(1400)

            copied = false
        }
    }

    val count = remember(code) { code.count { it == '\n' } + 1 }
    val folded = !whole && count > lines
    val shown = if (folded) remember(code, lines) { code.lineSequence().take(lines).joinToString("\n") } else code

    Column(
        modifier
            .fillMaxWidth()
            .clip(Shapes.small)
            .background(colors.sunken)
            .border(1.dp, colors.border.copy(alpha = 0.6f), Shapes.small),
    ) {
        Line(
            Modifier
                .fillMaxWidth()
                .height(32.dp)
                .background(colors.raised.copy(alpha = 0.45f))
                .padding(start = Space.m),
            gap = Space.xs,
        ) {
            Text(language.ifEmpty { "code" }, style = Type.monoSmall, color = colors.textMuted, modifier = Modifier.weight(1f))

            Line(
                Modifier
                    .heightIn(min = 32.dp)
                    .pressable {
                        clipboard.setText(AnnotatedString(code))
                        Haptics.confirm(view)

                        copied = true
                    }
                    .padding(horizontal = Space.m),
                gap = Space.xs,
            ) {
                Icon(
                    if (copied) Icons.Rounded.Check else Icons.Rounded.ContentCopy,
                    null,
                    tint = if (copied) colors.good else colors.textSecondary,
                    modifier = Modifier.size(14.dp),
                )

                Text(if (copied) "Copied" else "Copy", style = Type.caption, color = colors.textSecondary)
            }
        }

        Text(
            shown,
            style = Type.mono,
            color = colors.text,
            softWrap = false,
            modifier = Modifier
                .horizontalScroll(rememberScrollState())
                .padding(Space.m),
        )

        if (folded) {
            Text(
                "Show ${count - lines} more lines",
                style = Type.caption,
                color = colors.accent,
                modifier = Modifier
                    .fillMaxWidth()
                    .pressable { whole = true }
                    .padding(horizontal = Space.m, vertical = Space.s),
            )
        }
    }
}

/** The widest a column of a table grows before its words wrap. */
private val WidestColumn = 220.dp

/**
 * A table. Every cell's natural width is measured before anything is
 * placed, so each column is as wide as its widest cell up to a limit, each
 * row as tall as its tallest cell, and the table scrolls sideways when
 * the columns do not fit.
 */
@Composable
private fun TableView(table: Block.Table, modifier: Modifier) {
    val columns = maxOf(table.header.size, table.rows.maxOfOrNull { it.size } ?: 0)
    val rows = listOf(table.header) + table.rows
    val line = colors.border.copy(alpha = 0.6f)

    Box(
        modifier
            .fillMaxWidth()
            .clip(Shapes.small)
            .border(1.dp, colors.border, Shapes.small),
    ) {
        Layout(
            content = {
                rows.forEachIndexed { row, cells ->
                    val ground = when {
                        row == 0 -> colors.raised.copy(alpha = 0.6f)
                        row % 2 == 0 -> colors.sunken.copy(alpha = 0.5f)
                        else -> Color.Transparent
                    }

                    for (column in 0 until columns) {
                        val align = when (table.aligns.getOrNull(column)) {
                            Align.CENTER -> TextAlign.Center
                            Align.END -> TextAlign.End
                            else -> TextAlign.Start
                        }

                        Text(
                            cells.getOrNull(column)?.styled() ?: AnnotatedString(""),
                            style = if (row == 0) Type.footnote.copy(fontWeight = FontWeight.SemiBold) else Type.footnote,
                            color = colors.text,
                            textAlign = align,
                            modifier = Modifier
                                .background(ground)
                                .drawBehind {
                                    if (row < rows.lastIndex) drawLine(line, Offset(0f, size.height), Offset(size.width, size.height), 1.dp.toPx())
                                }
                                .padding(horizontal = Space.m, vertical = Space.s),
                        )
                    }
                }
            },
            modifier = Modifier.horizontalScroll(rememberScrollState()),
        ) { measurables, _ ->
            val most = WidestColumn.roundToPx()

            val widths = IntArray(columns) { column ->
                rows.indices.maxOf { row -> measurables[row * columns + column].maxIntrinsicWidth(Constraints.Infinity) }
                    .coerceAtMost(most)
            }

            val heights = IntArray(rows.size) { row ->
                (0 until columns).maxOf { column -> measurables[row * columns + column].minIntrinsicHeight(widths[column]) }
            }

            val placeables = measurables.mapIndexed { index, measurable ->
                measurable.measure(Constraints.fixed(widths[index % columns], heights[index / columns]))
            }

            layout(widths.sum(), heights.sum()) {
                var y = 0

                for (row in rows.indices) {
                    var x = 0

                    for (column in 0 until columns) {
                        placeables[row * columns + column].place(x, y)

                        x += widths[column]
                    }

                    y += heights[row]
                }
            }
        }
    }
}
