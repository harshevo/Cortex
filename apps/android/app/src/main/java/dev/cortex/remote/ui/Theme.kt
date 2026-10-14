package dev.cortex.remote.ui

import android.app.Activity
import android.graphics.drawable.ColorDrawable
import android.os.Build
import android.view.HapticFeedbackConstants
import android.view.View
import androidx.compose.animation.core.Spring
import androidx.compose.animation.core.spring
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.layout.wrapContentWidth
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.Typography
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.Immutable
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.Density
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.core.view.WindowCompat

/**
 * One theme's colours. Every colour a screen draws comes from here, so
 * that a theme changes all of the app and nothing is left behind.
 */
@Immutable
data class Palette(
    val name: String,
    /** Light text on dark grounds, rather than the other way round. */
    val dark: Boolean,
    val background: Color,
    val sunken: Color,
    val surface: Color,
    val raised: Color,
    val border: Color,
    val text: Color,
    val textSecondary: Color,
    val textMuted: Color,
    /** Code set inside a sentence. */
    val code: Color,
    /** What the agent does, and what the person can do next. */
    val accent: Color,
    val good: Color,
    val bad: Color,
    val warning: Color,
    /** A model response. */
    val model: Color,
    /** Words and icons set on the accent. */
    val onAccent: Color,
)

private val LocalPalette = staticCompositionLocalOf { Dark }

/** The colours in use. */
val colors: Palette
    @Composable get() = LocalPalette.current

object Type {
    /** Titles are set in a serif, as the desktop app's are. */
    fun title(size: Int) = TextStyle(
        fontFamily = FontFamily.Serif,
        fontWeight = FontWeight.SemiBold,
        fontSize = size.sp,
        lineHeight = (size * 1.2).sp,
    )

    val body = TextStyle(fontSize = 16.sp, lineHeight = 24.sp)
    val bodyStrong = body.copy(fontWeight = FontWeight.Medium)
    val callout = TextStyle(fontSize = 15.sp, lineHeight = 21.sp)
    val subhead = TextStyle(fontSize = 14.sp, lineHeight = 19.sp, fontWeight = FontWeight.Medium)
    val footnote = TextStyle(fontSize = 13.sp, lineHeight = 18.sp)
    val caption = TextStyle(fontSize = 12.sp, lineHeight = 16.sp)
    val mono = TextStyle(fontFamily = FontFamily.Monospace, fontSize = 13.sp, lineHeight = 19.sp)
    val monoSmall = TextStyle(fontFamily = FontFamily.Monospace, fontSize = 11.sp, lineHeight = 15.sp)

    val heading = TextStyle(
        fontSize = 12.sp,
        fontWeight = FontWeight.SemiBold,
        letterSpacing = 0.8.sp,
    )
}

/** The gaps between things: every one in the app is one of these. */
object Space {
    val xxs = 2.dp
    val xs = 4.dp
    val s = 8.dp
    val m = 12.dp
    val l = 16.dp
    val xl = 24.dp
    val xxl = 32.dp
}

/** The corners of things. */
object Shapes {
    val card = RoundedCornerShape(16.dp)
    val field = RoundedCornerShape(12.dp)
    val small = RoundedCornerShape(10.dp)
    val bubble = RoundedCornerShape(20.dp)
    val pill = CircleShape
}

val Gutter = Space.l
val Radius = 16.dp
val SmallRadius = 10.dp

/** The widest a column of reading is allowed to grow, on a tablet or a phone on its side. */
val Measure = 680.dp

/** The height of every screen's top bar. */
val BarHeight = 52.dp

/** Keeps what it is put on to a readable width, in the middle of the screen. */
fun Modifier.measure(max: Dp = Measure): Modifier =
    fillMaxWidth().wrapContentWidth(Alignment.CenterHorizontally).widthIn(max = max)

/** How things move: quick, and settling without a wobble. */
fun <T> settle() = spring<T>(dampingRatio = 0.82f, stiffness = Spring.StiffnessMediumLow)

/** The theme a choice stands for: a theme's name, or [FOLLOW_SYSTEM]. */
@Composable
fun paletteFor(choice: String): Palette = when (choice) {
    FOLLOW_SYSTEM -> if (isSystemInDarkTheme()) Dark else Light
    else -> themeNamed(choice)
}

/**
 * Puts a theme and a text size in use for everything inside, and makes
 * the status and navigation bars' icons readable on it.
 */
@Composable
fun CortexTheme(choice: String = FOLLOW_SYSTEM, textScale: Float = 1f, content: @Composable () -> Unit) {
    val palette = paletteFor(choice)

    SystemBars(palette)

    val scheme = if (palette.dark) {
        darkColorScheme(
            primary = palette.accent,
            onPrimary = palette.onAccent,
            background = palette.background,
            surface = palette.surface,
            surfaceVariant = palette.raised,
            surfaceContainer = palette.surface,
            surfaceContainerLow = palette.background,
            surfaceContainerHigh = palette.raised,
            surfaceContainerHighest = palette.raised,
            onBackground = palette.text,
            onSurface = palette.text,
            onSurfaceVariant = palette.textSecondary,
            outline = palette.border,
            outlineVariant = palette.border,
            error = palette.bad,
            secondaryContainer = palette.accent,
            onSecondaryContainer = palette.onAccent,
            scrim = palette.sunken,
        )
    } else {
        lightColorScheme(
            primary = palette.accent,
            onPrimary = palette.onAccent,
            background = palette.background,
            surface = palette.surface,
            surfaceVariant = palette.raised,
            surfaceContainer = palette.surface,
            surfaceContainerLow = palette.background,
            surfaceContainerHigh = palette.raised,
            surfaceContainerHighest = palette.raised,
            onBackground = palette.text,
            onSurface = palette.text,
            onSurfaceVariant = palette.textSecondary,
            outline = palette.border,
            outlineVariant = palette.border,
            error = palette.bad,
            secondaryContainer = palette.accent,
            onSecondaryContainer = palette.onAccent,
            scrim = palette.text,
        )
    }

    val density = LocalDensity.current
    val scaled = Density(density.density, density.fontScale * textScale)

    CompositionLocalProvider(LocalPalette provides palette, LocalDensity provides scaled) {
        // Text that names no style of its own is set as the body is.
        MaterialTheme(colorScheme = scheme, typography = Typography(bodyLarge = Type.body), content = content)
    }
}

/**
 * The system's bars are see-through and the app draws under them, so
 * only their icons need telling: dark ones on a light theme.
 */
@Composable
private fun SystemBars(palette: Palette) {
    val view = LocalView.current

    DisposableEffect(view, palette) {
        val window = (view.context as? Activity)?.window

        if (window != null) {
            val bars = WindowCompat.getInsetsController(window, view)

            bars.isAppearanceLightStatusBars = !palette.dark
            bars.isAppearanceLightNavigationBars = !palette.dark

            // What shows for a moment behind the keyboard as it moves.
            window.setBackgroundDrawable(ColorDrawable(palette.background.toArgb()))

            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                window.isNavigationBarContrastEnforced = false
            }
        }

        onDispose {}
    }
}

/** A card: the ground most things sit on. */
@Composable
fun Modifier.card(padding: Dp = 14.dp): Modifier = this
    .clip(Shapes.card)
    .background(colors.surface)
    .border(BorderStroke(1.dp, colors.border.copy(alpha = 0.7f)), Shapes.card)
    .padding(padding)

/** A small heading above a group. */
@Composable
fun SectionHeading(text: String, modifier: Modifier = Modifier) {
    Text(text.uppercase(), style = Type.heading, color = colors.textMuted, modifier = modifier)
}

/** A touch the hand can feel. */
object Haptics {
    fun tap(view: View) {
        view.performHapticFeedback(HapticFeedbackConstants.KEYBOARD_TAP)
    }

    fun confirm(view: View) {
        view.performHapticFeedback(HapticFeedbackConstants.CONTEXT_CLICK)
    }

    fun warn(view: View) {
        view.performHapticFeedback(HapticFeedbackConstants.LONG_PRESS)
    }
}
