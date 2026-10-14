import SwiftUI

/// The chats open on the computer, and its projects and their sessions.
struct HomeScreen: View {
    @Environment(AppModel.self) private var model

    @State private var path: [String] = []
    @State private var showingHosts = false
    @State private var showingPhone = false
    @State private var showingTrading = false
    @State private var showingInsights = false

    var body: some View {
        NavigationStack(path: $path) {
            ScrollView {
                VStack(alignment: .leading, spacing: 28) {
                    HomeHeader(
                        showingHosts: $showingHosts,
                        showingTrading: $showingTrading,
                        showingInsights: $showingInsights
                    )

                    if model.connected {
                        ChatsSection(open: open)

                        ProjectsSection(open: open)
                    } else {
                        Waiting(showingHosts: $showingHosts)
                    }
                }
                .padding(.horizontal, Theme.gutter)
                .padding(.bottom, 32)
                .frame(maxWidth: 640)
                .frame(maxWidth: .infinity)
            }
            .background(Theme.background.ignoresSafeArea())
            .navigationDestination(for: String.self) { chat in
                ConversationScreen(chat: chat)
            }
            #if os(iOS)
            .toolbar(.hidden, for: .navigationBar)
            #endif
        }
        .sheet(isPresented: $showingHosts) {
            HostsSheet(showPhone: { showingPhone = true })
                .macSheet()
        }
        .sheet(isPresented: $showingPhone) {
            PhoneSheet()
                .macSheet()
        }
        .sheet(isPresented: $showingTrading) {
            NavigationStack { TradingScreen() }
                .macSheet()
        }
        .sheet(isPresented: $showingInsights) {
            NavigationStack { InsightsScreen() }
                .macSheet()
        }
        // A chat that is closed on the computer cannot stay open here.
        .onChange(of: model.state.chats.map(\.id)) { _, chats in
            path.removeAll { !chats.contains($0) }
        }
        .onChange(of: model.connected) { _, connected in
            if !connected {
                path.removeAll()
            }
        }
        .onChange(of: model.stage) { _, stage in
            if stage == .chat, let chat = model.state.chats.first {
                path = [chat.id]
            }

            if stage == .phone {
                showingPhone = true
            }
        }
    }

    private func open(_ chat: String) {
        Haptics.tap()
        path = [chat]
    }
}

private struct HomeHeader: View {
    @Environment(AppModel.self) private var model
    @Binding var showingHosts: Bool
    @Binding var showingTrading: Bool
    @Binding var showingInsights: Bool

    var body: some View {
        HStack(alignment: .center, spacing: 14) {
            LogoBadge(size: 44)

            VStack(alignment: .leading, spacing: 2) {
                Text("Cortex")
                    .font(Theme.title(26))
                    .foregroundStyle(Theme.text)

                ConnectionPill(connection: model.state.connection)
            }

            Spacer()

            RoundButton(symbol: "chart.bar.xaxis", label: "Insights") {
                showingInsights = true
            }

            RoundButton(symbol: "chart.line.uptrend.xyaxis", label: "Trading") {
                showingTrading = true
            }

            Button {
                showingHosts = true
            } label: {
                Image(systemName: "desktopcomputer")
                    .font(.system(size: 17, weight: .medium))
                    .foregroundStyle(Theme.text)
                    .frame(width: 44, height: 44)
                    .background(Theme.surface, in: Circle())
                    .overlay(Circle().strokeBorder(Theme.border.opacity(0.7), lineWidth: 1))
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Computers")
        }
        .padding(.top, 12)
    }
}

/// A round button with a symbol, as in the header.
private struct RoundButton: View {
    let symbol: String
    let label: String
    let action: () -> Void

    var body: some View {
        Button {
            Haptics.tap()
            action()
        } label: {
            Image(systemName: symbol)
                .font(.system(size: 16, weight: .medium))
                .foregroundStyle(Theme.text)
                .frame(width: 44, height: 44)
                .background(Theme.surface, in: Circle())
                .overlay(Circle().strokeBorder(Theme.border.opacity(0.7), lineWidth: 1))
        }
        .buttonStyle(.plain)
        .accessibilityLabel(label)
    }
}

/// The connection in a word, with a light.
struct ConnectionPill: View {
    let connection: Connection

    var body: some View {
        HStack(spacing: 6) {
            StatusLight(color: color, pulsing: pulsing)

            Text(text)
                .font(.footnote)
                .foregroundStyle(Theme.textSecondary)
                .lineLimit(1)
        }
        .animation(Theme.quick, value: text)
    }

    private var text: String {
        switch connection {
        case .idle: "Not connected"
        case .connecting(let host): "Connecting to \(host)…"
        case .connected(let host): host
        case .reconnecting(let host, _): "Reaching \(host)…"
        case .failed: "Could not connect"
        }
    }

    private var color: Color {
        switch connection {
        case .connected: Theme.good
        case .connecting, .reconnecting: Theme.warning
        case .failed: Theme.bad
        case .idle: Theme.textMuted
        }
    }

    private var pulsing: Bool {
        switch connection {
        case .connecting, .reconnecting: true
        default: false
        }
    }
}

/// A small light that can breathe.
struct StatusLight: View {
    let color: Color
    var pulsing = false
    var size: CGFloat = 8

    @State private var breath = false

    var body: some View {
        Circle()
            .fill(color)
            .frame(width: size, height: size)
            .overlay(
                Circle()
                    .stroke(color.opacity(0.5), lineWidth: 2)
                    .scaleEffect(breath ? 2.4 : 1)
                    .opacity(breath ? 0 : (pulsing ? 0.9 : 0))
            )
            .onAppear { animate() }
            .onChange(of: pulsing) { _, _ in animate() }
    }

    private func animate() {
        breath = false

        guard pulsing else { return }

        withAnimation(.easeOut(duration: 1.3).repeatForever(autoreverses: false)) {
            breath = true
        }
    }
}

private struct ChatsSection: View {
    @Environment(AppModel.self) private var model
    let open: (String) -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: Theme.gap) {
            Text(model.state.onPhone ? "On this phone" : "Open on your computer").sectionHeading()

            if model.state.chats.isEmpty {
                Text("No chats yet.")
                    .font(.callout)
                    .foregroundStyle(Theme.textMuted)
            }

            ForEach(model.state.chats, id: \.id) { chat in
                Button {
                    open(chat.id)
                } label: {
                    ChatCard(chat: chat)
                }
                .buttonStyle(PressableStyle())
                .transition(.opacity.combined(with: .move(edge: .top)))
            }
        }
    }
}

private struct ChatCard: View {
    let chat: ChatView

    var body: some View {
        HStack(alignment: .center, spacing: 14) {
            ZStack {
                Circle()
                    .fill(tint.opacity(0.15))
                    .frame(width: 42, height: 42)

                if chat.running && !chat.asking {
                    RunningRing(color: tint)
                        .frame(width: 42, height: 42)
                }

                Image(systemName: symbol)
                    .font(.system(size: 16, weight: .semibold))
                    .foregroundStyle(tint)
            }

            VStack(alignment: .leading, spacing: 3) {
                Text(chat.title)
                    .font(.body.weight(.medium))
                    .foregroundStyle(Theme.text)
                    .lineLimit(1)

                HStack(spacing: 6) {
                    Text(status)
                        .foregroundStyle(chat.asking ? Theme.warning : Theme.textSecondary)

                    Text("·").foregroundStyle(Theme.textMuted)

                    Text(chat.placeName)
                        .foregroundStyle(Theme.textMuted)
                        .lineLimit(1)
                }
                .font(.footnote)
            }

            Spacer(minLength: 0)

            Image(systemName: "chevron.right")
                .font(.footnote.weight(.semibold))
                .foregroundStyle(Theme.textMuted)
        }
        .card()
        .contentShape(Rectangle())
    }

    private var status: String {
        if chat.asking { return "Needs your answer" }
        if chat.running { return "Working" }
        if chat.problem != nil { return "Not set up" }
        if !chat.ready { return "Starting" }

        return chat.model.isEmpty ? "Ready" : chat.model
    }

    private var symbol: String {
        if chat.asking { return "hand.raised.fill" }
        if chat.problem != nil { return "exclamationmark" }

        return "bubble.left.fill"
    }

    private var tint: Color {
        if chat.asking { return Theme.warning }
        if chat.problem != nil { return Theme.bad }
        if chat.running { return Theme.accent }

        return Theme.model
    }
}

/// An arc that goes round while something works.
struct RunningRing: View {
    let color: Color

    @State private var turning = false

    var body: some View {
        Circle()
            .trim(from: 0, to: 0.28)
            .stroke(color, style: StrokeStyle(lineWidth: 2.5, lineCap: .round))
            .rotationEffect(.degrees(turning ? 360 : 0))
            .onAppear {
                withAnimation(.linear(duration: 1.1).repeatForever(autoreverses: false)) {
                    turning = true
                }
            }
    }
}

struct PressableStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .scaleEffect(configuration.isPressed ? 0.975 : 1)
            .opacity(configuration.isPressed ? 0.85 : 1)
            .animation(Theme.quick, value: configuration.isPressed)
    }
}

private struct ProjectsSection: View {
    @Environment(AppModel.self) private var model
    let open: (String) -> Void

    @State private var expanded: Set<String> = []

    var body: some View {
        VStack(alignment: .leading, spacing: Theme.gap) {
            Text("Projects and past sessions").sectionHeading()

            ForEach(model.state.projects, id: \.path) { project in
                VStack(alignment: .leading, spacing: 0) {
                    Button {
                        withAnimation(Theme.spring) {
                            if !expanded.insert(project.path).inserted {
                                expanded.remove(project.path)
                            }
                        }
                    } label: {
                        HStack(spacing: 12) {
                            Image(systemName: project.scratch ? "bubble.left.and.bubble.right" : "folder")
                                .font(.system(size: 15, weight: .medium))
                                .foregroundStyle(Theme.textSecondary)
                                .frame(width: 24)

                            Text(project.scratch ? "Chats" : project.name)
                                .font(.body.weight(.medium))
                                .foregroundStyle(Theme.text)
                                .lineLimit(1)

                            Spacer()

                            Text("\(project.sessions.count)")
                                .font(.footnote.monospacedDigit())
                                .foregroundStyle(Theme.textMuted)

                            Image(systemName: "chevron.right")
                                .font(.caption.weight(.semibold))
                                .foregroundStyle(Theme.textMuted)
                                .rotationEffect(.degrees(expanded.contains(project.path) ? 90 : 0))
                        }
                        .frame(minHeight: 44)
                        .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain)

                    if expanded.contains(project.path) {
                        SessionRows(project: project, open: open)
                            .transition(.opacity.combined(with: .move(edge: .top)))
                    }
                }
                .card(padding: 12)
            }
        }
    }
}

private struct SessionRows: View {
    @Environment(AppModel.self) private var model

    let project: ProjectView
    let open: (String) -> Void

    /// The chat that sessions are opened in: the first that is free.
    private var target: ChatView? {
        model.state.chats.first { !$0.running } ?? model.state.chats.first
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Divider().overlay(Theme.border).padding(.vertical, 6)

            Button {
                guard let target else { return }

                model.core.newSession(chat: target.id, folder: project.path)
                open(target.id)
            } label: {
                Label("New session here", systemImage: "plus")
                    .font(.callout.weight(.medium))
                    .foregroundStyle(Theme.accent)
                    .frame(maxWidth: .infinity, minHeight: 44, alignment: .leading)
                    .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .disabled(target?.running ?? true)

            ForEach(project.sessions, id: \.path) { session in
                Button {
                    guard let target else { return }

                    model.core.openSession(chat: target.id, path: session.path)
                    open(target.id)
                } label: {
                    HStack(spacing: 10) {
                        Text(session.title.isEmpty ? "Untitled" : session.title)
                            .font(.callout)
                            .foregroundStyle(Theme.text)
                            .lineLimit(1)

                        Spacer(minLength: 8)

                        Text(ago(session.updatedMs))
                            .font(.caption)
                            .foregroundStyle(Theme.textMuted)
                    }
                    .frame(minHeight: 44)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                .disabled(target?.running ?? true)
            }

            if target?.running == true {
                Text("The chat is working. Sessions can be opened when it is done.")
                    .font(.caption)
                    .foregroundStyle(Theme.textMuted)
                    .padding(.top, 4)
            }
        }
    }

    private func ago(_ milliseconds: UInt64) -> String {
        let date = Date(timeIntervalSince1970: Double(milliseconds) / 1000)

        return date.formatted(.relative(presentation: .named))
    }
}

/// Shown instead of the lists while there is no connection.
private struct Waiting: View {
    @Environment(AppModel.self) private var model
    @Binding var showingHosts: Bool

    var body: some View {
        VStack(spacing: 18) {
            LogoBadge(size: 72, alive: true)
                .padding(.top, 40)

            Text(title)
                .font(Theme.title(22))
                .foregroundStyle(Theme.text)

            ConnectionNote(connection: model.state.connection)

            Text("Cortex has to be open on the computer, with remote control switched on in its settings.")
                .font(.callout)
                .foregroundStyle(Theme.textSecondary)
                .multilineTextAlignment(.center)

            if case .idle = model.state.connection, let host = model.state.hosts.first {
                Button("Connect to \(host.name)") {
                    model.core.connect(host: host.id)
                }
                .buttonStyle(PrimaryButtonStyle())
            }

            Button("Work on this phone") {
                model.core.usePhone()
            }
            .buttonStyle(SecondaryButtonStyle())

            Button("Computers") {
                showingHosts = true
            }
            .buttonStyle(SecondaryButtonStyle())
        }
        .frame(maxWidth: .infinity)
    }

    private var title: String {
        switch model.state.connection {
        case .failed: "Could not connect"
        case .idle: "Not connected"
        default: "Finding your computer"
        }
    }
}

/// The computers this phone is paired with.
struct HostsSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss

    @State private var pairing = false

    /// Opens the phone's own settings once this sheet is gone.
    var showPhone: () -> Void = {}

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: Theme.gap) {
                    HStack(spacing: 12) {
                        Image(systemName: "iphone")
                            .foregroundStyle(model.state.onPhone ? Theme.accent : Theme.textSecondary)
                            .frame(width: 28)

                        VStack(alignment: .leading, spacing: 2) {
                            Text("This phone")
                                .font(.body.weight(.medium))
                                .foregroundStyle(Theme.text)

                            Text(model.state.onPhone ? "In use" : "The agent runs here, on its own")
                                .font(.footnote)
                                .foregroundStyle(Theme.textSecondary)
                        }

                        Spacer()

                        Menu {
                            if !model.state.onPhone {
                                Button("Use") {
                                    model.core.usePhone()
                                    dismiss()
                                }
                            }

                            Button("Model and projects") {
                                dismiss()
                                showPhone()
                            }
                        } label: {
                            Image(systemName: "ellipsis")
                                .foregroundStyle(Theme.textSecondary)
                                .frame(width: 44, height: 44)
                                .contentShape(Rectangle())
                        }
                    }
                    .card(padding: 12)

                    ForEach(model.state.hosts, id: \.id) { host in
                        let current = model.state.host == host.id

                        HStack(spacing: 12) {
                            Image(systemName: "desktopcomputer")
                                .foregroundStyle(current ? Theme.accent : Theme.textSecondary)
                                .frame(width: 28)

                            VStack(alignment: .leading, spacing: 2) {
                                Text(host.name)
                                    .font(.body.weight(.medium))
                                    .foregroundStyle(Theme.text)

                                if current {
                                    ConnectionPill(connection: model.state.connection)
                                }
                            }

                            Spacer()

                            Menu {
                                if current {
                                    Button("Disconnect") { model.core.disconnect() }
                                } else {
                                    Button("Connect") { model.core.connect(host: host.id) }
                                }

                                Button("Forget", role: .destructive) {
                                    model.core.forget(host: host.id)
                                }
                            } label: {
                                Image(systemName: "ellipsis")
                                    .foregroundStyle(Theme.textSecondary)
                                    .frame(width: 44, height: 44)
                                    .contentShape(Rectangle())
                            }
                        }
                        .card(padding: 12)
                        .contentShape(Rectangle())
                        .onTapGesture {
                            if !current {
                                model.core.connect(host: host.id)
                            }
                        }
                    }

                    Button {
                        pairing = true
                    } label: {
                        Label("Pair with another computer", systemImage: "qrcode.viewfinder")
                    }
                    .buttonStyle(SecondaryButtonStyle())
                    .padding(.top, 8)
                }
                .padding(Theme.gutter)
            }
            .background(Theme.background.ignoresSafeArea())
            .navigationTitle("Where the agent runs")
            #if os(iOS)
            .navigationBarTitleDisplayMode(.inline)
            #endif
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                }
            }
            .sheet(isPresented: $pairing) {
                PairingSheet()
            }
        }
        .presentationDetents([.medium, .large])
    }
}
