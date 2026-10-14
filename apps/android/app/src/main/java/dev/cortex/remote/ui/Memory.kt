package dev.cortex.remote.ui

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.slideOutVertically
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.Hub
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.withFrameNanos
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import dev.cortex.remote.AppModel
import uniffi.cortex_mobile.MemoryKindView
import uniffi.cortex_mobile.MemoryNodeView
import uniffi.cortex_mobile.MemoryView
import kotlin.math.cos
import kotlin.math.hypot
import kotlin.math.max
import kotlin.math.min
import kotlin.math.sin
import kotlin.math.sqrt

private val Nothing = MemoryView(emptyList(), emptyList())

@Composable
private fun colour(kind: MemoryKindView): Color = when (kind) {
    MemoryKindView.WORKING -> colors.accent
    MemoryKindView.EPISODE -> colors.model
    MemoryKindView.FACT -> colors.good
}

/**
 * What the agent remembers, drawn as dots that settle near what they
 * are tied to. Touch one to read it.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun MemorySheet(model: AppModel, chat: String, close: () -> Unit) {
    val memory = model.memory[chat] ?: Nothing

    // Fetched when the sheet opens, off the main thread, and kept fresh as it changes.
    LaunchedEffect(chat) { model.refreshMemory(chat) }

    ModalBottomSheet(
        onDismissRequest = close,
        sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
        containerColor = colors.sunken,
    ) {
        Column(Modifier.fillMaxHeight(0.92f).navigationBarsPadding()) {
            Text(
                "Memory",
                style = Type.title(24),
                color = colors.text,
                modifier = Modifier.padding(horizontal = Gutter),
            )

            if (memory.nodes.isEmpty()) {
                Column(
                    Modifier.fillMaxSize().padding(32.dp),
                    horizontalAlignment = Alignment.CenterHorizontally,
                    verticalArrangement = Arrangement.spacedBy(10.dp, Alignment.CenterVertically),
                ) {
                    Icon(Icons.Rounded.Hub, null, tint = colors.textMuted, modifier = Modifier.size(44.dp))

                    Text("Nothing remembered yet", style = Type.title(20), color = colors.text)

                    Text(
                        "Tasks done in this project appear here, joined to the ones they resemble.",
                        style = Type.callout,
                        color = colors.textSecondary,
                        textAlign = TextAlign.Center,
                    )
                }
            } else {
                Graph(memory, Modifier.weight(1f))
            }
        }
    }
}

@Composable
private fun Graph(memory: MemoryView, modifier: Modifier) {
    val view = LocalView.current
    val density = LocalDensity.current

    val layout = remember { Layout() }

    var selected by remember { mutableStateOf<Int?>(null) }

    // Counts frames, so that the dots are drawn again as they move.
    var time by remember { mutableFloatStateOf(0f) }

    LaunchedEffect(Unit) {
        val start = withFrameNanos { it }

        while (true) {
            withFrameNanos { time = (it - start) / 1_000_000_000f }
        }
    }

    val palette = colors
    val reach = with(density) { 30.dp.toPx() }

    // What the chosen dot is tied to: worked out when the choice changes, not every frame.
    val tied = remember(memory, selected) {
        val chosen = selected

        if (chosen == null) {
            emptySet()
        } else {
            memory.links.mapNotNull { link ->
                when (chosen) {
                    link.from.toInt() -> link.to.toInt()
                    link.to.toInt() -> link.from.toInt()
                    else -> null
                }
            }.toSet()
        }
    }

    Box(modifier.fillMaxWidth()) {
        Canvas(
            Modifier
                .fillMaxSize()
                .pointerInput(memory) {
                    detectTapGestures { point ->
                        selected = layout.nearest(point, reach)

                        if (selected != null) Haptics.tap(view)
                    }
                },
        ) {
            layout.step(memory, size, bottomMargin = 150.dp.toPx(), margin = 24.dp.toPx())

            if (layout.count != memory.nodes.size) return@Canvas

            val chosen = selected

            for (link in memory.links) {
                val from = link.from.toInt()
                val to = link.to.toInt()

                if (from >= layout.count || to >= layout.count) continue

                val lit = chosen == from || chosen == to

                drawLine(
                    color = if (lit) {
                        palette.accent.copy(alpha = 0.9f)
                    } else {
                        palette.textMuted.copy(alpha = if (chosen == null) 0.28f else 0.1f)
                    },
                    start = layout.at(from),
                    end = layout.at(to),
                    strokeWidth = (if (lit) 1.6f else 0.8f).dp.toPx(),
                )
            }

            memory.nodes.forEachIndexed { index, node ->
                val color = when (node.kind) {
                    MemoryKindView.WORKING -> palette.accent
                    MemoryKindView.EPISODE -> palette.model
                    MemoryKindView.FACT -> palette.good
                }

                val isChosen = chosen == index
                val dimmed = chosen != null && !isChosen && index !in tied

                // Each dot breathes a little, out of step with the others.
                val breath = 1 + 0.12f * sin(time * 1.4f + index * 0.7f)
                val base = if (node.kind == MemoryKindView.FACT) 4.5f else 6f
                val radius = (base * (if (isChosen) 1.7f else breath)).dp.toPx()
                val centre = layout.at(index)

                drawCircle(color.copy(alpha = if (dimmed) 0.03f else 0.16f), radius * 1.9f, centre)
                drawCircle(color.copy(alpha = if (dimmed) 0.25f else 1f), radius, centre)
            }
        }

        Column(
            Modifier.align(Alignment.BottomCenter).padding(Gutter),
            verticalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            val node = selected?.let { memory.nodes.getOrNull(it) }

            AnimatedVisibility(
                node != null,
                enter = slideInVertically(settle()) { it / 2 } + fadeIn(),
                exit = slideOutVertically(settle()) { it / 2 } + fadeOut(),
            ) {
                node?.let { NodeCard(it) }
            }

            Line(Modifier.fillMaxWidth(), gap = 16.dp) {
                Legend(memory, MemoryKindView.EPISODE, "earlier tasks")
                Legend(memory, MemoryKindView.FACT, "facts and lessons")
            }
        }
    }
}

@Composable
private fun Legend(memory: MemoryView, kind: MemoryKindView, name: String) {
    Line(gap = 6.dp) {
        Box(Modifier.size(7.dp).clip(CircleShape).background(colour(kind)))

        Text("${memory.nodes.count { it.kind == kind }} $name", style = Type.caption, color = colors.textSecondary)
    }
}

@Composable
private fun NodeCard(node: MemoryNodeView) {
    val shape = RoundedCornerShape(Radius)

    Column(
        Modifier
            .fillMaxWidth()
            .clip(shape)
            .background(colors.surface)
            .border(1.dp, colors.border, shape)
            .padding(14.dp),
        verticalArrangement = Arrangement.spacedBy(6.dp),
    ) {
        Line {
            Box(Modifier.size(8.dp).clip(CircleShape).background(colour(node.kind)))

            Text(node.title, style = Type.subhead.copy(fontWeight = FontWeight.SemiBold), color = colors.text, maxLines = 2)
        }

        Text(
            node.text,
            style = Type.footnote,
            color = colors.textSecondary,
            modifier = Modifier.heightIn(max = 140.dp).verticalScroll(rememberScrollState()),
        )
    }
}

/**
 * Where the dots are. They push each other apart, are pulled together
 * by their ties and towards the middle, and soon come to rest.
 *
 * Moved every frame while they settle, so nothing is made anew per
 * frame: the positions and forces live in arrays that are kept.
 */
private class Layout {
    var count = 0
        private set

    private var x = FloatArray(0)
    private var y = FloatArray(0)
    private var fx = FloatArray(0)
    private var fy = FloatArray(0)
    private var vx = FloatArray(0)
    private var vy = FloatArray(0)

    private var size = Size.Zero

    /** How much the dots still move. Once it is small, nothing is done. */
    private var heat = 1f

    fun at(index: Int) = Offset(x[index], y[index])

    private fun seed(memory: MemoryView, size: Size) {
        this.size = size

        heat = 1f
        count = memory.nodes.size

        x = FloatArray(count)
        y = FloatArray(count)
        fx = FloatArray(count)
        fy = FloatArray(count)
        vx = FloatArray(count)
        vy = FloatArray(count)

        val spread = min(size.width, size.height) * 0.36f

        // A sunflower's arrangement: even, and the same every time.
        for (index in 0 until count) {
            val distance = spread * sqrt(index + 0.5f) / sqrt(max(count, 1).toFloat())
            val angle = index * 2.399963f

            x[index] = size.width / 2 + cos(angle) * distance
            y[index] = size.height / 2 + sin(angle) * distance
        }
    }

    fun step(memory: MemoryView, size: Size, bottomMargin: Float, margin: Float) {
        if (count != memory.nodes.size || size != this.size) {
            seed(memory, size)
        }

        if (heat <= 0.02f || count < 2) return

        fx.fill(0f)
        fy.fill(0f)

        val ideal = sqrt(size.width * size.height / count) * 0.62f

        for (a in 0 until count) {
            for (b in a + 1 until count) {
                val dx = x[a] - x[b]
                val dy = y[a] - y[b]
                val distance = max(hypot(dx, dy), 1f)
                val push = ideal * ideal / distance / distance * 0.9f

                fx[a] += dx / distance * push
                fy[a] += dy / distance * push
                fx[b] -= dx / distance * push
                fy[b] -= dy / distance * push
            }

            fx[a] += (size.width / 2 - x[a]) * 0.012f
            fy[a] += (size.height / 2 - y[a]) * 0.012f
        }

        for (link in memory.links) {
            val a = link.from.toInt()
            val b = link.to.toInt()

            if (a >= count || b >= count) continue

            val dx = x[b] - x[a]
            val dy = y[b] - y[a]
            val distance = max(hypot(dx, dy), 1f)
            val pull = (distance - ideal * 0.8f) * 0.02f

            fx[a] += dx / distance * pull
            fy[a] += dy / distance * pull
            fx[b] -= dx / distance * pull
            fy[b] -= dy / distance * pull
        }

        val bottom = max(margin, size.height - bottomMargin)

        for (index in 0 until count) {
            vx[index] = (vx[index] + fx[index]) * 0.82f
            vy[index] = (vy[index] + fy[index]) * 0.82f

            x[index] = (x[index] + vx[index] * heat).coerceIn(margin, size.width - margin)
            y[index] = (y[index] + vy[index] * heat).coerceIn(margin, bottom)
        }

        heat *= 0.992f
    }

    fun nearest(point: Offset, reach: Float): Int? {
        var best = -1
        var closest = Float.MAX_VALUE

        for (index in 0 until count) {
            val distance = hypot(x[index] - point.x, y[index] - point.y)

            if (distance < closest) {
                closest = distance
                best = index
            }
        }

        return best.takeIf { it >= 0 && closest <= reach }
    }
}
