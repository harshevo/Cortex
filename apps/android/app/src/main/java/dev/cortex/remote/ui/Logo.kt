package dev.cortex.remote.ui

import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.lerp
import androidx.compose.ui.graphics.luminance
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp

/**
 * The Cortex mark on its ground: an open ring with a signal leaving it.
 * Drawn from the same numbers as the iOS mark and the launcher icon, in
 * the accent of the theme in use: a lighter and a darker shade of it for
 * the ground, and the ink that reads on the accent for the mark.
 *
 * With [alive], the signal travels out again and again.
 */
@Composable
fun LogoBadge(size: Dp = 96.dp, alive: Boolean = false, modifier: Modifier = Modifier) {
    val signal = if (alive) {
        val travel by rememberInfiniteTransition(label = "signal").animateFloat(
            initialValue = 0f,
            targetValue = 1f,
            animationSpec = infiniteRepeatable(tween(1500, easing = LinearEasing), RepeatMode.Restart),
            label = "signal",
        )

        // Slow at both ends, as the iOS mark moves.
        val eased = travel * travel * (3 - 2 * travel)

        eased
    } else {
        1f
    }

    val shape = RoundedCornerShape(size * 0.225f)

    val accent = colors.accent
    val light = lerp(accent, colors.onAccent, 0.18f)
    val dark = lerp(accent, colors.sunken, 0.16f)
    // The mark is light where a light one stands out from the ground enough for a shape,
    // as on the launcher icon; on a pale accent it takes the accent's own ink.
    val mark = listOf(colors.background, colors.text, colors.surface)
        .filter { contrast(it, accent) >= 3f }
        .maxByOrNull { it.luminance() }
        ?.takeIf { it.luminance() > colors.onAccent.luminance() }
        ?: colors.onAccent

    Box(
        modifier
            .size(size)
            .shadow(size * 0.16f, shape, ambientColor = dark, spotColor = dark)
            .clip(shape)
            .background(Brush.linearGradient(listOf(light, dark))),
    ) {
        Canvas(Modifier.fillMaxSize()) {
            val side = this.size.minDimension
            val centre = Offset(this.size.width / 2, this.size.height / 2)
            val radius = side * 0.27f

            // The ring, open to the right.
            drawArc(
                color = mark,
                startAngle = 42f,
                sweepAngle = 276f,
                useCenter = false,
                topLeft = Offset(centre.x - radius, centre.y - radius),
                size = Size(radius * 2, radius * 2),
                style = Stroke(width = side * 0.085f, cap = StrokeCap.Round),
            )

            // The path the signal takes, and the signal on it.
            drawLine(
                color = mark.copy(alpha = 0.55f),
                start = centre,
                end = Offset(centre.x + radius, centre.y),
                strokeWidth = side * 0.034f,
                cap = StrokeCap.Round,
            )

            drawCircle(mark, radius = side * 0.068f, center = centre)
            drawCircle(mark, radius = side * 0.05f, center = Offset(centre.x + radius * signal, centre.y))
        }
    }
}

/** The WCAG contrast of two colours. */
private fun contrast(one: Color, other: Color): Float {
    val a = one.luminance()
    val b = other.luminance()

    return (maxOf(a, b) + 0.05f) / (minOf(a, b) + 0.05f)
}
