package dev.cortex.remote.ui

import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.gestures.detectTransformGestures
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
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
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.rounded.FileOpen
import androidx.compose.material.icons.rounded.Link
import androidx.compose.material.icons.rounded.Search
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.Icon
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.PathEffect
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.nativeCanvas
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import dev.cortex.remote.AppModel
import dev.cortex.remote.TradingDesk
import dev.cortex.remote.ui.settings.Field
import dev.cortex.remote.ui.settings.SmallButton
import uniffi.cortex_mobile.ChartView
import uniffi.cortex_mobile.ForecastRowView
import uniffi.cortex_mobile.MarketView
import uniffi.cortex_mobile.MetricsView
import uniffi.cortex_mobile.NetMetricsView
import uniffi.cortex_mobile.OutcomesView
import uniffi.cortex_mobile.StepView
import uniffi.cortex_mobile.marketTime
import uniffi.cortex_mobile.priceText
import kotlin.math.abs
import kotlin.math.max
import kotlin.math.min

private val BRAINS = listOf("LNN" to "Liquid", "DANN" to "Dendritic", "SNN" to "Spiking", "ENS" to "Ensemble", "LLM" to "Language model")
private val INTERVALS = listOf("1m", "5m", "15m", "25m", "1h", "1D")
private val TABS = listOf("Chart", "Forecast", "Outcomes", "Metrics")

private fun price(value: Double) = priceText(value)
private fun price(value: Double?) = value?.let(::priceText) ?: "—"
private fun pct(value: Double) = "%+.2f%%".format(value)
private fun share(value: Double?) = value?.let { "%.0f%%".format(it * 100) } ?: "—"

private fun quantity(value: Double): String = when {
    abs(value) >= 1e9 -> "%.2fB".format(value / 1e9)
    abs(value) >= 1e6 -> "%.2fM".format(value / 1e6)
    abs(value) >= 1e4 -> "%.1fK".format(value / 1e3)
    else -> "%.0f".format(value)
}

@Composable
private fun toneColor(tone: String): Color = when (tone) {
    "up", "good" -> colors.good
    "down", "bad" -> colors.bad
    "warn" -> colors.warning
    "accent" -> colors.accent
    else -> colors.textMuted
}

@Composable
private fun signColor(value: Double): Color = when {
    value > 0 -> colors.good
    value < 0 -> colors.bad
    else -> colors.textSecondary
}

/**
 * Live prices from Dhan, or bars from a file or chart picture, with the
 * next ten candles as the chosen network forecasts them, a plan for each
 * case, how earlier forecasts came out, and how every network measures up.
 */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun TradingScreen(model: AppModel, back: () -> Unit) {
    val desk = model.trading
    val context = LocalContext.current
    val view = desk.view

    var tab by rememberSaveable { mutableStateOf("Chart") }
    var connecting by remember { mutableStateOf(false) }
    var picking by remember { mutableStateOf(false) }

    val choose = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { file ->
        if (file != null) desk.load(context, file)
    }

    val import = { choose.launch(arrayOf("text/*", "application/csv", "text/csv", "image/*", "application/octet-stream")) }
    val connected = view?.connection == "connected"

    LaunchedEffect(Unit) { desk.refresh() }

    // The language-model forecast asks the phone's chat model: set up again when that changes.
    LaunchedEffect(model.account) { desk.useChatModel() }

    Column(Modifier.fillMaxSize().background(colors.background)) {
        TopBar(
            title = view?.title?.takeIf { it.isNotEmpty() } ?: "Trading",
            subtitle = view?.interval?.takeIf { view.source == "dhan" }?.let { "Dhan · $it" },
            back = back,
        ) {
            if (connected) BarButton(Icons.Rounded.Search, "Choose an instrument") { picking = true; desk.search("") }
            BarButton(Icons.Rounded.FileOpen, "Import a CSV or a chart picture", onClick = import)
            BarButton(Icons.Rounded.Link, "Dhan", tint = if (connected) colors.good else colors.text) { connecting = true }
        }

        if (view == null || view.source.isEmpty()) {
            Welcome(view, desk.problem, connect = { if (connected) { picking = true; desk.search("") } else connecting = true }, import = import)
        } else {
            Column(
                Modifier.fillMaxSize().verticalScroll(rememberScrollState()).navigationBarsPadding().padding(bottom = Space.xxl),
                verticalArrangement = Arrangement.spacedBy(Space.m),
            ) {
                PriceHeader(view)
                desk.problem?.let { Text(it, style = Type.footnote, color = colors.bad, modifier = Modifier.padding(horizontal = Gutter)) }
                BrainPicker(view) { desk.setBrain(it) }
                Chips(INTERVALS, view.interval, enabled = { view.source == "dhan" || it == view.interval }) { desk.setInterval(it) }
                Tabs(tab) { tab = it }

                Column(Modifier.padding(horizontal = Gutter), verticalArrangement = Arrangement.spacedBy(Space.m)) {
                    when (tab) {
                        "Chart" -> ChartTab(desk, view)
                        "Forecast" -> ForecastTab(view)
                        "Outcomes" -> OutcomesTab(desk.outcomes, view) { desk.clearOutcomes() }
                        "Metrics" -> MetricsTab(desk.metrics)
                    }
                }
            }
        }
    }

    if (connecting) {
        ModalBottomSheet(onDismissRequest = { connecting = false }, sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true), containerColor = colors.background) {
            ConnectSheet(desk, view) { connecting = false }
        }
    }

    if (picking) {
        ModalBottomSheet(onDismissRequest = { picking = false }, sheetState = rememberModalBottomSheetState(skipPartiallyExpanded = true), containerColor = colors.background) {
            PickerSheet(desk, view) { key -> desk.watch(key); picking = false; tab = "Chart" }
        }
    }
}

@Composable
private fun Welcome(view: MarketView?, problem: String?, connect: () -> Unit, import: () -> Unit) {
    Column(
        Modifier.fillMaxSize().padding(horizontal = Space.xl),
        verticalArrangement = Arrangement.Center,
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Text("Forecast the next ten candles", style = Type.title(24), color = colors.text, textAlign = TextAlign.Center)
        Spacer(Modifier.height(Space.xs))
        Text("Liquid, dendritic and spiking networks, or your chat model", style = Type.callout, color = colors.textSecondary, textAlign = TextAlign.Center)
        Spacer(Modifier.height(Space.xl))
        PrimaryButton(if (view?.connection == "connected") "Choose an instrument" else "Connect Dhan", onClick = connect)
        Spacer(Modifier.height(Space.s))
        SecondaryButton("Import a CSV or chart picture", onClick = import)

        val failed = view?.connectionDetail?.takeIf { view.connection == "failed" } ?: problem

        failed?.let {
            Spacer(Modifier.height(Space.m))
            Text(it, style = Type.footnote, color = colors.bad, textAlign = TextAlign.Center)
        }
    }
}

@Composable
private fun PriceHeader(view: MarketView) {
    val now = view.now

    Column(Modifier.padding(horizontal = Gutter)) {
        Row(verticalAlignment = Alignment.Bottom, horizontalArrangement = Arrangement.spacedBy(Space.s)) {
            Text(if (now.price > 0) price(now.price) else "—", style = Type.title(30), color = colors.text)

            val change = now.change
            val percent = now.changePct

            if (change != null && percent != null) {
                Text("%+.2f  %s".format(change, pct(percent)), style = Type.subhead, color = signColor(change), modifier = Modifier.padding(bottom = 4.dp))
            }

            Spacer(Modifier.weight(1f))

            view.status?.let { status ->
                Row(
                    Modifier.padding(bottom = 4.dp).clip(CircleShape).background(toneColor(status.tone).copy(alpha = 0.14f)).padding(horizontal = 10.dp, vertical = 4.dp),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(6.dp),
                ) {
                    Box(Modifier.size(7.dp).clip(CircleShape).background(toneColor(status.tone)))
                    Text(status.text, style = Type.caption, color = toneColor(status.tone), maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.width(IntrinsicMax))
                }
            }
        }

        view.status?.fraction?.let {
            Spacer(Modifier.height(Space.xs))
            LinearProgressIndicator(progress = { it }, color = colors.accent, trackColor = colors.sunken, modifier = Modifier.fillMaxWidth().clip(CircleShape))
        }
    }
}

private val IntrinsicMax = 150.dp

/** Which network forecasts: the one in use filled, with its record under. */
@Composable
private fun BrainPicker(view: MarketView, pick: (String) -> Unit) {
    Column(Modifier.fillMaxWidth()) {
        Row(
            Modifier.horizontalScroll(rememberScrollState()).padding(horizontal = Gutter),
            horizontalArrangement = Arrangement.spacedBy(Space.s),
        ) {
            for ((short, name) in BRAINS) {
                val on = view.brain == short

                Box(
                    Modifier
                        .heightIn(min = 36.dp)
                        .pressable { pick(short) }
                        .clip(CircleShape)
                        .background(if (on) colors.accent else colors.surface)
                        .border(1.dp, if (on) colors.accent else colors.border, CircleShape)
                        .padding(horizontal = 14.dp),
                    contentAlignment = Alignment.Center,
                ) {
                    Text(
                        if (on) "$short · $name" else short,
                        style = Type.footnote.copy(fontWeight = FontWeight.SemiBold),
                        color = if (on) colors.onAccent else colors.textSecondary,
                    )
                }
            }
        }

        Spacer(Modifier.height(Space.s))

        Column(
            Modifier.padding(horizontal = Gutter).fillMaxWidth().clip(Shapes.small).background(colors.accent.copy(alpha = 0.08f)).padding(horizontal = Space.m, vertical = Space.s),
        ) {
            val title = if (view.brain == "LLM") (view.llmModel ?: "No chat model set up") else "${view.brainName} network"

            Text(title, style = Type.subhead, color = colors.accent)
            Text(view.brainAbout, style = Type.caption, color = colors.textSecondary)

            if (view.brain != "LLM") {
                val held = view.heldOut

                Text(
                    "Held out  +1 ${share(held.getOrNull(0))} · +5 ${share(held.getOrNull(1))} · +10 ${share(held.getOrNull(2))}" +
                        if (view.liveJudged > 0u) "   Live ${view.liveRight} of ${view.liveJudged}" else "",
                    style = Type.caption,
                    color = colors.text,
                )
            }
        }
    }
}

@Composable
private fun Chips(options: List<String>, selected: String, enabled: (String) -> Boolean = { true }, pick: (String) -> Unit) {
    Row(Modifier.horizontalScroll(rememberScrollState()).padding(horizontal = Gutter), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        for (option in options) {
            val on = option == selected
            val usable = enabled(option)

            Box(
                Modifier
                    .heightIn(min = 32.dp)
                    .pressable(usable) { pick(option) }
                    .clip(Shapes.small)
                    .background(if (on) colors.raised else colors.background)
                    .padding(horizontal = 12.dp),
                contentAlignment = Alignment.Center,
            ) {
                Text(option, style = Type.footnote, color = if (on) colors.text else colors.textMuted)
            }
        }
    }
}

@Composable
private fun Tabs(selected: String, pick: (String) -> Unit) {
    Row(
        Modifier.padding(horizontal = Gutter).fillMaxWidth().clip(Shapes.small).background(colors.sunken).padding(3.dp),
        horizontalArrangement = Arrangement.spacedBy(2.dp),
    ) {
        for (tab in TABS) {
            val on = tab == selected

            Box(
                Modifier.weight(1f).heightIn(min = 36.dp).pressable { pick(tab) }.clip(Shapes.small).background(if (on) colors.surface else colors.sunken),
                contentAlignment = Alignment.Center,
            ) {
                Text(tab, style = Type.footnote.copy(fontWeight = if (on) FontWeight.SemiBold else FontWeight.Normal), color = if (on) colors.text else colors.textSecondary)
            }
        }
    }
}

// The chart ---------------------------------------------------------------

@Composable
private fun ChartTab(desk: TradingDesk, view: MarketView) {
    val chart = desk.chart

    if (chart != null && chart.candles.isNotEmpty()) {
        Chart(chart, view.rows, view.brain)
    }

    if (view.brain == "LLM") {
        Column(Modifier.card(), verticalArrangement = Arrangement.spacedBy(Space.xs)) {
            when {
                view.llmAsking -> Text("Asking ${view.llmModel ?: "the model"}…", style = Type.footnote, color = colors.textMuted)
                view.llmProblem != null -> Text(view.llmProblem!!, style = Type.footnote, color = colors.bad)
                view.llmReason.isNotEmpty() -> Text(view.llmReason, style = Type.footnote, color = colors.text)
            }

            SmallButton("Ask again") { desk.askModel() }
        }
    }

    view.plan?.let { plan ->
        Column(Modifier.card(), verticalArrangement = Arrangement.spacedBy(Space.xs)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(plan.bias, style = Type.title(20), color = toneColor(plan.tone), modifier = Modifier.weight(1f))
                Text("%+d".format(plan.score), style = Type.mono, color = colors.textMuted)
            }

            Text(plan.reasons.joinToString(" · "), style = Type.caption, color = colors.textSecondary)
            Pair("Support", price(plan.support)).let { StatRow(it.first, it.second, colors.good) }
            StatRow("Resistance", price(plan.resistance), colors.bad)
        }
    }

    val next = view.rows.firstOrNull()
    val last = view.rows.lastOrNull()

    if (next != null && last != null) {
        Column(Modifier.card(), verticalArrangement = Arrangement.spacedBy(Space.xs)) {
            Text("NEXT CANDLE", style = Type.heading, color = colors.textMuted)
            Text("${price(next.close)}  ${pct(next.changePct)}", style = Type.title(20), color = if (next.close >= next.open) colors.good else colors.bad)
            Text("O ${price(next.open)}  H ${price(next.high)}  L ${price(next.low)}", style = Type.caption, color = colors.textSecondary)
            StatRow("In ${last.step}", "${price(last.close)}  ${pct(last.changePct)}", signColor(last.changePct))
            StatRow("80% range", "${price(last.closeLow)} – ${price(last.closeHigh)}")
        }
    }

    NowCard(view)

    if (view.source == "picture") PictureScale(desk)

    for (note in view.notes) Text(note, style = Type.caption, color = colors.textMuted)
}

@Composable
private fun NowCard(view: MarketView) {
    val now = view.now

    Column(Modifier.card(), verticalArrangement = Arrangement.spacedBy(Space.xs)) {
        Text("NOW", style = Type.heading, color = colors.textMuted)
        StatRow("VWAP", price(now.vwap))
        StatRow("RSI 14", now.rsi?.let { "%.1f".format(it) } ?: "—", when { (now.rsi ?: 50.0) >= 70 -> colors.bad; (now.rsi ?: 50.0) <= 30 -> colors.good; else -> colors.text })
        StatRow("EMA 9 / 21", "${price(now.emaFast)} / ${price(now.emaSlow)}")
        StatRow("ATR 14", price(now.atr))
        now.volume?.let { StatRow("Volume", quantity(it)) }

        val high = now.dayHigh
        val low = now.dayLow

        if (high != null && low != null) StatRow("Day range", "${price(low)} – ${price(high)}")

        val bid = now.bid
        val ask = now.ask

        if (bid != null && ask != null) StatRow("Bid / ask", "${price(bid)} / ${price(ask)}")
        now.spread?.let { StatRow("Spread", "%.2f".format(it)) }

        now.buyShare?.let { buyers ->
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(Space.s)) {
                Text("Buyers", style = Type.footnote, color = colors.textSecondary)
                Box(Modifier.weight(1f).height(8.dp).clip(CircleShape).background(colors.bad.copy(alpha = 0.7f))) {
                    Box(Modifier.fillMaxWidth(buyers.toFloat()).height(8.dp).clip(CircleShape).background(colors.good))
                }
                Text(share(buyers), style = Type.footnote, color = colors.text)
            }
        }

        now.openInterest?.let { StatRow("Open interest", quantity(it)) }
    }
}

@Composable
private fun PictureScale(desk: TradingDesk) {
    var top by remember { mutableStateOf("") }
    var bottom by remember { mutableStateOf("") }

    Column(Modifier.card(), verticalArrangement = Arrangement.spacedBy(Space.s)) {
        Text("PICTURE'S PRICE SCALE", style = Type.heading, color = colors.textMuted)

        Row(horizontalArrangement = Arrangement.spacedBy(Space.s), verticalAlignment = Alignment.CenterVertically) {
            Field(top, "Highest", Modifier.weight(1f)) { top = it }
            Field(bottom, "Lowest", Modifier.weight(1f)) { bottom = it }

            val high = top.replace(",", "").toDoubleOrNull()
            val low = bottom.replace(",", "").toDoubleOrNull()

            SmallButton("Apply", enabled = high != null && low != null && high > low && low > 0) {
                desk.scalePicture(high!!, low!!)
            }
        }
    }
}

@Composable
private fun StatRow(name: String, value: String, tint: Color = colors.text) {
    Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
        Text(name, style = Type.footnote, color = colors.textSecondary, modifier = Modifier.weight(1f))
        Text(value, style = Type.footnote, color = tint)
    }
}

/**
 * Candles, the one forming, the forecast after them with its 80% band,
 * VWAP and the moving averages, support and resistance, volume, and RSI.
 * Pinched to zoom, dragged to move along.
 */
@Composable
private fun Chart(chart: ChartView, rows: List<ForecastRowView>, brain: String) {
    val candles = chart.candles
    val total = candles.size + max(rows.size, 0) - (if (candles.lastOrNull()?.forming == true && rows.isNotEmpty()) 1 else 0)
    val closedCount = candles.count { !it.forming }

    var shown by remember { mutableFloatStateOf(70f) }
    var offset by remember { mutableFloatStateOf(0f) }

    val good = colors.good
    val bad = colors.bad
    val accent = colors.accent
    val muted = colors.textMuted
    val border = colors.border
    val vwapColor = colors.warning
    val emaColor = colors.model
    val textSecondary = colors.textSecondary
    val ground = colors.sunken

    Column(
        Modifier
            .fillMaxWidth()
            .clip(Shapes.card)
            .background(ground.copy(alpha = 0.5f))
            .pointerInput(total) {
                detectTransformGestures { _, pan, zoom, _ ->
                    shown = (shown / zoom).coerceIn(20f, total.toFloat().coerceAtLeast(20f))
                    offset = (offset + pan.x / size.width * shown).coerceIn(0f, max(0f, total - shown))
                }
            }
            .padding(Space.s),
    ) {
        Row(horizontalArrangement = Arrangement.spacedBy(Space.m)) {
            Legend("VWAP", vwapColor)
            Legend("EMA 9", textSecondary)
            Legend("EMA 21", emaColor)
            Legend("$brain forecast", accent)
        }

        Spacer(Modifier.height(Space.xs))

        Canvas(Modifier.fillMaxWidth().height(300.dp)) {
            val count = min(shown, total.toFloat())
            val last = total - offset
            val first = (last - count).coerceAtLeast(0f)
            val axis = 58.dp.toPx()
            val plotWidth = size.width - axis
            val step = plotWidth / count
            val x = { at: Float -> (at - first + 0.5f) * step }

            // What is in view, for the price range.
            var high = Double.MIN_VALUE
            var low = Double.MAX_VALUE

            for (at in first.toInt() until last.toInt().coerceAtMost(total)) {
                candles.getOrNull(at)?.let { high = max(high, it.high); low = min(low, it.low) }
                rows.getOrNull(at - closedCount)?.let { high = max(high, max(it.high, it.closeHigh)); low = min(low, min(it.low, it.closeLow)) }
            }

            if (high <= low) return@Canvas

            val pad = (high - low) * 0.06
            high += pad
            low -= pad

            val volumeTop = size.height * 0.82f
            val y = { price: Double -> (size.height * ((high - price) / (high - low))).toFloat() }

            // Price lines.
            for (index in 0..4) {
                val level = low + (high - low) * index / 4
                val at = y(level)

                drawLine(border.copy(alpha = 0.4f), Offset(0f, at), Offset(plotWidth, at), 1f)
                drawContext.canvas.nativeCanvas.drawText(
                    priceText(level), plotWidth + 6.dp.toPx(), at + 4.dp.toPx(),
                    android.graphics.Paint().apply { color = android.graphics.Color.argb(170, 160, 160, 160); textSize = 10.dp.toPx(); isAntiAlias = true },
                )
            }

            // The forecast side.
            val aheadLeft = x(closedCount - 0.5f).coerceAtLeast(0f)

            if (aheadLeft < plotWidth) {
                drawRect(accent.copy(alpha = 0.06f), Offset(aheadLeft, 0f), Size(plotWidth - aheadLeft, size.height))
            }

            // Support and resistance.
            for ((level, tint) in listOf(chart.support to good, chart.resistance to bad)) {
                if (level == null || level <= low || level >= high) continue

                drawLine(tint.copy(alpha = 0.7f), Offset(0f, y(level)), Offset(plotWidth, y(level)), 1.5f, pathEffect = PathEffect.dashPathEffect(floatArrayOf(10f, 8f)))
            }

            // Volume.
            val loudest = (candles.maxOfOrNull { it.volume } ?: 0.0).coerceAtLeast(rows.maxOfOrNull { it.volume } ?: 0.0)
            val body = (step * 0.66f).coerceIn(1.5f, 22f)

            if (loudest > 0) {
                for (at in first.toInt() until last.toInt().coerceAtMost(total)) {
                    val candle = candles.getOrNull(at)
                    val row = rows.getOrNull(at - closedCount)
                    val (volume, tint) = when {
                        candle != null -> candle.volume to (if (candle.close >= candle.open) good else bad)
                        row != null -> row.volume to accent
                        else -> continue
                    }
                    val height = (volume / loudest).toFloat() * (size.height - volumeTop)

                    drawRect(tint.copy(alpha = 0.22f), Offset(x(at.toFloat()) - body / 2, size.height - height), Size(body, height))
                }
            }

            // The band.
            if (rows.size > 1) {
                val band = Path()

                rows.forEachIndexed { index, row ->
                    val px = x((closedCount + index).toFloat())
                    if (index == 0) band.moveTo(px, y(row.closeHigh)) else band.lineTo(px, y(row.closeHigh))
                }

                rows.indices.reversed().forEach { index -> band.lineTo(x((closedCount + index).toFloat()), y(rows[index].closeLow)) }
                band.close()

                drawPath(band, accent.copy(alpha = 0.12f))
            }

            // Indicator lines.
            fun line(pick: (uniffi.cortex_mobile.ChartCandle) -> Double?, tint: Color) {
                val path = Path()
                var started = false

                for (at in first.toInt() until last.toInt().coerceAtMost(closedCount)) {
                    val value = candles.getOrNull(at)?.let(pick) ?: continue
                    val point = Offset(x(at.toFloat()), y(value))

                    if (started) path.lineTo(point.x, point.y) else path.moveTo(point.x, point.y)
                    started = true
                }

                if (started) drawPath(path, tint, style = Stroke(1.5.dp.toPx()))
            }

            line({ it.emaSlow }, emaColor.copy(alpha = 0.8f))
            line({ it.emaFast }, textSecondary.copy(alpha = 0.8f))
            line({ it.vwap }, vwapColor)

            // Candles.
            for (at in first.toInt() until last.toInt().coerceAtMost(total)) {
                val middle = x(at.toFloat())

                candles.getOrNull(at)?.let { candle ->
                    val tint = (if (candle.close >= candle.open) good else bad).let { if (candle.forming) it.copy(alpha = 0.75f) else it }
                    val top = y(max(candle.open, candle.close))
                    val bottom = max(y(min(candle.open, candle.close)), top + 1f)

                    drawLine(tint, Offset(middle, y(candle.high)), Offset(middle, y(candle.low)), 1.2f)

                    if (candle.forming) {
                        drawRect(tint.copy(alpha = 0.3f), Offset(middle - body / 2, top), Size(body, bottom - top))
                        drawRect(tint, Offset(middle - body / 2, top), Size(body, bottom - top), style = Stroke(1.2f))
                    } else {
                        drawRect(tint, Offset(middle - body / 2, top), Size(body, bottom - top))
                    }
                }

                rows.getOrNull(at - closedCount)?.let { row ->
                    val top = y(max(row.open, row.close))
                    val bottom = max(y(min(row.open, row.close)), top + 1f)

                    drawLine(accent, Offset(middle, y(row.high)), Offset(middle, y(row.low)), 1.2f)
                    drawRect(accent.copy(alpha = if (row.close >= row.open) 0.2f else 0.55f), Offset(middle - body / 2, top), Size(body, bottom - top))
                    drawRect(accent, Offset(middle - body / 2, top), Size(body, bottom - top), style = Stroke(1.2f))
                }
            }

            // The latest price.
            candles.lastOrNull()?.let { latest ->
                val at = y(latest.close)
                val tint = if (latest.close >= latest.open) good else bad

                drawLine(tint.copy(alpha = 0.5f), Offset(0f, at), Offset(plotWidth, at), 1f, pathEffect = PathEffect.dashPathEffect(floatArrayOf(4f, 6f)))
                drawRoundRect(tint, Offset(plotWidth + 2.dp.toPx(), at - 8.dp.toPx()), Size(axis - 4.dp.toPx(), 16.dp.toPx()), androidx.compose.ui.geometry.CornerRadius(4.dp.toPx()))
                drawContext.canvas.nativeCanvas.drawText(
                    priceText(latest.close), plotWidth + 5.dp.toPx(), at + 4.dp.toPx(),
                    android.graphics.Paint().apply { color = android.graphics.Color.WHITE; textSize = 10.dp.toPx(); isAntiAlias = true; isFakeBoldText = true },
                )
            }
        }

        Spacer(Modifier.height(Space.s))

        // RSI.
        Canvas(Modifier.fillMaxWidth().height(56.dp)) {
            val count = min(shown, total.toFloat())
            val last = total - offset
            val first = (last - count).coerceAtLeast(0f)
            val plotWidth = size.width - 58.dp.toPx()
            val step = plotWidth / count
            val x = { at: Float -> (at - first + 0.5f) * step }
            val y = { value: Double -> (size.height * (1 - value / 100)).toFloat() }

            for (level in listOf(30.0, 70.0)) {
                drawLine(border, Offset(0f, y(level)), Offset(plotWidth, y(level)), 1f, pathEffect = PathEffect.dashPathEffect(floatArrayOf(6f, 6f)))
            }

            val path = Path()
            var started = false

            for (at in first.toInt() until last.toInt().coerceAtMost(closedCount)) {
                val value = candles.getOrNull(at)?.rsi ?: continue

                if (started) path.lineTo(x(at.toFloat()), y(value)) else path.moveTo(x(at.toFloat()), y(value))
                started = true
            }

            if (started) drawPath(path, emaColor, style = Stroke(1.5.dp.toPx()))

            drawContext.canvas.nativeCanvas.drawText(
                "RSI 14", 4.dp.toPx(), 12.dp.toPx(),
                android.graphics.Paint().apply { color = android.graphics.Color.argb(170, 160, 160, 160); textSize = 10.dp.toPx(); isAntiAlias = true },
            )
        }

        candles.firstOrNull()?.let { firstCandle ->
            Row(Modifier.fillMaxWidth().padding(top = 2.dp)) {
                val visibleFirst = candles.getOrNull(((total - offset) - min(shown, total.toFloat())).toInt().coerceIn(0, candles.size - 1)) ?: firstCandle
                Text(marketTime(visibleFirst.timeMs, chart.intraday), style = Type.caption, color = muted, modifier = Modifier.weight(1f))
                Text(rows.lastOrNull()?.time ?: marketTime(candles.last().timeMs, chart.intraday), style = Type.caption, color = muted)
            }
        }
    }
}

@Composable
private fun Legend(name: String, tint: Color) {
    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(4.dp)) {
        Box(Modifier.size(width = 10.dp, height = 2.dp).background(tint))
        Text(name, style = Type.caption, color = colors.textMuted)
    }
}

// Tables ------------------------------------------------------------------

/** A table that scrolls sideways: a header and striped rows. */
@Composable
private fun Table(widths: List<Dp>, header: List<String>, rows: List<List<Pair<String, Color>>>, strongFirst: Boolean = true) {
    Column(Modifier.fillMaxWidth().horizontalScroll(rememberScrollState())) {
        Row(Modifier.padding(vertical = 6.dp)) {
            header.forEachIndexed { index, name ->
                Text(
                    name, style = Type.caption.copy(fontWeight = FontWeight.SemiBold), color = colors.textMuted,
                    textAlign = if (index == 0) TextAlign.Start else TextAlign.End,
                    modifier = Modifier.width(widths[index]).padding(horizontal = 6.dp),
                )
            }
        }

        rows.forEachIndexed { rowIndex, cells ->
            Row(
                Modifier.clip(RoundedCornerShape(6.dp)).background(if (rowIndex % 2 == 0) colors.surface else colors.background).padding(vertical = 8.dp),
            ) {
                cells.forEachIndexed { index, (text, tint) ->
                    Text(
                        text,
                        style = if (index == 0 && strongFirst) Type.footnote.copy(fontWeight = FontWeight.SemiBold) else Type.footnote,
                        color = tint,
                        textAlign = if (index == 0) TextAlign.Start else TextAlign.End,
                        maxLines = 1,
                        modifier = Modifier.width(widths[index]).padding(horizontal = 6.dp),
                    )
                }
            }
        }
    }
}

@Composable
private fun ForecastTab(view: MarketView) {
    val using = if (view.brain == "LLM") "LLM · ${view.llmModel ?: "not set up"}" else "${view.brain} · ${view.brainName} network"

    SectionTitle("Next 10 candles", using)

    if (view.rows.isEmpty()) {
        Text(
            if (view.brain == "LLM") "Ask the model from the Chart tab." else "The networks are training, or there are too few candles.",
            style = Type.footnote, color = colors.textMuted,
        )
    } else {
        val text = colors.text
        val secondary = colors.textSecondary
        val volume = view.rows.any { it.volume > 0 }

        Table(
            widths = listOf(84.dp, 78.dp, 64.dp, 76.dp, 76.dp, 140.dp, 64.dp, 48.dp, 78.dp, 76.dp),
            header = listOf("Candle", "Close", "Chg", "High", "Low", "Close 80%", "Volume", "RSI", "VWAP", "Open"),
            rows = view.rows.map { row ->
                listOf(
                    "+${row.step} ${row.time}" to text,
                    price(row.close) to (if (row.close >= row.open) colors.good else colors.bad),
                    pct(row.changePct) to signColor(row.changePct),
                    price(row.high) to text,
                    price(row.low) to text,
                    "${price(row.closeLow)} – ${price(row.closeHigh)}" to secondary,
                    (if (volume) quantity(row.volume) else "—") to text,
                    (row.rsi?.let { "%.1f".format(it) } ?: "—") to text,
                    price(row.vwap) to text,
                    price(row.open) to text,
                )
            },
        )
    }

    val plan = view.plan ?: return

    SectionTitle("Plan", null)

    Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(Space.s)) {
        Text(
            plan.bias, style = Type.subhead, color = toneColor(plan.tone),
            modifier = Modifier.clip(CircleShape).background(toneColor(plan.tone).copy(alpha = 0.14f)).padding(horizontal = 10.dp, vertical = 4.dp),
        )
        Text(plan.reasons.joinToString(" · "), style = Type.caption, color = colors.textSecondary)
    }

    for (scenario in plan.scenarios) {
        val tint = toneColor(scenario.tone)
        val mark = when (scenario.tone) { "up" -> "▲"; "down" -> "▼"; else -> "■" }

        Column(
            Modifier.fillMaxWidth().clip(Shapes.small).background(colors.surface).border(1.dp, colors.border, Shapes.small).padding(Space.m),
            verticalArrangement = Arrangement.spacedBy(2.dp),
        ) {
            Text("$mark ${scenario.case}", style = Type.subhead, color = tint)
            Text(scenario.expect, style = Type.footnote, color = colors.textSecondary)
            Text(scenario.action, style = Type.footnote, color = colors.text)

            if (scenario.entry != null || scenario.target != null || scenario.stop != null) {
                Row(horizontalArrangement = Arrangement.spacedBy(Space.m), modifier = Modifier.padding(top = 2.dp)) {
                    scenario.entry?.let { Text("At ${price(it)}", style = Type.caption, color = colors.text) }
                    scenario.target?.let { Text("Target ${price(it)}", style = Type.caption, color = colors.good) }
                    scenario.stop?.let { Text("Stop ${price(it)}", style = Type.caption, color = colors.bad) }
                }
            }
        }
    }

    Text("Rules applied to the forecast and levels. Not advice; see Outcomes.", style = Type.caption, color = colors.textMuted)
}

@Composable
private fun SectionTitle(title: String, detail: String?) {
    Row(verticalAlignment = Alignment.Bottom, horizontalArrangement = Arrangement.spacedBy(Space.s), modifier = Modifier.padding(top = Space.xs)) {
        Text(title, style = Type.bodyStrong, color = colors.text)
        detail?.let { Text(it, style = Type.caption, color = colors.textMuted, modifier = Modifier.padding(bottom = 2.dp)) }
    }
}

@Composable
private fun OutcomesTab(outcomes: OutcomesView?, view: MarketView, clear: () -> Unit) {
    SectionTitle("How the forecasts came out", null)

    if (outcomes == null || !outcomes.live) {
        Text("Recorded for live Dhan instruments: every closed candle's forecasts are checked when their candles close.", style = Type.footnote, color = colors.textMuted)
        return
    }

    if (outcomes.tallies.isEmpty()) {
        Text("Nothing recorded yet: forecasts are written down as candles close.", style = Type.footnote, color = colors.textMuted)
        return
    }

    val text = colors.text

    @Composable
    fun record(right: Double?, judged: UInt): Pair<String, Color> =
        if (right == null) "—" to colors.textMuted
        else "${share(right)} of $judged" to when { right > 0.55 -> colors.good; right < 0.45 -> colors.bad; else -> text }

    Table(
        widths = listOf(110.dp, 86.dp, 86.dp, 86.dp, 64.dp, 64.dp, 60.dp),
        header = listOf("Network", "Right +1", "+5", "+10", "In band", "Error", "Waiting"),
        rows = outcomes.tallies.map { tally ->
            listOf(
                "${tally.brain} ${tally.name}" to (if (tally.selected) colors.accent else text),
                record(tally.right.getOrNull(0), tally.judged.getOrNull(0) ?: 0u),
                record(tally.right.getOrNull(1), tally.judged.getOrNull(1) ?: 0u),
                record(tally.right.getOrNull(2), tally.judged.getOrNull(2) ?: 0u),
                share(tally.inside) to text,
                (tally.errorPct?.let { "%.2f%%".format(it) } ?: "—") to text,
                tally.waiting.toString() to colors.textMuted,
            )
        },
    )

    SectionTitle("Each forecast", "newest first")

    Table(
        widths = listOf(54.dp, 52.dp, 92.dp, 84.dp, 96.dp, 84.dp, 64.dp, 76.dp),
        header = listOf("Made", "By", "For", "From", "Forecast", "Actual", "Error", "Result"),
        strongFirst = false,
        rows = outcomes.outcomes.take(120).map { outcome ->
            val (result, tint) = when (outcome.result) {
                "right" -> "✓ right" to colors.good
                "wrong" -> "✗ wrong" to colors.bad
                "flat" -> "flat" to colors.textSecondary
                else -> "waiting" to colors.textMuted
            }
            val arrow = when { outcome.forecast > outcome.from -> "▲"; outcome.forecast < outcome.from -> "▼"; else -> "■" }

            listOf(
                outcome.made to text,
                outcome.brain to text,
                "+${outcome.step} → ${outcome.due}" to text,
                price(outcome.from) to text,
                "$arrow ${price(outcome.forecast)}" to signColor(outcome.forecast - outcome.from),
                price(outcome.actual) to text,
                (outcome.errorPct?.let { "%.2f%%".format(it) } ?: "—") to text,
                result to tint,
            )
        },
    )

    SmallButton("Clear this instrument's record", onClick = clear)
}

@Composable
private fun MetricsTab(metrics: MetricsView?) {
    if (metrics == null || !metrics.trained) {
        SectionTitle("Metrics", null)
        Text("The networks have not been trained on this series yet.", style = Type.footnote, color = colors.textMuted)
    } else {
        SectionTitle("Held-out metrics", "${metrics.trainedOn} · ${metrics.bars} candles, last ${metrics.heldOut} held out")

        val text = colors.text

        @Composable
        fun accuracy(step: StepView?): Pair<String, Color> {
            val value = step?.accuracy ?: return "—" to colors.textMuted
            val low = step.low ?: 0.0
            val high = step.high ?: 1.0

            return share(value) to when { low > 0.5 -> colors.good; high < 0.5 -> colors.bad; else -> text }
        }

        val rows = metrics.nets.map { net ->
            listOf(
                "${net.brain} ${net.name}" to (if (net.selected) colors.accent else text),
                accuracy(net.steps.getOrNull(0)),
                accuracy(net.steps.getOrNull(1)),
                accuracy(net.steps.getOrNull(2)),
                (net.errorPct?.let { "%.3f%%".format(it) } ?: "—") to text,
                (net.skill?.let { "%+.3f".format(it) } ?: "—") to signColor(net.skill ?: 0.0),
                share(net.bandHit) to text,
            )
        } + listOf(
            listOf(
                "Same as last" to colors.textMuted,
                accuracy(metrics.baseline.getOrNull(0)).first to colors.textMuted,
                accuracy(metrics.baseline.getOrNull(1)).first to colors.textMuted,
                accuracy(metrics.baseline.getOrNull(2)).first to colors.textMuted,
                "—" to colors.textMuted, "0" to colors.textMuted, "—" to colors.textMuted,
            ),
        )

        Table(
            widths = listOf(118.dp, 60.dp, 60.dp, 60.dp, 72.dp, 64.dp, 64.dp),
            header = listOf("Network", "Dir +1", "+5", "+10", "Error +1", "Skill", "In band"),
            rows = rows,
        )

        Text(
            "Direction: share called right on held-out candles; green only when its 99% range is above 50%. Skill above 0 beats predicting no change. An honest band holds about 80%.",
            style = Type.caption, color = colors.textMuted,
        )

        for (net in metrics.nets) NetCard(net)
    }

    if (metrics != null) {
        Column(Modifier.card(), verticalArrangement = Arrangement.spacedBy(Space.xs)) {
            Row(horizontalArrangement = Arrangement.spacedBy(Space.s)) {
                Text("LLM", style = Type.subhead, color = colors.accent)
                Text(metrics.llmModel ?: "No chat model set up", style = Type.subhead, color = colors.text)
            }

            StatRow("Asked", "${metrics.llmCalls} times, ${metrics.llmFailures} failed")
            if (metrics.llmCalls > 0u) StatRow("Mean answer", "%.1f s".format(metrics.llmMeanMs / 1000))
            StatRow(
                "Live, all instruments",
                if (metrics.llmLiveJudged > 0u) "${metrics.llmLiveRight} of ${metrics.llmLiveJudged} right" else "nothing checked yet",
            )
            Text("Judged on live outcomes only.", style = Type.caption, color = colors.textMuted)
        }
    }
}

@Composable
private fun NetCard(net: NetMetricsView) {
    val accent = colors.accent
    val sunken = colors.sunken
    val muted = colors.textMuted

    Column(Modifier.card(), verticalArrangement = Arrangement.spacedBy(Space.xs)) {
        Row(horizontalArrangement = Arrangement.spacedBy(Space.s)) {
            Text(net.brain, style = Type.subhead, color = colors.accent)
            Text(net.name, style = Type.subhead, color = colors.text)
        }

        Text(net.about, style = Type.caption, color = colors.textMuted)

        for (step in net.steps) {
            Row(verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(Space.s)) {
                Text("+${step.step}", style = Type.caption, color = colors.textSecondary, modifier = Modifier.width(28.dp))

                Canvas(Modifier.weight(1f).height(10.dp)) {
                    val at = { share: Double -> (((share - 0.3) / 0.4).coerceIn(0.0, 1.0) * size.width).toFloat() }

                    drawRoundRect(sunken, cornerRadius = androidx.compose.ui.geometry.CornerRadius(5f))

                    val low = step.low
                    val high = step.high

                    if (low != null && high != null) {
                        drawRoundRect(accent.copy(alpha = 0.3f), Offset(at(low), 0f), Size(at(high) - at(low), size.height), androidx.compose.ui.geometry.CornerRadius(5f))
                    }

                    drawLine(muted, Offset(at(0.5), -2f), Offset(at(0.5), size.height + 2f), 1.5f)
                    step.accuracy?.let { drawCircle(accent, 5.dp.toPx(), Offset(at(it), size.height / 2)) }
                }

                Text(share(step.accuracy), style = Type.caption, color = colors.text, modifier = Modifier.width(36.dp), textAlign = TextAlign.End)
            }
        }

        if (net.parameters > 0u) StatRow("Trained weights", quantity(net.parameters.toDouble()))
        if (net.neurons > 0u) StatRow("Neurons", net.neurons.toString())
        if (net.brain != "ENS") {
            StatRow("Ensemble weight", share(net.weight.toDouble()))
            StatRow("Training", "%.0f ms".format(net.trainMs))
        }
        for (fact in net.facts) StatRow(fact.name, fact.value)

        if (net.lossFit.size > 1) {
            Canvas(Modifier.fillMaxWidth().height(36.dp).padding(top = 4.dp)) {
                val values = net.lossFit + net.lossCheck
                val low = values.min()
                val span = (values.max() - low).coerceAtLeast(1e-6f)

                drawRoundRect(sunken, cornerRadius = androidx.compose.ui.geometry.CornerRadius(6f))

                for ((series, tint) in listOf(net.lossCheck to accent.copy(alpha = 0.4f), net.lossFit to accent)) {
                    val path = Path()

                    series.forEachIndexed { index, value ->
                        val px = 4f + index / (series.size - 1f) * (size.width - 8f)
                        val py = size.height - 4f - (value - low) / span * (size.height - 8f)

                        if (index == 0) path.moveTo(px, py) else path.lineTo(px, py)
                    }

                    drawPath(path, tint, style = Stroke(1.5.dp.toPx()))
                }
            }
        }
    }
}

// Sheets ------------------------------------------------------------------

@Composable
private fun ConnectSheet(desk: TradingDesk, view: MarketView?, done: () -> Unit) {
    var clientId by remember { mutableStateOf(desk.desk.clientId() ?: "") }
    var token by remember { mutableStateOf("") }

    Column(
        Modifier.fillMaxWidth().padding(horizontal = Gutter).padding(bottom = Space.xxl).imePadding(),
        verticalArrangement = Arrangement.spacedBy(Space.s),
    ) {
        Text("Dhan", style = Type.title(22), color = colors.text)
        Text("web.dhan.co → My Profile → Access DhanHQ APIs. Tokens last a day; data needs Dhan's data plan.", style = Type.footnote, color = colors.textMuted)

        Field(clientId, "Client ID") { clientId = it }
        Field(token, "Access token", secret = true) { token = it }

        when (view?.connection) {
            "checking" -> Text("Checking…", style = Type.footnote, color = colors.textMuted)
            "connected" -> Text("Connected as ${view.connectionDetail}", style = Type.footnote, color = colors.good)
            "failed" -> Text(view.connectionDetail, style = Type.footnote, color = colors.bad)
        }

        Row(horizontalArrangement = Arrangement.spacedBy(Space.s)) {
            SmallButton("Connect", strong = true, enabled = clientId.isNotBlank() && token.isNotBlank()) {
                desk.connect(clientId, token)
                token = ""
            }

            if (view?.connection == "connected") SmallButton("Disconnect") { desk.disconnect() }

            Spacer(Modifier.weight(1f))
            SmallButton("Done", onClick = done)
        }
    }
}

@Composable
private fun PickerSheet(desk: TradingDesk, view: MarketView?, pick: (String) -> Unit) {
    var query by remember { mutableStateOf("") }

    Column(Modifier.fillMaxWidth().padding(horizontal = Gutter).imePadding(), verticalArrangement = Arrangement.spacedBy(Space.s)) {
        Field(query, "Search: RELIANCE, NIFTY, TCS…") {
            query = it
            desk.search(it)
        }

        LazyColumn(Modifier.fillMaxWidth().heightIn(max = 420.dp)) {
            items(desk.instruments, key = { it.key }) { instrument ->
                Row(
                    Modifier.fillMaxWidth().pressable { pick(instrument.key) }.padding(vertical = 10.dp),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(Space.s),
                ) {
                    Text(instrument.symbol, style = Type.subhead, color = colors.text)
                    Text(instrument.name, style = Type.caption, color = colors.textMuted, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
                    Text(instrument.exchange, style = Type.caption, color = colors.textMuted)
                }
            }
        }

        if (view?.hasInstrumentList == false) {
            SmallButton("Load Dhan's full list") { desk.fetchInstrumentList() }
        }

        Spacer(Modifier.height(Space.xl))
    }
}
