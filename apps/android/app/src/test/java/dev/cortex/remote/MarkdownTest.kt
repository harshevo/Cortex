package dev.cortex.remote

import dev.cortex.remote.ui.Align
import dev.cortex.remote.ui.Block
import dev.cortex.remote.ui.Mark
import dev.cortex.remote.ui.Markdown
import dev.cortex.remote.ui.StreamingMarkdown
import org.junit.Assert.assertEquals
import org.junit.Assert.assertSame
import org.junit.Assert.assertTrue
import org.junit.Test

class MarkdownTest {
    private val sample = """
        # What changed

        The parser keeps **one pass** and each `Token`, see [the note](https://example.com/a) or https://example.com/b.
        A second line of the same paragraph, with *italics* and 2 * 3 * 4 and snake_case_name.

        1. Read the file.
        2. Changed it.
           Still the second.
        - [x] done
        - [ ] not done
          - nested

        > quoted words

        ```rust
        fn main() {

            println!("hi");
        }
        ```

        | Case | Before | After |
        |:-----|-------:|:-----:|
        | small | 0.8 ms | 0.3 ms |
        | big | 910 ms | 41 ms |

        ---

        Last words.
    """.trimIndent()

    @Test
    fun blocks_come_out_as_written() {
        val blocks = Markdown.parse(sample)

        assertEquals(Block.Heading::class, blocks[0]::class)

        val paragraph = blocks[1] as Block.Paragraph

        assertTrue(paragraph.text.text.startsWith("The parser keeps one pass and each Token"))
        assertTrue(paragraph.text.text.contains("2 * 3 * 4 and snake_case_name"))

        val marks = paragraph.text.spans.map { it.mark }

        assertEquals(listOf(Mark.BOLD, Mark.CODE, Mark.LINK, Mark.LINK, Mark.ITALIC), marks)
        assertEquals("https://example.com/b", paragraph.text.spans[3].target)

        val second = blocks[3] as Block.Entry

        assertEquals("2.", second.marker)
        assertEquals("Changed it. Still the second.", second.text.text)
        assertEquals(true, (blocks[4] as Block.Entry).checked)
        assertEquals(false, (blocks[5] as Block.Entry).checked)
        assertEquals(1, (blocks[6] as Block.Entry).depth)
        assertEquals("quoted words", (blocks[7] as Block.Quote).text.text)

        val code = blocks[8] as Block.Code

        assertEquals("rust", code.language)
        assertTrue(code.code.contains("\n\n    println!"))
        assertTrue(code.closed)

        val table = blocks[9] as Block.Table

        assertEquals(listOf(Align.START, Align.END, Align.CENTER), table.aligns)
        assertEquals(2, table.rows.size)
        assertEquals(Block.Rule, blocks[10])
        assertEquals("Last words.", (blocks[11] as Block.Paragraph).text.text)
        assertEquals(12, blocks.size)
    }

    @Test
    fun text_read_as_it_arrives_is_read_as_the_whole_would_be() {
        for (piece in listOf(1, 3, 7, 40)) {
            val streaming = StreamingMarkdown()

            for (start in sample.indices step piece) {
                val so = sample.substring(0, minOf(sample.length, start + piece))
                val blocks = streaming.append(sample.substring(start, minOf(sample.length, start + piece)))

                assertEquals("after ${so.length} characters in pieces of $piece", Markdown.parse(so), blocks)
            }
        }
    }

    @Test
    fun what_is_finished_is_not_read_again() {
        val streaming = StreamingMarkdown()

        streaming.append("First paragraph.\n\nSecond")

        val first = streaming.blocks[0]

        assertEquals(1, streaming.settledCount)

        streaming.append(" grows.\n\nThird")

        assertSame(first, streaming.blocks[0])
        assertEquals(2, streaming.settledCount)
    }

    @Test
    fun an_unfinished_block_of_code_is_shown_open() {
        val blocks = Markdown.parse("```kotlin\nval a = 1")

        assertEquals(Block.Code("kotlin", "val a = 1", closed = false), blocks.single())
    }
}
