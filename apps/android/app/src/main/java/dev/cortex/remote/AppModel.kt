package dev.cortex.remote

import android.app.Application
import android.os.Build
import android.os.Handler
import android.os.Looper
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.lifecycle.AndroidViewModel
import uniffi.cortex_mobile.AccountView
import uniffi.cortex_mobile.AppState
import uniffi.cortex_mobile.Change
import uniffi.cortex_mobile.ChatView
import uniffi.cortex_mobile.Connection
import uniffi.cortex_mobile.Cortex
import uniffi.cortex_mobile.Listener
import uniffi.cortex_mobile.MemoryView
import uniffi.cortex_mobile.SettingsView
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.CountDownLatch
import java.util.concurrent.Executors
import java.util.concurrent.RejectedExecutionException

/**
 * Everything the screens show, kept in step with the core.
 *
 * The core holds the truth and says when a part of it has changed. That
 * part is fetched on a thread of its own, which also reads the Markdown
 * of what it fetched, and handed to the main thread ready to draw: the
 * main thread never waits for the core.
 */
class AppModel(application: Application) : AndroidViewModel(application) {
    private val main = Handler(Looper.getMainLooper())

    /** The one thread that asks the core what changed. */
    private val fetcher = Executors.newSingleThreadExecutor { work ->
        Thread(work, "cortex-fetch").apply { isDaemon = true }
    }

    /** Changes heard and not yet fetched. A burst of them is fetched once. */
    private val heard = HashSet<Change>()

    @Volatile
    private var closed = false

    // What the fetching thread uses is made before the core, which may
    // say something has changed before this constructor has finished.

    private val feeds = HashMap<String, ChatFeed>()

    /** The chats that have a feed: the only ones worth fetching for. */
    private val watched = ConcurrentHashMap.newKeySet<String>()

    /** Used on the fetching thread only. */
    private val followers = HashMap<String, ChatFollower>()

    /** Opened once this is fully made: the fetching thread waits for it. */
    private val made = CountDownLatch(1)

    private val ears = object : Listener {
        // Called from one of the core's own threads.
        override fun changed(changes: List<Change>) {
            if (closed) return

            val idle = synchronized(heard) {
                val idle = heard.isEmpty()

                heard.addAll(changes)

                idle
            }

            if (idle) {
                try {
                    fetcher.execute(::fetch)
                } catch (_: RejectedExecutionException) {
                    // The app closed between the two lines above.
                }
            }
        }

        // The ChatGPT sign-in page, for the activity to show.
        override fun openAddress(address: String) {
            main.post {
                signInAddress = address
                lastSignInAddress = address
            }
        }
    }

    /** The folder the core keeps everything in, records of use among it. */
    val folder: String = RestWorker.folder(application)

    val core: Cortex = Cortex(
        folder,
        Build.MODEL ?: "Android phone",
        ears,
    )

    /** The theme and the size of the text. */
    val look = Look(application)

    private val tradingDesk = lazy { TradingDesk(folder) }

    /** The market on the Trading screen. Made when that screen first opens. */
    val trading: TradingDesk by tradingDesk

    /** The pictures sent with instructions, made small and kept for a while. */
    val pictures = Pictures(core)

    var state: AppState by mutableStateOf(core.state())
        private set

    /** Who the model on this phone is reached as. */
    var account: AccountView by mutableStateOf(core.account())
        private set

    /** A page waiting to be shown, taken by the activity. */
    var signInAddress: String? by mutableStateOf(null)

    /** The sign-in page last shown, to show again. */
    var lastSignInAddress: String? by mutableStateOf(null)

    val settings = mutableStateMapOf<String, SettingsView>()
    val memory = mutableStateMapOf<String, MemoryView>()


    /** Which screen a rehearsal wants in front. Nothing else sets it. */
    var stage: String? by mutableStateOf(null)

    val connected: Boolean
        get() = state.connection is Connection.Connected

    fun chat(id: String): ChatView? = state.chats.firstOrNull { it.id == id }

    /** The conversation of a chat and what its run is doing. Called on the main thread. */
    fun feed(chat: String): ChatFeed = feeds.getOrPut(chat) {
        watched += chat

        hear(listOf(Change.Items(chat), Change.Live(chat)))

        ChatFeed()
    }

    /** Fetches a chat's memory, which is only asked for when its screen opens. */
    fun refreshMemory(chat: String) = hear(listOf(Change.Memory(chat)))

    private fun hear(changes: List<Change>) = ears.changed(changes)

    /** Asks the core for what changed and hands it to the main thread. Runs on the fetching thread. */
    private fun fetch() {
        made.await()

        val changes = synchronized(heard) { heard.toList().also { heard.clear() } }

        if (closed) return

        var state: AppState? = null
        var account: AccountView? = null

        val settings = HashMap<String, SettingsView>()
        val memory = HashMap<String, MemoryView>()

        val items = HashSet<String>()
        val live = HashSet<String>()

        for (change in changes) {
            when (change) {
                is Change.State -> {
                    val fresh = core.state()

                    state = fresh
                    account = core.account()

                    for (chat in fresh.chats) {
                        settings[chat.id] = core.settings(chat.id)
                    }

                    // A chat that is gone is not followed any more.
                    followers.keys.retainAll(fresh.chats.map { it.id }.toSet())
                }

                is Change.Items -> items += change.chat
                is Change.Live -> live += change.chat
                is Change.Memory -> memory[change.chat] = core.memory(change.chat)
            }
        }

        // A response that is complete leaves the text being written and
        // joins the conversation: both are looked at together, so the
        // screen shows the one take the other's place in a single frame.
        val updates = (items + live).filter { it in watched }.associateWith { chat ->
            val follower = followers.getOrPut(chat) { ChatFollower(core, chat) }

            FeedUpdate(
                rows = follower.items(),
                streaming = follower.streaming(),
                live = follower.live(),
            )
        }

        main.post {
            if (closed) return@post

            state?.let { fresh ->
                this.state = fresh

                val open = fresh.chats.map { it.id }.toSet()

                feeds.keys.retainAll(open)
                watched.retainAll(open)
                this.settings.keys.retainAll(open)
                this.memory.keys.retainAll(open)
            }

            account?.let { this.account = it }

            this.settings.putAll(settings)
            this.memory.putAll(memory)

            for ((chat, update) in updates) {
                feeds[chat]?.take(update)
            }
        }
    }

    init {
        made.countDown()
    }

    /**
     * The activity is gone for good: the core stops, and lets go of this
     * and of its threads. Without it a second core would start beside the
     * first the next time the app is opened in the same process.
     */
    override fun onCleared() {
        closed = true

        main.removeCallbacksAndMessages(null)

        if (tradingDesk.isInitialized()) trading.close()

        // Stopping the core waits for its threads: not on the main one.
        fetcher.execute { core.close() }
        fetcher.shutdown()
    }
}
