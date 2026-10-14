package dev.cortex.remote.ui

import android.util.LruCache
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.Box
import androidx.compose.runtime.Composable
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.drawscope.ContentDrawScope
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.Measurable
import androidx.compose.ui.layout.MeasureResult
import androidx.compose.ui.layout.MeasureScope
import androidx.compose.ui.node.DrawModifierNode
import androidx.compose.ui.node.LayoutModifierNode
import androidx.compose.ui.node.ModifierNodeElement
import androidx.compose.ui.node.SemanticsModifierNode
import androidx.compose.ui.node.invalidateDraw
import androidx.compose.ui.node.invalidateMeasurement
import androidx.compose.ui.node.invalidateSemantics
import androidx.compose.ui.platform.LocalUriHandler
import androidx.compose.ui.semantics.SemanticsPropertyReceiver
import androidx.compose.ui.semantics.text
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.TextLayoutResult
import androidx.compose.ui.text.TextMeasurer
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.drawText
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.rememberTextMeasurer
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.unit.Constraints
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.em
import kotlin.math.ceil

/**
 * Text that is measured before it is drawn.
 *
 * A conversation is mostly text, and the usual way lays a piece of it
 * out afresh every time it comes on screen. Here one measurer is shared
 * by the whole conversation and keeps what it has laid out, so a block
 * that scrolls back into view is drawn from what is already known; its
 * height comes from the measured layout and not from composing anything.
 * The same measuring finds the narrowest width a short message can be
 * set in without taking more lines, which is what makes its bubble fit.
 */
@Stable
class ProseLayouts(val measurer: TextMeasurer) {
    /** The width found for a message at a given room, so it is searched for once. */
    internal val fitted = LruCache<FitKey, Int>(256)
}

internal data class FitKey(val text: String, val room: Int, val style: TextStyle)

/** The layouts of the conversation in view. Absent outside one. */
val LocalProse = staticCompositionLocalOf<ProseLayouts?> { null }

/** How many laid-out blocks are kept: a few screens' worth. */
private const val KEPT = 96

@Composable
fun rememberProseLayouts(): ProseLayouts {
    val measurer = rememberTextMeasurer(cacheSize = KEPT)

    return remember(measurer) { ProseLayouts(measurer) }
}

private const val LINK = "link"

/** The words as they are drawn in this theme: marks become styles, once. */
@Composable
fun Inline.styled(): AnnotatedString {
    val palette = colors

    return remember(this, palette) { styled(palette) }
}

fun Inline.styled(palette: Palette): AnnotatedString = buildAnnotatedString {
    append(text)

    for (span in spans) {
        val style = when (span.mark) {
            Mark.BOLD -> SpanStyle(fontWeight = FontWeight.SemiBold)
            Mark.ITALIC -> SpanStyle(fontStyle = FontStyle.Italic)
            Mark.STRIKE -> SpanStyle(textDecoration = TextDecoration.LineThrough)

            // A little smaller than the words around it, whatever their size.
            Mark.CODE -> SpanStyle(
                fontFamily = FontFamily.Monospace,
                color = palette.code,
                fontSize = 0.9.em,
                background = palette.raised.copy(alpha = 0.55f),
            )

            Mark.LINK -> SpanStyle(color = palette.accent, textDecoration = TextDecoration.Underline)
        }

        addStyle(style, span.start, span.end)

        if (span.mark == Mark.LINK && span.target != null) {
            addStringAnnotation(LINK, span.target, span.start, span.end)
        }
    }
}

/**
 * A piece of text, measured ahead and drawn from the measurement.
 *
 * [fitted] sets it in the narrowest width that takes no more lines.
 * [growing] is for text still being written: its layouts are not kept,
 * and [caret] draws the mark of writing after its last character, in
 * the line and not under it, so nothing moves when a line fills.
 */
@Composable
fun ProseText(
    text: AnnotatedString,
    style: TextStyle,
    modifier: Modifier = Modifier,
    fitted: Boolean = false,
    growing: Boolean = false,
    caret: Boolean = false,
    onLongPress: (() -> Unit)? = null,
) {
    val layouts = LocalProse.current ?: rememberProseLayouts()
    val laid = remember { Laid() }
    val links = LocalUriHandler.current

    val blink = if (caret) {
        val alpha by rememberInfiniteTransition(label = "caret").animateFloat(
            initialValue = 1f,
            targetValue = 0.2f,
            animationSpec = infiniteRepeatable(tween(550), RepeatMode.Reverse),
            label = "caret",
        )

        remember { { alpha } }
    } else {
        null
    }

    val pressed by rememberUpdatedState(onLongPress)
    val hasLinks = remember(text) { text.getStringAnnotations(LINK, 0, text.length).isNotEmpty() }

    val touch = if (onLongPress != null || hasLinks) {
        Modifier.pointerInput(text) {
            detectTapGestures(
                onLongPress = { pressed?.invoke() },
                onTap = { point ->
                    val layout = laid.result ?: return@detectTapGestures
                    val at = layout.getOffsetForPosition(point)

                    text.getStringAnnotations(LINK, at, at).firstOrNull()?.let { link ->
                        runCatching { links.openUri(link.item) }
                    }
                },
            )
        }
    } else {
        Modifier
    }

    Box(
        modifier
            .then(ProseElement(text, style, layouts, fitted, growing, colors.accent, blink, laid))
            .then(touch),
    )
}

/** Where the node leaves its layout for the taps to read. */
internal class Laid {
    var result: TextLayoutResult? = null
}

private data class ProseElement(
    val text: AnnotatedString,
    val style: TextStyle,
    val layouts: ProseLayouts,
    val fitted: Boolean,
    val growing: Boolean,
    val caretColor: Color,
    val caret: (() -> Float)?,
    val laid: Laid,
) : ModifierNodeElement<ProseNode>() {
    override fun create() = ProseNode(text, style, layouts, fitted, growing, caretColor, caret, laid)

    override fun update(node: ProseNode) {
        val again = node.text != text || node.style != style || node.layouts !== layouts ||
            node.fitted != fitted || node.growing != growing

        node.text = text
        node.style = style
        node.layouts = layouts
        node.fitted = fitted
        node.growing = growing
        node.caretColor = caretColor
        node.caret = caret
        node.laid = laid

        if (again) {
            node.invalidateMeasurement()
            node.invalidateSemantics()
        }

        node.invalidateDraw()
    }
}

private class ProseNode(
    var text: AnnotatedString,
    var style: TextStyle,
    var layouts: ProseLayouts,
    var fitted: Boolean,
    var growing: Boolean,
    var caretColor: Color,
    var caret: (() -> Float)?,
    var laid: Laid,
) : Modifier.Node(), LayoutModifierNode, DrawModifierNode, SemanticsModifierNode {
    private var layout: TextLayoutResult? = null

    override fun MeasureScope.measure(measurable: Measurable, constraints: Constraints): MeasureResult {
        val room = constraints.maxWidth
        val measurer = layouts.measurer

        var result = measurer.measure(text, style, constraints = Constraints(maxWidth = room), skipCache = growing)
        var width = result.size.width

        if (fitted && result.lineCount > 1) {
            val key = FitKey(text.text, room, style)

            val narrow = layouts.fitted.get(key) ?: narrowest(measurer, result.lineCount, room).also {
                layouts.fitted.put(key, it)
            }

            result = measurer.measure(text, style, constraints = Constraints(maxWidth = narrow))

            // The layout is as wide as it was allowed to be: the box is as wide as its widest line.
            width = (0 until result.lineCount).maxOf { ceil(result.getLineRight(it) - result.getLineLeft(it)).toInt() }
        }

        layout = result
        laid.result = result

        // The size may be what it was while the words are not.
        invalidateDraw()

        val size = Constraints.fixed(
            width.coerceIn(constraints.minWidth, constraints.maxWidth),
            result.size.height.coerceIn(constraints.minHeight, constraints.maxHeight),
        )

        val placeable = measurable.measure(size)

        return layout(size.maxWidth, size.maxHeight) { placeable.place(0, 0) }
    }

    /** The least width at which the text still takes [lines] lines. */
    private fun narrowest(measurer: TextMeasurer, lines: Int, room: Int): Int {
        var low = room / (lines + 1)
        var high = room

        while (high - low > 3) {
            val middle = (low + high) / 2
            val probe = measurer.measure(text, style, constraints = Constraints(maxWidth = middle), skipCache = true)

            if (probe.lineCount <= lines) high = middle else low = middle
        }

        return high
    }

    override fun ContentDrawScope.draw() {
        val result = layout

        if (result != null) {
            drawText(result)

            caret?.let { alpha ->
                val end = result.getCursorRect(text.length)
                val wide = 7.dp.toPx()
                val tall = end.height * 0.72f

                drawRoundRect(
                    color = caretColor.copy(alpha = alpha()),
                    topLeft = Offset(end.left + 2.dp.toPx(), end.top + (end.height - tall) / 2),
                    size = Size(wide, tall),
                    cornerRadius = CornerRadius(2.dp.toPx()),
                )
            }
        }

        drawContent()
    }

    override fun SemanticsPropertyReceiver.applySemantics() {
        text = this@ProseNode.text
    }
}
