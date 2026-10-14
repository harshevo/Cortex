package dev.cortex.remote.ui

import androidx.compose.runtime.Immutable
import uniffi.cortex_mobile.ItemView

/**
 * One row of a conversation as the list draws it.
 *
 * The core hands over items; a response is long, so each of its blocks
 * is a row of its own and the list only ever lays out the ones on
 * screen. Rows are made on a background thread and never change.
 */
@Immutable
sealed interface Row {
    /** The same for as long as the row is the same thing, across updates. */
    val key: String

    /** Rows of one type are drawn the same way, so the list reuses them. */
    val type: Any
        get() = this::class
}

data class UserRow(override val key: String, val id: ULong, val text: String, val images: Int) : Row

/** The model's reasoning, folded away. [live] while it is still being written. */
data class ThoughtRow(override val key: String, val text: String, val live: Boolean) : Row

/**
 * One block of a response. [first] is the block that opens it.
 * [source] is the whole response, for copying; null while it is written.
 */
data class BlockRow(
    override val key: String,
    val block: Block,
    val first: Boolean,
    val source: String?,
) : Row {
    override val type: Any
        get() = block::class
}

/** The quiet line under a response: how long it took, how fast it came. */
data class MetaRow(override val key: String, val source: String, val seconds: Float, val speed: Float?) : Row

/** Everything else, drawn from the core's own item. */
data class ItemRow(override val key: String, val item: ItemView) : Row {
    override val type: Any
        get() = item::class
}

/** The response being written: its blocks so far, the last of them still growing. */
@Immutable
class Streaming(
    /** The id the response will have in the conversation, so its rows keep their keys. */
    val item: ULong,
    val blocks: List<Block>,
    val thinking: String,
) {
    val isEmpty: Boolean
        get() = blocks.isEmpty() && thinking.isEmpty()

    companion object {
        val None = Streaming(0uL, emptyList(), "")
    }
}

object Rows {
    fun thought(item: ULong) = "t-$item"

    fun block(item: ULong, index: Int) = "b-$item-$index"

    /** The rows of one item. Reads a response's Markdown: call it off the main thread. */
    fun of(item: ItemView): List<Row> = when (item) {
        is ItemView.User -> listOf(UserRow("u-${item.id}", item.id, item.text, item.images.toInt()))

        is ItemView.Assistant -> buildList {
            if (item.thinking.isNotEmpty()) {
                add(ThoughtRow(thought(item.id), item.thinking, live = false))
            }

            if (item.text.isNotEmpty()) {
                Markdown.parse(item.text).forEachIndexed { index, block ->
                    add(BlockRow(block(item.id, index), block, first = index == 0, source = item.text))
                }

                add(MetaRow("m-${item.id}", item.text, item.seconds, item.speed))
            }
        }

        is ItemView.Briefing -> listOf(ItemRow("i-${item.id}", item))
        is ItemView.Tools -> listOf(ItemRow("i-${item.id}", item))
        is ItemView.Checks -> listOf(ItemRow("i-${item.id}", item))
        is ItemView.Review -> listOf(ItemRow("i-${item.id}", item))
        is ItemView.Notice -> listOf(ItemRow("i-${item.id}", item))
        is ItemView.Summary -> listOf(ItemRow("i-${item.id}", item))
    }
}
