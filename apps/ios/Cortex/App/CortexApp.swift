import SwiftUI

@main
struct CortexApp: App {
    @State private var model: AppModel
    @Environment(\.scenePhase) private var phase

    init() {
        let model = AppModel()

        _model = State(initialValue: model)

        // Not tied to a screen: a rehearsal starts whether or not the
        // window has come up yet.
        Task { @MainActor in
            await Rehearsal.run(model)
        }
    }

    var body: some Scene {
        WindowGroup {
            RootView()
                .environment(model)
                .tint(Theme.accent)
                .onChange(of: phase) { _, phase in
                    // A connection lost while the app was away is made
                    // again at once.
                    if phase == .active {
                        model.core.foreground()
                    }

                    if phase == .background {
                        Rest.schedule()
                    }
                }
            #if os(macOS)
                .frame(width: 393, height: 852)
            #endif
        }
        #if os(macOS)
        .windowResizability(.contentSize)
        #else
        .backgroundTask(.appRefresh(Rest.task)) {
            Rest.schedule()
            Rest.take()
        }
        #endif
    }
}

/// Which screen is in front.
struct RootView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        Group {
            // A phone that works with neither its own agent nor a
            // desktop's has nothing to show but how to begin.
            if unbegun {
                WelcomeScreen()
                    .transition(.opacity)
            } else {
                HomeScreen()
                    .transition(.opacity.combined(with: .scale(scale: 1.02)))
            }
        }
        .animation(Theme.spring, value: unbegun)
        .background(Theme.background.ignoresSafeArea())
        .signInPage()
    }

    private var unbegun: Bool {
        model.state.hosts.isEmpty && !model.state.onPhone
    }
}

private extension View {
    /// Shows the ChatGPT sign-in page over the app, which has to go on
    /// running under it to hear when the sign-in is done.
    func signInPage() -> some View {
        modifier(SignInPage())
    }
}

private struct SignInPage: ViewModifier {
    @Environment(AppModel.self) private var model

    func body(content: Content) -> some View {
        @Bindable var model = model

        #if os(iOS)
        content.sheet(item: $model.signInAddress) { address in
            SafariPage(address: address)
                .ignoresSafeArea()
        }
        #else
        content.onChange(of: model.signInAddress) { _, address in
            if let address {
                NSWorkspace.shared.open(address)

                model.signInAddress = nil
            }
        }
        #endif
    }
}

#if os(iOS)
import SafariServices

extension URL: @retroactive Identifiable {
    public var id: String { absoluteString }
}

private struct SafariPage: UIViewControllerRepresentable {
    let address: URL

    func makeUIViewController(context: Context) -> SFSafariViewController {
        SFSafariViewController(url: address)
    }

    func updateUIViewController(_ controller: SFSafariViewController, context: Context) {}
}
#endif
