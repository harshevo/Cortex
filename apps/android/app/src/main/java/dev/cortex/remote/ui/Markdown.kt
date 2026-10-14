package dev.cortex.remote.ui

import androidx.compose.runtime.Immutable

/**
 * A model's Markdown, read once into plain values the screens draw.
 *
 * Nothing here knows of a screen: it is read on a background thread as
 * the text arrives, and what comes out never changes, so a part of the
 * conversation that has been read is never read, or drawn, again.
 */

/** Something set differently inside a line. */
enum class Mark { BOLD, ITALIC, CODE, STRIKE, LINK }

/** A stretch of a line's characters that is set as [mark]. */
@Immutable
data class Span(val start: Int, val end: Int, val mark: Mark, val target: String? = null)

/** A line's words with the Markdown signs taken out, and where they applied. */
@Immutable
data class Inline(val text: String, val spans: List<Span> = emptyList()) {
    val links: Boolean
        get() = spans.any { it.mark == Mark.LINK }
}

enum class Align { START, CENTER, END }

/** One piece of a response: each is laid out and drawn on its own. */
@Immutable
sealed interface Block {
    data class Paragraph(val text: Inline) : Block

    data class Heading(val level: Int, val text: Inline) : Block

    /** One entry of a list. [checked] is set for a task: `- [x]`. */
    data class Entry(val marker: String, val depth: Int, val text: Inline, val checked: Boolean? = null) : Block

    data class Quote(val text: Inline) : Block

    /** [closed] is false while the closing fence has not been written yet. */
    data class Code(val language: String, val code: String, val closed: Boolean = true) : Block

    data class Table(val header: List<Inline>, val rows: List<List<Inline>>, val aligns: List<Align>) : Block

    data object Rule : Block
}

object Markdown {
    private val heading = Regex("""^(#{1,6})\s+(.*?)\s*#*$""")
    private val entry = Regex("""^(\s*)([-*+]|\d{1,9}[.)])\s+(.*)$""")
    private val task = Regex("""^\[([ xX])]\s+(.*)$""")
    private val rule = Regex("""^\s{0,3}([-*_])(\s*\1){2,}\s*$""")
    private val divider = Regex("""^\s*\|?\s*:?-{1,}:?\s*(\|\s*:?-{1,}:?\s*)*\|?\s*$""")

    /** The blocks of a whole text. */
    fun parse(text: String): List<Block> {
        val blocks = ArrayList<Block>()

        blocksInto(text.split('\n'), blocks)

        return blocks
    }

    /** A line on its own: for a label, or the help under a setting. */
    fun inline(text: String): Inline {
        val out = StringBuilder(text.length)
        val spans = ArrayList<Span>()

        inlineInto(text, 0, text.length, out, spans)

        return Inline(out.toString(), spans)
    }

    /** Whether the line opens or closes a fenced block of code, and with which fence. */
    internal fun fence(line: String): String? {
        val trimmed = line.trimStart()

        return when {
            line.length - trimmed.length > 3 -> null
            trimmed.startsWith("```") -> "```"
            trimmed.startsWith("~~~") -> "~~~"
            else -> null
        }
    }

    private fun blocksInto(lines: List<String>, blocks: MutableList<Block>) {
        val paragraph = ArrayList<String>()

        fun endParagraph() {
            if (paragraph.isNotEmpty()) {
                blocks += Block.Paragraph(inline(paragraph.joinToString("\n")))

                paragraph.clear()
            }
        }

        var index = 0

        while (index < lines.size) {
            val line = lines[index]
            val trimmed = line.trim()
            val opening = fence(line)

            when {
                opening != null -> {
                    endParagraph()

                    val language = trimmed.drop(3).trim().substringBefore(' ')
                    val code = ArrayList<String>()

                    index += 1

                    while (index < lines.size && fence(lines[index]) != opening) {
                        code += lines[index]
                        index += 1
                    }

                    // A block still being written has no closing fence yet.
                    blocks += Block.Code(language, code.joinToString("\n"), closed = index < lines.size)
                }

                trimmed.isEmpty() -> endParagraph()

                heading.matches(trimmed) -> {
                    endParagraph()

                    val (signs, words) = heading.find(trimmed)!!.destructured

                    blocks += Block.Heading(signs.length, inline(words))
                }

                rule.matches(line) -> {
                    endParagraph()

                    blocks += Block.Rule
                }

                trimmed.startsWith(">") -> {
                    endParagraph()

                    val quoted = ArrayList<String>()

                    while (index < lines.size && lines[index].trim().startsWith(">")) {
                        quoted += lines[index].trim().removePrefix(">").removePrefix(" ")
                        index += 1
                    }

                    index -= 1

                    blocks += Block.Quote(inline(quoted.joinToString("\n")))
                }

                entry.matches(line) -> {
                    endParagraph()

                    val (indent, sign, first) = entry.find(line)!!.destructured
                    val words = StringBuilder(first)

                    // Lines set under the entry carry it on.
                    while (index + 1 < lines.size && continues(lines[index + 1])) {
                        index += 1

                        words.append(' ').append(lines[index].trim())
                    }

                    val done = task.find(words)

                    blocks += Block.Entry(
                        marker = if (sign.first().isDigit()) sign.dropLast(1) + "." else "•",
                        depth = (indent.replace("\t", "    ").length / 2).coerceAtMost(3),
                        text = inline(done?.groupValues?.get(2) ?: words.toString()),
                        checked = done?.let { it.groupValues[1] != " " },
                    )
                }

                '|' in line && index + 1 < lines.size && '-' in lines[index + 1] && divider.matches(lines[index + 1]) -> {
                    endParagraph()

                    val header = cells(line)

                    val aligns = cells(lines[index + 1]).map { cell ->
                        when {
                            cell.startsWith(":") && cell.endsWith(":") -> Align.CENTER
                            cell.endsWith(":") -> Align.END
                            else -> Align.START
                        }
                    }

                    val rows = ArrayList<List<Inline>>()

                    index += 2

                    while (index < lines.size && '|' in lines[index] && lines[index].isNotBlank()) {
                        rows += cells(lines[index]).map(::inline)
                        index += 1
                    }

                    index -= 1

                    blocks += Block.Table(header.map(::inline), rows, aligns)
                }

                else -> paragraph += line.trimEnd()
            }

            index += 1
        }

        endParagraph()
    }

    /** A line that belongs to the list entry above it. */
    private fun continues(line: String): Boolean =
        line.isNotBlank() && line.first().isWhitespace() && !entry.matches(line) && fence(line) == null

    private fun cells(line: String): List<String> =
        line.trim().removePrefix("|").removeSuffix("|").split('|').map { it.trim() }

    private const val ESCAPED = "\\`*_{}[]()#+-.!|~>"

    private fun inlineInto(source: String, from: Int, to: Int, out: StringBuilder, spans: MutableList<Span>) {
        var index = from

        while (index < to) {
            val char = source[index]

            when {
                char == '\\' && index + 1 < to && source[index + 1] in ESCAPED -> {
                    out.append(source[index + 1])

                    index += 2
                }

                char == '`' -> {
                    var ticks = 1

                    while (index + ticks < to && source[index + ticks] == '`') ticks += 1

                    val fence = "`".repeat(ticks)
                    val close = source.indexOf(fence, index + ticks)

                    if (close < 0 || close + ticks > to) {
                        out.append(fence)

                        index += ticks
                    } else {
                        val start = out.length

                        out.append(source, index + ticks, close)

                        spans += Span(start, out.length, Mark.CODE)

                        index = close + ticks
                    }
                }

                char == '*' || char == '_' || char == '~' -> {
                    val double = index + 1 < to && source[index + 1] == char
                    val sign = if (double) "$char$char" else "$char"
                    val close = if (char == '~' && !double) -1 else closing(source, sign, index, to)

                    if (close < 0) {
                        out.append(sign)

                        index += sign.length
                    } else {
                        val start = out.length

                        inlineInto(source, index + sign.length, close, out, spans)

                        val mark = when {
                            char == '~' -> Mark.STRIKE
                            double -> Mark.BOLD
                            else -> Mark.ITALIC
                        }

                        spans += Span(start, out.length, mark)

                        index = close + sign.length
                    }
                }

                char == '[' -> {
                    val label = source.indexOf("](", index + 1)
                    val end = if (label in (index + 1) until to) source.indexOf(')', label + 2) else -1

                    if (end < 0 || end >= to || '\n' in source.substring(index, end)) {
                        out.append(char)

                        index += 1
                    } else {
                        val start = out.length

                        inlineInto(source, index + 1, label, out, spans)

                        spans += Span(start, out.length, Mark.LINK, source.substring(label + 2, end).trim())

                        index = end + 1
                    }
                }

                char == 'h' && (source.startsWith("https://", index) || source.startsWith("http://", index)) &&
                    (index == 0 || !source[index - 1].isLetterOrDigit()) -> {
                    var end = index

                    while (end < to && !source[end].isWhitespace() && source[end] !in "<>\"`") end += 1

                    // The full stop after an address is the sentence's.
                    while (end > index && source[end - 1] in ".,;:!?)*_'") end -= 1

                    val start = out.length

                    out.append(source, index, end)

                    spans += Span(start, out.length, Mark.LINK, source.substring(index, end))

                    index = end
                }

                else -> {
                    out.append(char)

                    index += 1
                }
            }
        }
    }

    /**
     * Where the sign that opens at [open] is closed, or -1: after words,
     * not after a space, and for an underscore not inside a word, so
     * that `2 * 3 * 4` and `snake_case_name` are left as they are.
     */
    private fun closing(source: String, sign: String, open: Int, to: Int): Int {
        val first = open + sign.length

        if (first >= to || source[first].isWhitespace()) return -1

        if (sign[0] == '_' && open > 0 && source[open - 1].isLetterOrDigit()) return -1

        var at = source.indexOf(sign, first + 1)

        while (at in 0..(to - sign.length)) {
            val after = at + sign.length

            val single = sign.length == 2 ||
                ((after >= to || source[after] != sign[0]) && source[at - 1] != sign[0])

            val word = sign[0] != '_' || after >= to || !source[after].isLetterOrDigit()

            if (single && word && !source[at - 1].isWhitespace()) {
                return at
            }

            at = source.indexOf(sign, at + 1)
        }

        return -1
    }
}

/**
 * A response read as it is written.
 *
 * A blank line outside a block of code ends whatever came before it, so
 * everything up to the last such line is read once and kept, and only
 * what follows it, the piece still growing, is read again as text is
 * added. What [blocks] holds for the finished part are the same objects
 * every time, which is how the screen knows not to lay them out again.
 */
class StreamingMarkdown {
    private val text = StringBuilder()

    /** Up to where the text has been read for good. */
    private var settledEnd = 0

    private val settled = ArrayList<Block>()

    /** How many of [blocks], from the first, will never change. */
    val settledCount: Int
        get() = settled.size

    var blocks: List<Block> = emptyList()
        private set

    val isEmpty: Boolean
        get() = text.isEmpty()

    fun clear() {
        text.setLength(0)
        settled.clear()

        settledEnd = 0
        blocks = emptyList()
    }

    /** Takes in the next piece of the text. Returns all its blocks. */
    fun append(piece: String): List<Block> {
        text.append(piece)

        val boundary = lastBoundary()

        if (boundary > settledEnd) {
            settled += Markdown.parse(text.substring(settledEnd, boundary))

            settledEnd = boundary
        }

        val tail = Markdown.parse(text.substring(settledEnd))

        blocks = if (tail.isEmpty()) ArrayList(settled) else settled + tail

        return blocks
    }

    /**
     * The start of the last line that follows a blank line, or the end
     * of a block of code, among the lines that are complete.
     */
    private fun lastBoundary(): Int {
        var boundary = settledEnd
        var start = settledEnd
        var fence: String? = null

        while (true) {
            val end = text.indexOf("\n", start)

            if (end < 0) break

            val line = text.substring(start, end)
            val sign = Markdown.fence(line)

            when {
                fence != null -> if (sign == fence) {
                    fence = null
                    boundary = end + 1
                }

                sign != null -> fence = sign
                line.isBlank() -> boundary = end + 1
            }

            start = end + 1
        }

        return boundary
    }
}
