package dev.cortex.remote

import androidx.compose.runtime.Immutable
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.referentialEqualityPolicy
import androidx.compose.runtime.setValue
import dev.cortex.remote.ui.Row
import dev.cortex.remote.ui.Rows
import dev.cortex.remote.ui.Streaming
import dev.cortex.remote.ui.StreamingMarkdown
import uniffi.cortex_mobile.ApprovalView
import uniffi.cortex_mobile.CellView
import uniffi.cortex_mobile.Cortex
import uniffi.cortex_mobile.LiveView

/** How many of a run's squares the strip has room for. */
private const val CELLS = 12

/** What the run is busy with while there is nothing to read. */
@Immutable
data class Busy(val text: String, val seconds: Int)

/** A run's numbers. */
@Immutable
data class Meter(
    val speed: Int? = null,
    val inputTokens: ULong = 0uL,
    val outputTokens: ULong = 0uL,
    val cells: List<CellView> = emptyList(),
)

/**
 * One chat as the screens read it.
 *
 * Each thing a screen shows is a state of its own, and is only written
 * when it differs, so that text arriving wakes the row being written and
 * nothing else: not the composer, not the bar, not the rows above.
 * Written on the main thread only.
 */
@Stable
class ChatFeed {
    /** The conversation. Only its end ever changes. */
    val rows = mutableStateListOf<Row>()

    var streaming: Streaming by mutableStateOf(Streaming.None, referentialEqualityPolicy())
        private set

    var running: Boolean by mutableStateOf(false)
        private set

    var canContinue: Boolean by mutableStateOf(false)
        private set

    var busy: Busy? by mutableStateOf(null)
        private set

    var approvals: List<ApprovalView> by mutableStateOf(emptyList())
        private set

    var suggestion: String? by mutableStateOf(null)
        private set

    var queued: List<String> by mutableStateOf(emptyList())
        private set

    var meter: Meter by mutableStateOf(Meter())
        private set

    internal fun take(update: FeedUpdate) {
        update.rows?.let { (keep, fresh) ->
            if (keep < rows.size) rows.removeRange(keep, rows.size)

            rows.addAll(fresh)
        }

        update.streaming?.let { streaming = it }

        update.live?.let { live ->
            running = live.running
            canContinue = live.canContinue
            busy = live.activity?.let { Busy(it, live.activitySeconds.toInt()) }
            approvals = live.approvals
            suggestion = live.suggestion
            queued = live.queued

            meter = Meter(
                speed = live.speed?.toInt(),
                inputTokens = live.stats.inputTokens,
                outputTokens = live.stats.outputTokens,
                cells = live.stats.cells.takeLast(CELLS),
            )
        }
    }

}

/** What one look at the core found changed in a chat. Made on the fetching thread. */
internal class FeedUpdate(
    /** How many rows to keep, and the ones that follow them. */
    val rows: Pair<Int, List<Row>>?,
    val streaming: Streaming?,
    val live: LiveView?,
)

/**
 * Follows one chat in the core, asking only for what is new: the items
 * that changed and the text that was added. Used on the fetching thread
 * only.
 */
internal class ChatFollower(private val core: Cortex, private val chat: String) {
    private var revision = 0uL

    /** The row each item held begins at. */
    private val starts = ArrayList<Int>()
    private var rowCount = 0

    private var response = 0uL
    private var textEnd = 0uL
    private var thinkingEnd = 0uL

    private val markdown = StreamingMarkdown()
    private val thinking = StringBuilder()
    private var shown = Streaming.None

    /** The conversation's changes since the last look, or null when there are none. */
    fun items(): Pair<Int, List<Row>>? {
        val delta = core.itemsSince(chat, revision)

        revision = delta.revision

        val keep = delta.keep.toInt().coerceAtMost(starts.size)

        if (keep == starts.size && delta.items.isEmpty()) return null

        val keepRows = if (keep < starts.size) starts[keep] else rowCount

        while (starts.size > keep) starts.removeAt(starts.lastIndex)

        val fresh = ArrayList<Row>()

        for (item in delta.items) {
            starts += keepRows + fresh.size

            fresh += Rows.of(item)
        }

        rowCount = keepRows + fresh.size

        return keepRows to fresh
    }

    /** The response being written, or null when it is as it was. */
    fun streaming(): Streaming? {
        val view = core.writingSince(chat, response, textEnd, thinkingEnd)

        response = view.response
        textEnd = view.textEnd
        thinkingEnd = view.thinkingEnd

        if (!view.active) {
            markdown.clear()
            thinking.setLength(0)

            return Streaming.None.takeIf { shown !== it }?.also { shown = it }
        }

        if (view.whole) {
            markdown.clear()
            thinking.setLength(0)
        }

        if (!view.whole && view.text.isEmpty() && view.thinking.isEmpty() && shown.item == view.item) return null

        thinking.append(view.thinking)

        val blocks = if (view.text.isEmpty()) markdown.blocks else markdown.append(view.text)

        shown = Streaming(view.item, blocks, if (view.thinking.isEmpty()) shown.thinkingOr(thinking) else thinking.toString())

        return shown
    }

    fun live(): LiveView = core.liveStatus(chat)

    private fun Streaming.thinkingOr(text: StringBuilder): String =
        if (thinking.length == text.length) thinking else text.toString()
}
