package dev.cortex.remote.ui

import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.scaleIn
import androidx.compose.animation.scaleOut
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.slideOutVertically
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.ArrowDownward
import androidx.compose.material.icons.rounded.EditNote
import androidx.compose.material.icons.rounded.Hub
import androidx.compose.material.icons.rounded.PanTool
import androidx.compose.material.icons.rounded.Schedule
import androidx.compose.material.icons.rounded.Tune
import androidx.compose.material.icons.rounded.Warning
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.input.nestedscroll.NestedScrollConnection
import androidx.compose.ui.input.nestedscroll.NestedScrollSource
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.max
import dev.cortex.remote.AppModel
import dev.cortex.remote.Busy
import dev.cortex.remote.ChatFeed
import dev.cortex.remote.Meter
import kotlinx.coroutines.launch
import uniffi.cortex_mobile.Answer
import uniffi.cortex_mobile.ApprovalView
import uniffi.cortex_mobile.CellView
import kotlin.math.PI
import kotlin.math.sin

/** One chat: its conversation, what the run is doing, and the composer. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun ConversationScreen(model: AppModel, chat: String, back: () -> Unit, openSettings: (String?) -> Unit) {
    val view = LocalView.current
    val feed = model.feed(chat)
    val info = model.chat(chat)

    var showingMemory by remember { mutableStateOf(false) }
    var selecting by remember { mutableStateOf<String?>(null) }

    LaunchedEffect(model.stage) {
        showingMemory = model.stage == "memory"
    }

    // A question arriving and a run ending are both worth feeling.
    LaunchedEffect(feed.approvals.size) {
        if (feed.approvals.isNotEmpty()) Haptics.warn(view)
    }

    LaunchedEffect(feed.running) {
        if (!feed.running && feed.rows.isNotEmpty()) Haptics.confirm(view)
    }

    val context = remember(model, chat, info?.model) {
        RowContext(model, chat, info?.model.orEmpty()) { text -> selecting = text }
    }

    Column(
        Modifier
            .fillMaxSize()
            .background(colors.background)
            .navigationBarsPadding()
            .imePadding(),
    ) {
        TopBar(
            title = info?.title?.ifEmpty { null } ?: "New chat",
            subtitle = listOfNotNull(info?.placeName, info?.model?.takeIf { it.isNotEmpty() }).joinToString(" · "),
            back = back,
        ) {
            BarButton(Icons.Rounded.Hub, "Memory") { showingMemory = true }
            BarButton(Icons.Rounded.Tune, "Settings") { openSettings(null) }
            BarButton(Icons.Rounded.EditNote, "New session", enabled = !feed.running) {
                model.core.newSession(chat, null)
            }
        }

        Transcript(feed, context, info?.problem, model.state.onPhone, Modifier.weight(1f))

        Column(
            Modifier
                .measure()
                .padding(horizontal = Gutter)
                .padding(top = Space.xs, bottom = Space.s),
            verticalArrangement = Arrangement.spacedBy(Space.s),
        ) {
            AnimatedContent(
                targetState = feed.approvals.firstOrNull(),
                transitionSpec = {
                    (slideInVertically(settle()) { it / 2 } + fadeIn()) togetherWith
                        (slideOutVertically(settle()) { it / 2 } + fadeOut())
                },
                contentKey = { it?.id },
                label = "approval",
            ) { approval ->
                if (approval != null) {
                    ApprovalCard(approval, waiting = feed.approvals.size - 1) { answer ->
                        model.core.answer(chat, approval.id, answer)
                    }
                } else {
                    Spacer(Modifier.fillMaxWidth())
                }
            }

            Composer(model, chat, feed, ready = info?.ready ?: false, openSettings = openSettings)
        }
    }

    if (showingMemory) {
        MemorySheet(model, chat) {
            showingMemory = false
            model.stage = null
        }
    }

    selecting?.let { text ->
        SelectSheet(text) { selecting = null }
    }
}

/**
 * The conversation. Every row has a key that stays the same for as long
 * as it is the same thing, so text arriving at the end touches only the
 * row being written; and the list stays at the newest text only while
 * the person has not scrolled away from it.
 */
@Composable
private fun Transcript(feed: ChatFeed, context: RowContext, problem: String?, onPhone: Boolean, modifier: Modifier) {
    val list = rememberLazyListState()
    val scope = rememberCoroutineScope()
    val layouts = rememberProseLayouts()

    // Whether the newest text is followed. Only the person's own finger
    // lets go of it, and coming back to the end takes it up again.
    var pinned by remember { mutableStateOf(true) }

    val finger = remember {
        object : NestedScrollConnection {
            override fun onPreScroll(available: Offset, source: NestedScrollSource): Offset {
                if (source == NestedScrollSource.UserInput && available.y > 0f) pinned = false

                return Offset.Zero
            }
        }
    }

    LaunchedEffect(list) {
        snapshotFlow { list.isScrollInProgress }.collect { moving ->
            if (!moving && !list.canScrollForward) pinned = true
        }
    }

    // New rows, new text, and the keyboard taking room all keep the end in view.
    LaunchedEffect(list) {
        snapshotFlow {
            listOf(feed.rows.size, feed.streaming, feed.busy, feed.queued.size, list.layoutInfo.viewportSize)
        }.collect {
            if (pinned) list.scrollToItem(Int.MAX_VALUE / 2)
        }
    }

    BoxWithConstraints(modifier.fillMaxWidth()) {
        val side = max(Gutter, (maxWidth - Measure) / 2)

        androidx.compose.runtime.CompositionLocalProvider(LocalProse provides layouts) {
            LazyColumn(
                state = list,
                contentPadding = PaddingValues(start = side, end = side, top = Space.s, bottom = Space.m),
                verticalArrangement = Arrangement.spacedBy(6.dp),
                modifier = Modifier.fillMaxSize().nestedScroll(finger),
            ) {
                // Read here, where the list is laid out, so that the rows and the text being
                // written are always of the same moment: the list can be laid out again
                // between a change and the screen being composed again.
                val rows = feed.rows
                val streaming = feed.streaming
                val busy = feed.busy
                val queued = feed.queued

                // A response that has just been completed is in the rows already.
                val showStreaming = !streaming.isEmpty &&
                    rows.takeLast(4).none { it.key.endsWith("-${streaming.item}") || it.key.startsWith("b-${streaming.item}-") }

                if (problem != null) {
                    item("problem") {
                        Line(Modifier.fillMaxWidth().card()) {
                            Icon(Icons.Rounded.Warning, null, tint = colors.bad, modifier = Modifier.size(18.dp))
                            Text(problem, style = Type.footnote, color = colors.bad)
                        }
                    }
                }

                if (rows.isEmpty() && streaming.isEmpty && busy == null) {
                    item("empty") { EmptyChat(onPhone) }
                }

                items(rows.size, key = { rows[it].key }, contentType = { rows[it].type }) { index ->
                    RowView(rows[index], context)
                }

                if (showStreaming) {
                    if (streaming.thinking.isNotEmpty()) {
                        item(Rows.thought(streaming.item), contentType = ThoughtRow::class) {
                            RowView(ThoughtRow(Rows.thought(streaming.item), streaming.thinking, live = streaming.blocks.isEmpty()), context)
                        }
                    }

                    val blocks = streaming.blocks

                    items(blocks.size, key = { Rows.block(streaming.item, it) }, contentType = { blocks[it]::class }) { index ->
                        BlockView(
                            blocks[index],
                            Modifier.fillMaxWidth().padding(top = if (index == 0) Space.s else 0.dp),
                            growing = index == blocks.lastIndex,
                        )
                    }
                }

                if (busy != null) {
                    item("activity") { ActivityLine(busy, feed.meter) }
                }

                items(queued.size, key = { "queued-$it" }) { index ->
                    Box(Modifier.fillMaxWidth(), contentAlignment = Alignment.CenterEnd) {
                        Line(
                            Modifier
                                .padding(start = 48.dp)
                                .clip(Shapes.bubble)
                                .background(colors.raised.copy(alpha = 0.55f))
                                .border(1.dp, colors.border, Shapes.bubble)
                                .padding(horizontal = Space.m, vertical = Space.s),
                            gap = 6.dp,
                        ) {
                            Icon(Icons.Rounded.Schedule, "Waiting to be read", tint = colors.textMuted, modifier = Modifier.size(14.dp))
                            Text(queued[index], style = Type.footnote, color = colors.textSecondary)
                        }
                    }
                }

                item("bottom") { Spacer(Modifier.height(Space.l)) }
            }
        }

        AnimatedVisibility(
            visible = !pinned,
            enter = scaleIn() + fadeIn(),
            exit = scaleOut() + fadeOut(),
            modifier = Modifier.align(Alignment.BottomCenter).padding(Space.m),
        ) {
            Line(
                Modifier
                    .pressable {
                        pinned = true

                        scope.launch { list.animateScrollToItem(Int.MAX_VALUE / 2) }
                    }
                    .clip(Shapes.pill)
                    .background(colors.raised)
                    .border(1.dp, colors.border, Shapes.pill)
                    .padding(horizontal = 14.dp, vertical = Space.s),
                gap = 6.dp,
            ) {
                Icon(Icons.Rounded.ArrowDownward, null, tint = colors.text, modifier = Modifier.size(16.dp))
                Text(if (feed.running) "Jump to latest" else "Latest", style = Type.footnote, color = colors.text)
            }
        }
    }
}

@Composable
private fun EmptyChat(onPhone: Boolean) {
    Column(
        Modifier.fillMaxWidth().padding(top = 56.dp, start = Space.m, end = Space.m),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(14.dp),
    ) {
        LogoBadge(size = 56.dp)

        Text("What should Cortex do?", style = Type.title(22), color = colors.text)

        Text(
            if (onPhone) {
                "It runs on this phone, in this chat's project, and asks before it changes anything. Keep the app open while it works."
            } else {
                "It runs on your computer, in this chat's project. You can leave: it carries on, and asks here before it changes anything."
            },
            style = Type.callout,
            color = colors.textSecondary,
            textAlign = TextAlign.Center,
        )
    }
}

/** What the run is busy with while there is nothing to read, with its loop so far. */
@Composable
private fun ActivityLine(busy: Busy, meter: Meter) {
    val phase by rememberInfiniteTransition(label = "dots").animateFloat(
        initialValue = 0f,
        targetValue = (2 * PI).toFloat(),
        animationSpec = infiniteRepeatable(tween(1200, easing = LinearEasing), RepeatMode.Restart),
        label = "dots",
    )

    val accent = colors.accent

    Line(Modifier.fillMaxWidth().padding(vertical = Space.xs), gap = 10.dp) {
        // The phase is read while drawing only: the dots move without composing anything.
        Canvas(Modifier.size(width = 26.dp, height = 10.dp)) {
            for (index in 0 until 3) {
                val wave = sin(phase + index * 0.9f)

                drawCircle(
                    accent.copy(alpha = (0.65f + 0.35f * wave).coerceIn(0.2f, 1f)),
                    radius = 3.dp.toPx() * (1 + 0.35f * wave),
                    center = Offset(4.dp.toPx() + index * 9.dp.toPx(), size.height / 2),
                )
            }
        }

        Text(
            busy.text,
            style = Type.subhead.copy(fontWeight = FontWeight.Normal),
            color = colors.textSecondary,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
            modifier = Modifier.weight(1f, fill = false),
        )

        if (busy.seconds >= 3) {
            Text("${busy.seconds}s", style = Type.caption, color = colors.textMuted)
        }

        Spacer(Modifier.weight(1f))

        Cells(meter.cells)
    }
}

/** The loop of the run, one square per thing done. */
@Composable
fun Cells(cells: List<CellView>) {
    val palette = colors

    Canvas(Modifier.size(width = (cells.size * 8).dp, height = 9.dp)) {
        cells.forEachIndexed { index, cell ->
            val color = when (cell) {
                CellView.THINKING -> palette.model.copy(alpha = 0.45f)
                CellView.MODEL -> palette.model
                CellView.OBSERVATION -> palette.accent
                CellView.FAILED_OBSERVATION -> palette.bad
                CellView.CHECK_PASSED -> palette.good
                CellView.CHECK_FAILED -> palette.warning
            }

            drawRoundRect(
                color,
                topLeft = Offset(index * 8.dp.toPx(), 0f),
                size = Size(5.dp.toPx(), size.height),
                cornerRadius = CornerRadius(1.5.dp.toPx()),
            )
        }
    }
}

/** A tool's question, with its answers. */
@Composable
private fun ApprovalCard(approval: ApprovalView, waiting: Int, answer: (Answer) -> Unit) {
    Column(
        Modifier
            .fillMaxWidth()
            .clip(Shapes.card)
            .background(colors.surface)
            .border(1.dp, colors.warning.copy(alpha = 0.6f), Shapes.card)
            .padding(14.dp),
        verticalArrangement = Arrangement.spacedBy(Space.m),
    ) {
        Line {
            Icon(Icons.Rounded.PanTool, null, tint = colors.warning, modifier = Modifier.size(18.dp))

            Text(
                "Cortex wants to use ${approval.tool.replace('_', ' ')}",
                style = Type.subhead.copy(fontWeight = FontWeight.SemiBold),
                color = colors.text,
                modifier = Modifier.weight(1f),
            )

            if (waiting > 0) {
                Text("+$waiting", style = Type.caption, color = colors.textMuted)
            }
        }

        Text(
            approval.summary,
            style = Type.mono,
            color = colors.text,
            maxLines = 8,
            overflow = TextOverflow.Ellipsis,
            modifier = Modifier
                .fillMaxWidth()
                .clip(Shapes.small)
                .background(colors.sunken)
                .padding(10.dp),
        )

        Row(horizontalArrangement = Arrangement.spacedBy(Space.s)) {
            AnswerButton("Deny", colors.raised, colors.text, Modifier.weight(1f)) { answer(Answer.DENY) }
            AnswerButton("Always", colors.raised, colors.text, Modifier.weight(1f)) { answer(Answer.ALWAYS_ALLOW) }
            AnswerButton("Allow", colors.accent, colors.onAccent, Modifier.weight(1f)) { answer(Answer.ALLOW) }
        }
    }
}

@Composable
private fun AnswerButton(text: String, fill: Color, ink: Color, modifier: Modifier, onClick: () -> Unit) {
    Box(
        modifier
            .heightIn(min = 46.dp)
            .pressable(onClick = onClick)
            .clip(Shapes.field)
            .background(fill),
        contentAlignment = Alignment.Center,
    ) {
        Text(text, style = Type.subhead.copy(fontWeight = FontWeight.SemiBold), color = ink)
    }
}

/** A text shown whole, to select a part of, or copy all of. Opened by a long press. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun SelectSheet(text: String, close: () -> Unit) {
    val clipboard = LocalClipboardManager.current
    val view = LocalView.current

    ModalBottomSheet(
        onDismissRequest = close,
        sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true),
        containerColor = colors.background,
    ) {
        Column(
            Modifier
                .padding(horizontal = Gutter)
                .padding(bottom = Space.xl)
                .navigationBarsPadding(),
            verticalArrangement = Arrangement.spacedBy(Space.m),
        ) {
            Line {
                Text("Select text", style = Type.title(20), color = colors.text, modifier = Modifier.weight(1f))

                Box(
                    Modifier
                        .pressable {
                            clipboard.setText(AnnotatedString(text))
                            Haptics.confirm(view)
                            close()
                        }
                        .clip(Shapes.pill)
                        .background(colors.accent)
                        .padding(horizontal = 14.dp, vertical = Space.s),
                ) {
                    Text("Copy all", style = Type.footnote.copy(fontWeight = FontWeight.SemiBold), color = colors.onAccent)
                }
            }

            SelectionContainer(
                Modifier
                    .fillMaxWidth()
                    .heightIn(max = 520.dp)
                    .clip(Shapes.field)
                    .background(colors.surface)
                    .verticalScroll(rememberScrollState())
                    .padding(14.dp),
            ) {
                Text(text, style = Type.body, color = colors.text)
            }
        }
    }
}
