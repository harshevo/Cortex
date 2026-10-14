import Foundation

#if os(iOS)
import BackgroundTasks
#endif

/// Letting the living network go over what it met while nobody is using
/// the phone. The system decides when: it wakes the app in the background
/// for a short while, mostly when the phone is idle, and never on request.
enum Rest {
    /// Named in the app's Info.plist; the system runs nothing that is not.
    static let task = "dev.cortex.remote.rest"

    private static let seconds: UInt32 = 15

    /// The folder the core keeps everything in.
    static var folder: URL {
        FileManager.default
            .urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("Cortex", isDirectory: true)
    }

    /// Asks the system for the next rest. Called when the app leaves the
    /// screen.
    static func schedule() {
        #if os(iOS)
        let request = BGAppRefreshTaskRequest(identifier: task)

        request.earliestBeginDate = Date(timeIntervalSinceNow: 30 * 60)

        try? BGTaskScheduler.shared.submit(request)
        #endif
    }

    /// The rest itself. Returns when it is done.
    static func take() {
        let rested = rest(folder: folder.path, seconds: seconds)

        guard rested.replays > 0 else { return }

        let kept = UserDefaults.standard

        kept.set(Date().timeIntervalSince1970, forKey: "rest.at")
        kept.set(Int(rested.replays), forKey: "rest.replays")
        kept.set(Int(rested.neurons), forKey: "rest.neurons")
        kept.set(Int(rested.synapses), forKey: "rest.synapses")
    }

    /// What the last rest did, in a sentence.
    static var last: String? {
        let kept = UserDefaults.standard
        let at = kept.double(forKey: "rest.at")

        guard at > 0 else { return nil }

        let ago = RelativeDateTimeFormatter().localizedString(for: Date(timeIntervalSince1970: at), relativeTo: Date())

        return "Rested \(ago): went over \(kept.integer(forKey: "rest.replays")) moments. "
            + "The living network has \(kept.integer(forKey: "rest.neurons")) neurons and "
            + "\(kept.integer(forKey: "rest.synapses")) synapses."
    }
}
