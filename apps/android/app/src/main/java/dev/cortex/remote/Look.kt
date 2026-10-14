package dev.cortex.remote

import android.content.Context
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import dev.cortex.remote.ui.FOLLOW_SYSTEM
import dev.cortex.remote.ui.Themes

/** A size the app's text can be set in, as a share of the system's. */
enum class TextSize(val label: String, val scale: Float) {
    SMALL("Small", 0.9f),
    DEFAULT("Default", 1f),
    LARGE("Large", 1.12f),
    LARGER("Larger", 1.25f),
}

/**
 * How the app looks: its theme and the size of its text. Kept on the
 * phone, and in use the moment it is changed: the screens read it as
 * they draw.
 *
 * Two small values read once at launch, so the system's own preferences
 * file is enough and no library is brought in for it.
 */
@Stable
class Look(context: Context) {
    private val store = context.getSharedPreferences("look", Context.MODE_PRIVATE)

    /** A theme's name, or [FOLLOW_SYSTEM]. */
    var theme: String by mutableStateOf(
        store.getString(THEME, null)?.takeIf { name -> name == FOLLOW_SYSTEM || Themes.any { it.name == name } }
            ?: FOLLOW_SYSTEM,
    )
        private set

    var textScale: Float by mutableFloatStateOf(store.getFloat(TEXT, TextSize.DEFAULT.scale))
        private set

    val textSize: TextSize
        get() = TextSize.entries.minBy { kotlin.math.abs(it.scale - textScale) }

    fun choose(theme: String) {
        if (theme != FOLLOW_SYSTEM && Themes.none { it.name == theme }) return

        this.theme = theme

        store.edit().putString(THEME, theme).apply()
    }

    fun choose(size: TextSize) {
        textScale = size.scale

        store.edit().putFloat(TEXT, size.scale).apply()
    }

    private companion object {
        const val THEME = "theme"
        const val TEXT = "text"
    }
}
