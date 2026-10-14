package dev.cortex.remote.ui

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.rounded.KeyboardArrowLeft
import androidx.compose.material.icons.automirrored.rounded.KeyboardArrowRight
import androidx.compose.material.icons.rounded.Insights
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import dev.cortex.remote.AppModel
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.withContext
import uniffi.cortex_mobile.AdviserView
import uniffi.cortex_mobile.EffectView
import uniffi.cortex_mobile.MetricView
import uniffi.cortex_mobile.UsageDayView
import uniffi.cortex_mobile.UsageMonthView
import uniffi.cortex_mobile.VerdictView
import uniffi.cortex_mobile.WeekView
import uniffi.cortex_mobile.effectReport
import uniffi.cortex_mobile.usageMonth
import java.text.DateFormat
import java.text.DateFormatSymbols
import java.util.Calendar
import java.util.Date
import java.util.TimeZone
import kotlin.math.abs
import kotlin.math.ln
import kotlin.math.max

/** How often the records are read again while the screen is open. */
private const val REFRESH_MS = 30_000L

private val WEEKDAYS = listOf("M", "T", "W", "T", "F", "S", "S")

private fun runs(count: ULong) = if (count == 1uL) "1 run" else "$count runs"

/** How far the phone's clock is ahead of UTC, in minutes. */
private fun utcOffset(): Int = TimeZone.getDefault().getOffset(System.currentTimeMillis()) / 60_000

/**
 * What the agent on this phone has used, day by day, and whether the
 * parts that advise it help: runs with each adviser speaking are
 * compared with runs it was held out of, chosen at random.
 */
@Composable
fun InsightsScreen(model: AppModel, back: () -> Unit) {
    var tab by rememberSaveable { mutableIntStateOf(0) }

    Column(Modifier.fillMaxSize().background(colors.background)) {
        TopBar(title = "Insights", subtitle = "Recorded on this phone", back = back)

        Row(
            Modifier
                .measure()
                .padding(horizontal = Gutter)
                .padding(bottom = Space.s)
                .fillMaxWidth()
                .clip(Shapes.pill)
                .background(colors.sunken)
                .padding(3.dp),
        ) {
            for ((index, name) in listOf("Tokens", "Does it help?").withIndex()) {
                Box(
                    Modifier
                        .weight(1f)
                        .heightIn(min = 36.dp)
                        .clip(Shapes.pill)
                        .background(if (tab == index) colors.surface else colors.sunken)
                        .pressable { tab = index },
                    contentAlignment = Alignment.Center,
                ) {
                    Text(name, style = Type.subhead, color = if (tab == index) colors.text else colors.textSecondary)
                }
            }
        }

        Column(
            Modifier
                .fillMaxSize()
                .verticalScroll(rememberScrollState())
                .measure()
                .padding(horizontal = Gutter)
                .padding(bottom = Space.xxl)
                .navigationBarsPadding(),
            verticalArrangement = Arrangement.spacedBy(Space.l),
        ) {
            if (tab == 0) Tokens(model.folder) else Effect(model.folder)
        }
    }
}

@Composable
private fun Tokens(folder: String) {
    val today = Calendar.getInstance()

    var year by rememberSaveable { mutableIntStateOf(today.get(Calendar.YEAR)) }
    var month by rememberSaveable { mutableIntStateOf(today.get(Calendar.MONTH) + 1) }
    var picked by rememberSaveable { mutableStateOf<String?>(null) }

    val shown by produceState<UsageMonthView?>(null, folder, year, month) {
        value = null

        while (true) {
            value = withContext(Dispatchers.IO) {
                usageMonth(folder, year, month.toUInt(), utcOffset(), System.currentTimeMillis().toULong())
            }

            delay(REFRESH_MS)
        }
    }

    val current = year == today.get(Calendar.YEAR) && month == today.get(Calendar.MONTH) + 1

    Line {
        BarButton(Icons.AutoMirrored.Rounded.KeyboardArrowLeft, "Month before") {
            if (month == 1) { month = 12; year -= 1 } else month -= 1
            picked = null
        }

        Text(
            "${DateFormatSymbols().months[month - 1]} $year",
            style = Type.title(20),
            color = colors.text,
            textAlign = TextAlign.Center,
            modifier = Modifier.weight(1f),
        )

        BarButton(Icons.AutoMirrored.Rounded.KeyboardArrowRight, "Month after", enabled = !current) {
            if (month == 12) { month = 1; year += 1 } else month += 1
            picked = null
        }
    }

    val view = shown ?: run {
        Text("Reading the records…", style = Type.callout, color = colors.textMuted)
        return
    }

    Tiles(view)

    Column(Modifier.fillMaxWidth().card(), verticalArrangement = Arrangement.spacedBy(Space.s)) {
        SectionHeading("Each day")

        CalendarGrid(view, picked) { picked = if (picked == it) null else it }

        view.days.firstOrNull { it.date == picked }?.let { DayDetail(it) }
    }

    if (view.tokens > 0uL) {
        Hours(view)
        Models(view)
        Sessions(view)
    }

    Text(view.costNote, style = Type.caption, color = colors.textMuted)
}

@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun Tiles(view: UsageMonthView) {
    val tiles = buildList {
        add("Tokens" to compact(view.tokens))
        add("Cost" to if (view.cost == "no price known") "unpriced" else view.cost)
        add("Sessions" to view.sessions.toString())
        add("Model calls" to compact(view.calls))
        add("Streak" to if (view.streak == 1u) "1 day" else "${view.streak} days")
        view.cacheHit?.let { add("From cache" to "%.0f%%".format(it * 100)) }
        view.favouriteModel?.let { add("Favourite model" to it) }
        view.peakDay?.let { add("Peak day" to "${it.takeLast(2).trimStart('0')} · ${compact(view.peakDayTokens)}") }
        view.peakHour?.let { add("Peak hour" to "%02d:00".format(it.toInt())) }
    }

    FlowRow(
        Modifier.fillMaxWidth(),
        horizontalArrangement = Arrangement.spacedBy(Space.s),
        verticalArrangement = Arrangement.spacedBy(Space.s),
        maxItemsInEachRow = 3,
    ) {
        for ((title, value) in tiles) {
            Column(
                Modifier.weight(1f).card(12.dp),
                verticalArrangement = Arrangement.spacedBy(2.dp),
            ) {
                Text(title, style = Type.caption, color = colors.textMuted, maxLines = 1)
                Text(value, style = Type.bodyStrong, color = colors.text, maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
        }
    }
}

/** The month as a grid of days, each as deep as its tokens. */
@Composable
private fun CalendarGrid(view: UsageMonthView, picked: String?, pick: (String) -> Unit) {
    val most = view.days.maxOfOrNull { it.inputTokens + it.outputTokens } ?: 0uL
    val leading = view.firstWeekday.toInt()
    val cells = leading + view.days.size
    val rows = (cells + 6) / 7

    Column(verticalArrangement = Arrangement.spacedBy(4.dp)) {
        Row(horizontalArrangement = Arrangement.spacedBy(4.dp)) {
            for (day in WEEKDAYS) {
                Text(day, style = Type.caption, color = colors.textMuted, textAlign = TextAlign.Center, modifier = Modifier.weight(1f))
            }
        }

        for (row in 0 until rows) {
            Row(horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                for (column in 0 until 7) {
                    val index = row * 7 + column - leading
                    val day = view.days.getOrNull(index)

                    if (day == null) {
                        Spacer(Modifier.weight(1f).aspectRatio(1f))
                    } else {
                        DayCell(day, most, picked == day.date, Modifier.weight(1f)) { pick(day.date) }
                    }
                }
            }
        }
    }
}

@Composable
private fun DayCell(day: UsageDayView, most: ULong, picked: Boolean, modifier: Modifier, onClick: () -> Unit) {
    val tokens = day.inputTokens + day.outputTokens

    // Spread on a logarithm: one heavy day would leave the rest pale.
    val depth = if (tokens == 0uL || most == 0uL) 0f else (ln(1.0 + tokens.toDouble()) / ln(1.0 + most.toDouble())).toFloat()

    val shape = RoundedCornerShape(8.dp)

    Box(
        modifier
            .aspectRatio(1f)
            .clip(shape)
            .background(if (depth == 0f) colors.sunken else colors.accent.copy(alpha = 0.15f + 0.75f * depth))
            .then(if (picked) Modifier.border(2.dp, colors.text, shape) else Modifier)
            .pressable(onClick = onClick),
        contentAlignment = Alignment.Center,
    ) {
        Text(
            day.date.takeLast(2).trimStart('0'),
            style = Type.caption,
            color = if (depth > 0.55f) colors.onAccent else colors.textSecondary,
        )
    }
}

@Composable
private fun DayDetail(day: UsageDayView) {
    Column(
        Modifier.fillMaxWidth().clip(Shapes.small).background(colors.sunken).padding(Space.m),
        verticalArrangement = Arrangement.spacedBy(2.dp),
    ) {
        Text(day.date, style = Type.subhead, color = colors.text)

        Text(
            "${compact(day.inputTokens)} in · ${compact(day.cachedTokens)} from cache · ${compact(day.outputTokens)} out",
            style = Type.footnote,
            color = colors.textSecondary,
        )

        Text(
            "${day.calls} calls · ${day.sessions} sessions · ${day.cost}",
            style = Type.footnote,
            color = colors.textSecondary,
        )
    }
}

/** Tokens in each hour of the day, over the month. */
@Composable
private fun Hours(view: UsageMonthView) {
    val tint = colors.accent
    val peak = view.peakHour?.toInt()
    val most = view.hours.maxOrNull()?.takeIf { it > 0uL } ?: 1uL

    Column(Modifier.fillMaxWidth().card(), verticalArrangement = Arrangement.spacedBy(Space.s)) {
        SectionHeading("By hour of the day")

        Canvas(Modifier.fillMaxWidth().height(80.dp)) {
            val step = size.width / 24
            val bar = step * 0.7f

            view.hours.forEachIndexed { hour, tokens ->
                val tall = (tokens.toDouble() / most.toDouble()).toFloat() * size.height

                drawRoundRect(
                    tint.copy(alpha = if (hour == peak) 1f else 0.5f),
                    topLeft = Offset(hour * step + (step - bar) / 2, size.height - max(tall, 1f)),
                    size = Size(bar, max(tall, 1f)),
                    cornerRadius = androidx.compose.ui.geometry.CornerRadius(2.dp.toPx()),
                )
            }
        }

        // Each label stands at the start of its quarter of the day, under its hour's bar.
        Row {
            for (label in listOf("00:00", "06:00", "12:00", "18:00")) {
                Text(label, style = Type.caption, color = colors.textMuted, modifier = Modifier.weight(1f))
            }
        }
    }
}

@Composable
private fun Models(view: UsageMonthView) {
    Column(Modifier.fillMaxWidth().card(), verticalArrangement = Arrangement.spacedBy(Space.s)) {
        SectionHeading("Models")

        for (model in view.models) {
            Column(verticalArrangement = Arrangement.spacedBy(3.dp)) {
                Line {
                    Text(model.model, style = Type.footnote, color = colors.text, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
                    Text("${compact(model.tokens)} · ${model.cost}", style = Type.caption, color = colors.textSecondary)
                }

                Share(model.share.toFloat())
            }
        }
    }
}

@Composable
private fun Share(share: Float) {
    Box(Modifier.fillMaxWidth().height(6.dp).clip(Shapes.pill).background(colors.sunken)) {
        Box(Modifier.fillMaxWidth(share.coerceIn(0f, 1f)).height(6.dp).clip(Shapes.pill).background(colors.accent))
    }
}

@Composable
private fun Sessions(view: UsageMonthView) {
    var all by rememberSaveable { mutableStateOf(false) }

    val shown = if (all) view.recent else view.recent.take(8)
    val format = DateFormat.getDateTimeInstance(DateFormat.SHORT, DateFormat.SHORT)

    Column(Modifier.fillMaxWidth().card(), verticalArrangement = Arrangement.spacedBy(Space.s)) {
        SectionHeading("Sessions")

        for (session in shown) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(Space.s)) {
                Column(Modifier.weight(1f)) {
                    Text(session.title, style = Type.footnote, color = colors.text, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    Text(
                        "${format.format(Date(session.lastMs.toLong()))} · ${session.model}",
                        style = Type.caption,
                        color = colors.textMuted,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                }

                Column(horizontalAlignment = Alignment.End) {
                    Text(compact(session.tokens), style = Type.footnote, color = colors.text)
                    Text(session.cost, style = Type.caption, color = colors.textMuted)
                }
            }
        }

        if (view.recent.size > shown.size || all) {
            Text(
                if (all) "Show fewer" else "Show all ${view.recent.size}",
                style = Type.footnote,
                color = colors.accent,
                modifier = Modifier.pressable { all = !all },
            )
        }
    }
}

@Composable
private fun Effect(folder: String) {
    val shown by produceState<EffectView?>(null, folder) {
        while (true) {
            value = withContext(Dispatchers.IO) { effectReport(folder, utcOffset()) }

            delay(REFRESH_MS)
        }
    }

    val view = shown ?: run {
        Text("Reading the records…", style = Type.callout, color = colors.textMuted)
        return
    }

    Column(Modifier.fillMaxWidth().card(), verticalArrangement = Arrangement.spacedBy(Space.xs)) {
        Text("How this is measured", style = Type.bodyStrong, color = colors.text)

        Text(
            "In a share of runs, chosen at random, each adviser (the dendritic network, the intuition, the mind) " +
                "still watches and learns but what it would say is not shown to the model. Runs it spoke in are " +
                "compared with runs it was held out of. A verdict needs ${view.minPerArm} runs on each side.",
            style = Type.footnote,
            color = colors.textSecondary,
        )

        Text(
            "${runs(view.runs)} recorded · ${view.runsInExperiment} in the comparison",
            style = Type.footnote,
            color = colors.textMuted,
        )

        if (view.comparisons > 1u) {
            Text(
                "${view.comparisons} comparisons have a verdict. About one in twenty shows a difference by chance.",
                style = Type.caption,
                color = colors.textMuted,
            )
        }
    }

    if (view.runs == 0uL) {
        EmptyState(
            Icons.Rounded.Insights,
            "No runs yet",
            "Turn on the dendritic network or the intuition in a chat's capabilities, and give the agent work.",
        )

        return
    }

    Adviser(view.overall, "All advisers", view.minPerArm)

    for (adviser in view.advisers) {
        Adviser(adviser, adviser.name, view.minPerArm)
    }
}

@Composable
private fun Adviser(adviser: AdviserView, title: String, needed: ULong) {
    Column(Modifier.fillMaxWidth().card(), verticalArrangement = Arrangement.spacedBy(Space.s)) {
        Text(title, style = Type.bodyStrong, color = colors.text)

        Text(
            "spoke in ${adviser.runsSpoke} · held out of ${adviser.runsHeldOut} · nothing to say in ${adviser.runsNothingToSay}",
            style = Type.caption,
            color = colors.textMuted,
        )

        // One scale for every metric of the same kind in the card, so that they compare.
        val reach = adviser.metrics
            .filter { it.rate }
            .flatMap { listOfNotNull(it.low, it.high, it.difference) }
            .maxOfOrNull { abs(it) }
            ?.coerceAtLeast(0.1) ?: 0.25

        for (metric in adviser.metrics) {
            Metric(metric, reach, needed)
        }

        if (adviser.weeks.any { it.withRate != null || it.withoutRate != null }) {
            Weeks(adviser.weeks)
        }

        if (adviser.comparableObservational > 0uL) {
            Text(
                "Its advice named something checkable ${adviser.comparableObservational} times, and the model did it next " +
                    "${adviser.followedObservational} times. That is followed, not helped: it says nothing of cause.",
                style = Type.caption,
                color = colors.textMuted,
            )
        }
    }
}

/** One metric: the two means, and the difference with its interval against no difference. */
@Composable
private fun Metric(metric: MetricView, reach: Double, needed: ULong) {
    val (word, tint) = when (metric.verdict) {
        VerdictView.HELPS -> "helps" to colors.good
        VerdictView.HURTS -> "hurts" to colors.bad
        VerdictView.NO_DIFFERENCE_SEEN -> "no difference seen" to colors.textSecondary
        VerdictView.TOO_FEW_RUNS -> "too few runs" to colors.textMuted
    }

    fun shown(value: Double?) = when {
        value == null -> "–"
        metric.rate -> "%.0f%%".format(value * 100)
        abs(value) >= 1000 -> compact(abs(value).toULong()).let { if (value < 0) "-$it" else it }
        else -> "%.1f".format(value)
    }

    val zero = colors.border
    val scale = if (metric.rate) reach else listOfNotNull(metric.low, metric.high, metric.difference).maxOfOrNull { abs(it) }?.coerceAtLeast(0.5) ?: 1.0

    Column(verticalArrangement = Arrangement.spacedBy(3.dp)) {
        Line {
            Text(metric.name, style = Type.footnote, color = colors.text, modifier = Modifier.weight(1f))

            Text(
                word,
                style = Type.caption,
                color = tint,
                modifier = Modifier.clip(Shapes.pill).background(tint.copy(alpha = 0.12f)).padding(horizontal = 8.dp, vertical = 2.dp),
            )
        }

        Text(
            "with ${shown(metric.withMean)} (${metric.withRuns}) · without ${shown(metric.withoutMean)} (${metric.withoutRuns})" +
                if (metric.verdict == VerdictView.TOO_FEW_RUNS) " · needs $needed each" else "",
            style = Type.caption,
            color = colors.textMuted,
        )

        val difference = metric.difference
        val low = metric.low
        val high = metric.high

        if (difference != null && low != null && high != null) {
            Canvas(Modifier.fillMaxWidth().height(14.dp)) {
                // Better is always to the right.
                val sign = if (metric.higherIsBetter) 1.0 else -1.0

                fun x(value: Double) = ((sign * value / scale).coerceIn(-1.0, 1.0) * 0.5 + 0.5).toFloat() * size.width

                val middle = size.height / 2

                drawLine(zero, Offset(x(0.0), 0f), Offset(x(0.0), size.height), strokeWidth = 2f)
                drawLine(tint, Offset(x(low), middle), Offset(x(high), middle), strokeWidth = 3f)
                drawCircle(tint, radius = 4.dp.toPx(), center = Offset(x(difference), middle))
            }
        }
    }
}

/** The share of runs whose checks passed, week by week, with and without. */
@Composable
private fun Weeks(weeks: List<WeekView>) {
    val with = colors.accent
    val without = colors.textMuted

    Column(verticalArrangement = Arrangement.spacedBy(Space.xs)) {
        Text("Checks passed, by week", style = Type.caption, color = colors.textSecondary)

        Canvas(Modifier.fillMaxWidth().height(70.dp)) {
            fun line(values: List<Double?>, color: androidx.compose.ui.graphics.Color) {
                val path = Path()
                var started = false

                values.forEachIndexed { index, value ->
                    if (value == null) return@forEachIndexed

                    val x = if (values.size == 1) size.width / 2 else index * size.width / (values.size - 1)
                    val y = (1 - value.toFloat()) * size.height

                    if (started) path.lineTo(x, y) else path.moveTo(x, y)
                    started = true

                    drawCircle(color, radius = 2.5.dp.toPx(), center = Offset(x, y))
                }

                drawPath(path, color, style = Stroke(width = 2.dp.toPx()))
            }

            line(weeks.map { it.withoutRate }, without)
            line(weeks.map { it.withRate }, with)
        }

        Line(gap = Space.m) {
            Line(gap = 6.dp) {
                Box(Modifier.size(8.dp).clip(Shapes.pill).background(with))
                Text("spoke", style = Type.caption, color = colors.textMuted)
            }

            Line(gap = 6.dp) {
                Box(Modifier.size(8.dp).clip(Shapes.pill).background(without))
                Text("held out", style = Type.caption, color = colors.textMuted)
            }

            Spacer(Modifier.width(0.dp).weight(1f))

            Text(weeks.first().week + " on", style = Type.caption, color = colors.textMuted)
        }
    }
}
