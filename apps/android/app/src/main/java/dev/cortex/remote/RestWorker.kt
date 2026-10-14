package dev.cortex.remote

import android.content.Context
import android.text.format.DateUtils
import androidx.work.Constraints
import androidx.work.ExistingPeriodicWorkPolicy
import androidx.work.PeriodicWorkRequestBuilder
import androidx.work.WorkManager
import androidx.work.Worker
import androidx.work.WorkerParameters
import java.io.File
import java.util.concurrent.TimeUnit

/**
 * Lets the living network go over what it met while nobody is using the
 * phone. The system runs this about once an hour, when the battery is
 * not low, whether or not the app is open: it needs no screen and no
 * connection.
 */
class RestWorker(context: Context, parameters: WorkerParameters) : Worker(context, parameters) {
    override fun doWork(): Result {
        val folder = folder(applicationContext)

        val rested = uniffi.cortex_mobile.rest(folder, SECONDS)

        if (rested.replays > 0u) {
            applicationContext.getSharedPreferences(STORE, Context.MODE_PRIVATE).edit()
                .putLong("at", System.currentTimeMillis())
                .putLong("replays", rested.replays.toLong())
                .putLong("neurons", rested.neurons.toLong())
                .putLong("synapses", rested.synapses.toLong())
                .apply()
        }

        return Result.success()
    }

    companion object {
        private const val STORE = "rest"
        private const val SECONDS = 20u

        /** The folder the core keeps everything in. */
        fun folder(context: Context): String =
            File(context.filesDir, "cortex").apply { mkdirs() }.path

        fun schedule(context: Context) {
            val constraints = Constraints.Builder().setRequiresBatteryNotLow(true).build()

            val work = PeriodicWorkRequestBuilder<RestWorker>(1, TimeUnit.HOURS)
                .setConstraints(constraints)
                .build()

            WorkManager.getInstance(context)
                .enqueueUniquePeriodicWork("rest", ExistingPeriodicWorkPolicy.KEEP, work)
        }

        /** What the last rest did, in a sentence. */
        fun last(context: Context): String? {
            val store = context.getSharedPreferences(STORE, Context.MODE_PRIVATE)
            val at = store.getLong("at", 0)

            if (at == 0L) return null

            val ago = DateUtils.getRelativeTimeSpanString(at, System.currentTimeMillis(), DateUtils.MINUTE_IN_MILLIS)

            return "Rested $ago: went over ${store.getLong("replays", 0)} moments. " +
                "The living network has ${store.getLong("neurons", 0)} neurons and " +
                "${store.getLong("synapses", 0)} synapses."
        }
    }
}
