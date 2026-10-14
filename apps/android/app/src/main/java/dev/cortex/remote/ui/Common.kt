package dev.cortex.remote.ui

import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsPressedAsState
import androidx.activity.compose.PredictiveBackHandler
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.rounded.ArrowBack
import androidx.compose.material3.Icon
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.graphics.vector.ImageVector
import kotlinx.coroutines.CancellationException
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.composed
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import uniffi.cortex_mobile.Connection

/** Shrinks a little under the finger, and is felt. */
fun Modifier.pressable(enabled: Boolean = true, onClick: () -> Unit): Modifier = composed {
    val source = remember { MutableInteractionSource() }
    val pressed by source.collectIsPressedAsState()
    val scale by animateFloatAsState(if (pressed) 0.97f else 1f, label = "press")
    val view = LocalView.current

    graphicsLayer {
        scaleX = scale
        scaleY = scale
        alpha = if (enabled) 1f else 0.45f
    }.clickable(interactionSource = source, indication = null, enabled = enabled) {
        Haptics.tap(view)
        onClick()
    }
}

/** The app's main button. */
@Composable
fun PrimaryButton(text: String, modifier: Modifier = Modifier, enabled: Boolean = true, onClick: () -> Unit) {
    Box(
        modifier
            .fillMaxWidth()
            .heightIn(min = 52.dp)
            .pressable(enabled, onClick)
            .clip(RoundedCornerShape(Radius))
            .background(colors.accent),
        contentAlignment = Alignment.Center,
    ) {
        Text(text, style = Type.bodyStrong.copy(fontWeight = FontWeight.SemiBold), color = colors.onAccent)
    }
}

@Composable
fun SecondaryButton(text: String, modifier: Modifier = Modifier, enabled: Boolean = true, onClick: () -> Unit) {
    val shape = RoundedCornerShape(Radius)

    Box(
        modifier
            .fillMaxWidth()
            .heightIn(min = 52.dp)
            .pressable(enabled, onClick)
            .clip(shape)
            .background(colors.surface)
            .border(1.dp, colors.border, shape),
        contentAlignment = Alignment.Center,
    ) {
        Text(text, style = Type.bodyStrong, color = colors.text)
    }
}

/** A small light that can breathe. */
@Composable
fun StatusLight(color: Color, pulsing: Boolean = false, size: Dp = 8.dp) {
    val breath = if (pulsing) {
        val value by rememberInfiniteTransition(label = "light").animateFloat(
            initialValue = 0f,
            targetValue = 1f,
            animationSpec = infiniteRepeatable(tween(1300, easing = LinearEasing), RepeatMode.Restart),
            label = "light",
        )

        value
    } else {
        null
    }

    Canvas(Modifier.size(size)) {
        val radius = this.size.minDimension / 2

        if (breath != null) {
            drawCircle(
                color.copy(alpha = 0.5f * (1 - breath)),
                radius = radius * (1 + 1.4f * breath),
                style = Stroke(width = 2.dp.toPx()),
            )
        }

        drawCircle(color, radius = radius)
    }
}

/** An arc that goes round while something works. */
@Composable
fun RunningRing(color: Color, modifier: Modifier = Modifier) {
    val turn by rememberInfiniteTransition(label = "ring").animateFloat(
        initialValue = 0f,
        targetValue = 360f,
        animationSpec = infiniteRepeatable(tween(1100, easing = LinearEasing), RepeatMode.Restart),
        label = "ring",
    )

    Canvas(modifier) {
        val stroke = 2.5.dp.toPx()

        drawArc(
            color = color,
            startAngle = turn,
            sweepAngle = 100f,
            useCenter = false,
            topLeft = Offset(stroke / 2, stroke / 2),
            size = androidx.compose.ui.geometry.Size(size.width - stroke, size.height - stroke),
            style = Stroke(width = stroke, cap = StrokeCap.Round),
        )
    }
}

/** The connection in a word, with a light. */
@Composable
fun ConnectionPill(connection: Connection) {
    val (text, color, pulsing) = when (connection) {
        is Connection.Idle -> Triple("Not connected", colors.textMuted, false)
        is Connection.Connecting -> Triple("Connecting to ${connection.host}…", colors.warning, true)
        is Connection.Connected -> Triple(connection.host, colors.good, false)
        is Connection.Reconnecting -> Triple("Reaching ${connection.host}…", colors.warning, true)
        is Connection.Failed -> Triple("Could not connect", colors.bad, false)
    }

    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        StatusLight(color, pulsing)

        Text(text, style = Type.footnote, color = colors.textSecondary, maxLines = 1, overflow = TextOverflow.Ellipsis)
    }
}

/** Says how connecting is going, when there is something to say. */
@Composable
fun ConnectionNote(connection: Connection, modifier: Modifier = Modifier) {
    when (connection) {
        is Connection.Connecting -> Waiting("Connecting to ${connection.host}…", null, modifier)
        is Connection.Reconnecting -> Waiting("Trying to reach ${connection.host}…", connection.reason, modifier)

        is Connection.Failed -> Text(
            connection.reason,
            style = Type.footnote,
            color = colors.bad,
            textAlign = TextAlign.Center,
            modifier = modifier,
        )

        else -> {}
    }
}

@Composable
private fun Waiting(text: String, reason: String?, modifier: Modifier) {
    Column(modifier, horizontalAlignment = Alignment.CenterHorizontally) {
        Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            CircularProgressIndicator(Modifier.size(14.dp), color = colors.textSecondary, strokeWidth = 2.dp)

            Text(text, style = Type.footnote, color = colors.textSecondary)
        }

        if (reason != null) {
            Text(
                reason,
                style = Type.caption,
                color = colors.textMuted,
                textAlign = TextAlign.Center,
                modifier = Modifier.padding(top = 4.dp),
            )
        }
    }
}

/** A count in few characters: 6.3k, 1.2M. */
fun compact(number: ULong): String = when {
    number >= 1_000_000uL -> "%.1fM".format(number.toDouble() / 1_000_000)
    number >= 1_000uL -> "%.1fk".format(number.toDouble() / 1_000)
    else -> number.toString()
}

/** A row that lays its children out with a gap, centred on the line. */
@Composable
fun Line(modifier: Modifier = Modifier, gap: Dp = 8.dp, content: @Composable RowScope.() -> Unit) {
    Row(modifier, verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(gap), content = content)
}

/** A round button with an icon, for top bars. */
@Composable
fun BarButton(icon: ImageVector, label: String, enabled: Boolean = true, tint: Color = colors.text, onClick: () -> Unit) {
    Box(Modifier.size(44.dp).pressable(enabled, onClick), contentAlignment = Alignment.Center) {
        Icon(icon, label, tint = tint, modifier = Modifier.size(22.dp))
    }
}

/**
 * The bar at the top of every screen but the first: back, a title with
 * a line under it, and the screen's own buttons. Sits under the status
 * bar, on the screen's own ground.
 */
@Composable
fun TopBar(
    title: String,
    subtitle: String? = null,
    back: (() -> Unit)? = null,
    actions: @Composable RowScope.() -> Unit = {},
) {
    Row(
        Modifier
            .fillMaxWidth()
            .statusBarsPadding()
            .height(BarHeight)
            .padding(horizontal = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        if (back != null) {
            BarButton(Icons.AutoMirrored.Rounded.ArrowBack, "Back", onClick = back)
        }

        Column(Modifier.weight(1f).padding(horizontal = if (back != null) Space.xs else Space.s)) {
            Text(
                title,
                style = Type.bodyStrong.copy(fontWeight = FontWeight.SemiBold),
                color = colors.text,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )

            if (!subtitle.isNullOrEmpty()) {
                Text(subtitle, style = Type.caption, color = colors.textMuted, maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
        }

        actions()
    }
}

/** What a screen shows when it has nothing to show, or something went wrong. */
@Composable
fun EmptyState(icon: ImageVector, title: String, text: String, modifier: Modifier = Modifier, tint: Color = colors.textMuted) {
    Column(
        modifier.fillMaxWidth().padding(horizontal = Space.xl, vertical = Space.xxl),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(Space.s),
    ) {
        Icon(icon, null, tint = tint, modifier = Modifier.size(40.dp))
        Text(title, style = Type.title(20), color = colors.text, textAlign = TextAlign.Center)
        Text(text, style = Type.callout, color = colors.textSecondary, textAlign = TextAlign.Center)
    }
}

/**
 * Lets a screen follow the back gesture: it shrinks a little and moves
 * with the finger while the gesture is under way, and [back] runs when it
 * is let go. Let go of early, it settles back.
 */
@Composable
fun BackSwipe(enabled: Boolean = true, back: () -> Unit, content: @Composable (Modifier) -> Unit) {
    var progress by remember { mutableFloatStateOf(0f) }

    PredictiveBackHandler(enabled) { events ->
        try {
            events.collect { progress = it.progress }

            back()
        } catch (_: CancellationException) {
            // The gesture was let go of: nothing happens.
        } finally {
            progress = 0f
        }
    }

    content(
        Modifier.graphicsLayer {
            val shrink = 1f - 0.06f * progress

            scaleX = shrink
            scaleY = shrink
            translationX = 28.dp.toPx() * progress
            shape = RoundedCornerShape(28.dp * progress)
            clip = progress > 0f
        },
    )
}
