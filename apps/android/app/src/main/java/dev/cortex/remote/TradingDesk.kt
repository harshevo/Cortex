package dev.cortex.remote

import android.content.Context
import android.net.Uri
import android.os.Handler
import android.os.Looper
import android.provider.OpenableColumns
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import uniffi.cortex_mobile.ChartView
import uniffi.cortex_mobile.InstrumentView
import uniffi.cortex_mobile.MarketDesk
import uniffi.cortex_mobile.MarketListener
import uniffi.cortex_mobile.MarketView
import uniffi.cortex_mobile.MetricsView
import uniffi.cortex_mobile.OutcomesView
import java.util.concurrent.Executors
import java.util.concurrent.RejectedExecutionException
import java.util.concurrent.atomic.AtomicBoolean

/** The largest file read: far more than a phone draws. */
private const val MOST_BYTES = 32L * 1024 * 1024

/** Candles fetched for the chart. */
private const val CHART_CANDLES = 240u

/**
 * The Trading screen's market, kept for as long as the app is: the live
 * feed and training go on while the screen is closed.
 *
 * Dhan, the networks and the forecasts run in the core. Fetching what
 * they hold happens on a thread of this one, and what the screen draws is
 * written on the main thread only.
 */
@Stable
class TradingDesk(folder: String) {
    private val main = Handler(Looper.getMainLooper())

    private val worker = Executors.newSingleThreadExecutor { work ->
        Thread(work, "cortex-trading").apply { isDaemon = true }
    }

    @Volatile
    private var closed = false

    /** A fetch is waiting to run; more changes meanwhile join it. */
    private val pending = AtomicBoolean(false)

    var view: MarketView? by mutableStateOf(null)
        private set

    var chart: ChartView? by mutableStateOf(null)
        private set

    var outcomes: OutcomesView? by mutableStateOf(null)
        private set

    var metrics: MetricsView? by mutableStateOf(null)
        private set

    var instruments: List<InstrumentView> by mutableStateOf(emptyList())
        private set

    /** Why the last file could not be read. */
    var problem: String? by mutableStateOf(null)
        private set

    /** A picture loaded, kept to read again with a price scale. */
    var picture: Pair<String, ByteArray>? = null
        private set

    private val ears = object : MarketListener {
        // Called from the core's own threads.
        override fun changed() = refresh()
    }

    /** Made after the listener: the core may call it while it is being made, and is then asked again. */
    lateinit var desk: MarketDesk
        private set

    init {
        desk = MarketDesk(folder, ears)
        refresh()
    }

    fun refresh() {
        if (!::desk.isInitialized) return

        if (pending.compareAndSet(false, true)) run(::fetch)
    }

    private fun fetch() {
        pending.set(false)

        val view = desk.view()
        val chart = desk.chart(CHART_CANDLES)
        val outcomes = desk.outcomes()
        val metrics = desk.metrics()

        main.post {
            if (closed) return@post

            this.view = view
            this.chart = chart
            this.outcomes = outcomes
            this.metrics = metrics
        }
    }

    fun search(query: String) = run {
        val found = desk.search(query)

        main.post { instruments = found }
    }

    fun watch(key: String) = run { desk.watch(key) }

    fun connect(clientId: String, token: String) = run { desk.connect(clientId, token) }

    fun disconnect() = run { desk.disconnect() }

    fun setBrain(brain: String) = run { desk.setBrain(brain) }

    fun setInterval(interval: String) = run { desk.setInterval(interval) }

    fun askModel() = run { desk.askLlm() }

    fun fetchInstrumentList() = run { desk.fetchInstrumentList() }

    fun clearOutcomes() = run { desk.clearOutcomes() }

    /** After the phone's chat model is changed. */
    fun useChatModel() = run { desk.usePhoneModel() }

    /** Reads a file the person chose: bars as CSV, or a chart picture. */
    fun load(context: Context, file: Uri) {
        val resolver = context.applicationContext.contentResolver

        problem = null

        run {
            val (name, size) = resolver.query(file, arrayOf(OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE), null, null, null)
                ?.use { cursor ->
                    if (!cursor.moveToFirst()) return@use null to null

                    cursor.getString(0) to cursor.getLong(1).takeIf { !cursor.isNull(1) }
                } ?: (null to null)

            val type = resolver.getType(file) ?: ""
            val shown = name ?: "bars.csv"

            val failure = when {
                size != null && size > MOST_BYTES -> "The file is larger than ${MOST_BYTES / 1024 / 1024} MB."

                else -> try {
                    val bytes = resolver.openInputStream(file)?.use { it.readBytes() }

                    when {
                        bytes == null -> "The file could not be opened."

                        type.startsWith("image/") || shown.substringAfterLast('.').lowercase() in setOf("png", "jpg", "jpeg", "webp") -> {
                            picture = shown to bytes
                            desk.loadPicture(shown, bytes, null, null)
                        }

                        else -> desk.loadCsv(shown, String(bytes))
                    }
                } catch (error: Exception) {
                    error.message ?: "The file could not be read."
                }
            }

            main.post { problem = failure }
        }
    }

    /** Reads the picture again with its highest and lowest price. */
    fun scalePicture(top: Double, bottom: Double) {
        val (name, bytes) = picture ?: return

        run { desk.loadPicture(name, bytes, top, bottom) }
    }

    private fun run(work: () -> Unit) {
        if (closed) return

        try {
            worker.execute(work)
        } catch (_: RejectedExecutionException) {
            // Closed meanwhile.
        }
    }

    fun close() {
        closed = true

        worker.execute { desk.close() }
        worker.shutdown()
    }
}
