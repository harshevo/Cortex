import SwiftUI

/// Everything the screens show, kept in step with the core.
///
/// The core holds the truth and says when a part of it has changed; this
/// fetches that part again on the main thread, where the screens read it.
@MainActor
@Observable
final class AppModel {
    private(set) var state: AppState
    private(set) var items: [String: [ItemView]] = [:]
    private(set) var live: [String: LiveView] = [:]
    private(set) var settings: [String: SettingsView] = [:]
    private(set) var memory: [String: MemoryView] = [:]

    /// Who the model on this phone is reached as.
    private(set) var account: AccountView

    /// A page waiting to be shown: the ChatGPT sign-in.
    var signInAddress: URL?

    /// Which screen a rehearsal wants in front. Nothing else sets it.
    var stage: Stage?

    enum Stage {
        case chat, capabilities, memory, phone
    }

    let core: Cortex

    /// Held here because the core only borrows it.
    private let ears: Ears

    init() {
        let folder = Rest.folder

        try? FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)

        let ears = Ears()

        self.ears = ears
        self.core = Cortex(folder: folder.path, device: Self.deviceName, listener: ears)
        self.state = core.state()
        self.account = core.account()

        ears.model = self
    }

    private static var deviceName: String {
        #if os(iOS)
        UIDevice.current.name
        #else
        Host.current().localizedName ?? "Mac"
        #endif
    }

    fileprivate func took(_ changes: [Change]) {
        for change in changes {
            switch change {
            case .state:
                let fresh = core.state()

                withAnimation(Theme.spring) {
                    state = fresh
                }

                account = core.account()

                for chat in fresh.chats {
                    settings[chat.id] = core.settings(chat: chat.id)
                }

            case .items(let chat):
                withAnimation(Theme.spring) {
                    items[chat] = core.items(chat: chat)
                }

            case .live(let chat):
                live[chat] = core.live(chat: chat)

            case .memory(let chat):
                memory[chat] = core.memory(chat: chat)
            }
        }
    }

    var connected: Bool {
        if case .connected = state.connection { return true }

        return false
    }

    func chat(_ id: String) -> ChatView? {
        state.chats.first { $0.id == id }
    }
}

/// Hears the core, which speaks from a background thread.
private final class Ears: Listener, @unchecked Sendable {
    weak var model: AppModel?

    func changed(changes: [Change]) {
        Task { @MainActor [weak self] in
            self?.model?.took(changes)
        }
    }

    func openAddress(address: String) {
        Task { @MainActor [weak self] in
            self?.model?.signInAddress = URL(string: address)
        }
    }
}
