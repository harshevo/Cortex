package dev.cortex.remote

import androidx.compose.ui.graphics.Color
import dev.cortex.remote.ui.DEFAULT_DARK
import dev.cortex.remote.ui.DEFAULT_LIGHT
import dev.cortex.remote.ui.Themes
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import kotlin.math.pow

/** Every theme can be read: WCAG contrast of its text on its grounds. */
class ThemeContrastTest {
    private fun channel(value: Float): Double {
        val linear = value.toDouble()

        return if (linear <= 0.04045) linear / 12.92 else ((linear + 0.055) / 1.055).pow(2.4)
    }

    private fun luminance(color: Color) =
        0.2126 * channel(color.red) + 0.7152 * channel(color.green) + 0.0722 * channel(color.blue)

    private fun contrast(one: Color, other: Color): Double {
        val a = luminance(one)
        val b = luminance(other)

        return (maxOf(a, b) + 0.05) / (minOf(a, b) + 0.05)
    }

    @Test
    fun all_desktop_themes_are_here_and_names_are_distinct() {
        assertEquals(19, Themes.size)
        assertEquals(Themes.size, Themes.map { it.name }.toSet().size)
        assertTrue(Themes.any { it.name == DEFAULT_DARK && it.dark })
        assertTrue(Themes.any { it.name == DEFAULT_LIGHT && !it.dark })
    }

    @Test
    fun text_reads_on_every_ground_of_every_theme() {
        val failures = mutableListOf<String>()

        for (theme in Themes) {
            val grounds = mapOf(
                "background" to theme.background,
                "surface" to theme.surface,
                "sunken" to theme.sunken,
                "raised" to theme.raised,
            )

            for ((ground, color) in grounds) {
                fun need(what: String, ink: Color, least: Double) {
                    val ratio = contrast(ink, color)

                    if (ratio < least) failures += "${theme.name}: $what on $ground is %.2f, needs %.1f".format(ratio, least)
                }

                need("text", theme.text, 4.5)
                need("secondary text", theme.textSecondary, 4.5)
                need("muted text", theme.textMuted, 3.0)
            }

            val onAccent = contrast(theme.onAccent, theme.accent)

            if (onAccent < 4.5) failures += "${theme.name}: ink on the accent is %.2f".format(onAccent)
        }

        assertTrue(failures.joinToString("\n"), failures.isEmpty())
    }

    @Test
    fun a_theme_is_dark_exactly_when_its_ground_is() {
        for (theme in Themes) {
            assertEquals(theme.name, theme.dark, luminance(theme.background) < 0.2)
        }
    }
}
