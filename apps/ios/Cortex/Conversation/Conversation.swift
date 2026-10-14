import SwiftUI

/// One chat: its conversation, what the run is doing, and the composer.
struct ConversationScreen: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss

    let chat: String

    @State private var showingCapabilities = false
    @State private var showingMemory = false

    private var items: [ItemView] { model.items[chat] ?? [] }
    private var live: LiveView { model.live[chat] ?? model.core.live(chat: chat) }
    private var info: ChatView? { model.chat(chat) }

    var body: some View {
        VStack(spacing: 0) {
            Transcript(chat: chat, items: items, live: live, problem: info?.problem)

            VStack(spacing: 10) {
                if let approval = live.approvals.first {
                    ApprovalCard(chat: chat, approval: approval, waiting: live.approvals.count - 1)
                        .transition(.move(edge: .bottom).combined(with: .opacity))
                }

                Composer(chat: chat, live: live, ready: info?.ready ?? false)

                StatsStrip(live: live, model: info?.model ?? "")
            }
            .padding(.horizontal, Theme.gutter)
            .padding(.top, 8)
            .padding(.bottom, 8)
            .background(Theme.background)
            .animation(Theme.spring, value: live.approvals.first?.id)
        }
        .background(Theme.background.ignoresSafeArea())
        .navigationTitle(info?.title ?? "Chat")
        #if os(iOS)
        .navigationBarTitleDisplayMode(.inline)
        #endif
        .toolbar {
            ToolbarItemGroup(placement: .primaryAction) {
                Button {
                    showingMemory = true
                } label: {
                    Image(systemName: "brain")
                }
                .accessibilityLabel("Memory")

                Button {
                    showingCapabilities = true
                } label: {
                    Image(systemName: "slider.horizontal.3")
                }
                .accessibilityLabel("Capabilities")

                Button {
                    Haptics.tap()
                    model.core.newSession(chat: chat, folder: nil)
                } label: {
                    Image(systemName: "square.and.pencil")
                }
                .disabled(live.running)
                .accessibilityLabel("New session")
            }
        }
        .sheet(isPresented: $showingCapabilities) {
            CapabilitiesSheet(chat: chat)
                .macSheet()
        }
        .sheet(isPresented: $showingMemory) {
            MemorySheet(chat: chat)
                .macSheet()
        }
        .onChange(of: model.stage) { _, stage in
            showingCapabilities = stage == .capabilities
            showingMemory = stage == .memory
        }
        .onChange(of: live.approvals.count) { old, new in
            if new > old { Haptics.warning() }
        }
        .onChange(of: live.running) { old, new in
            if old && !new { Haptics.success() }
        }
    }
}

private struct Transcript: View {
    let chat: String
    let items: [ItemView]
    let live: LiveView
    let problem: String?

    /// Whether the newest text is on screen: only then does new text
    /// pull the view down with it.
    @State private var following = true

    private let bottom = "bottom"

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 6) {
                    if let problem {
                        Label(problem, systemImage: "exclamationmark.triangle.fill")
                            .font(.footnote)
                            .foregroundStyle(Theme.bad)
                            .card()
                    }

                    if items.isEmpty && !live.running {
                        EmptyChat()
                    }

                    ForEach(items) { item in
                        ItemRow(chat: chat, item: item)
                            .transition(.opacity.combined(with: .offset(y: 10)))
                    }

                    if live.thinking != nil || live.writing != nil {
                        WritingNow(thinking: live.thinking, writing: live.writing)
                    }

                    if let activity = live.activity {
                        ActivityLine(text: activity, seconds: live.activitySeconds)
                    }

                    ForEach(Array(live.queued.enumerated()), id: \.offset) { _, queued in
                        Label(queued, systemImage: "clock")
                            .font(.footnote)
                            .foregroundStyle(Theme.textSecondary)
                            .padding(.horizontal, 12)
                            .padding(.vertical, 8)
                            .background(Theme.raised.opacity(0.6), in: Capsule())
                            .frame(maxWidth: .infinity, alignment: .trailing)
                    }

                    Color.clear
                        .frame(height: 24)
                        .id(bottom)
                        .onAppear { following = true }
                        .onDisappear { following = false }
                }
                .padding(.horizontal, Theme.gutter)
                .padding(.vertical, 12)
                .frame(maxWidth: 760)
                .frame(maxWidth: .infinity)
            }
            // Stays at the newest text as it grows, until the person
            // scrolls away from it.
            .defaultScrollAnchor(.bottom)
            #if os(iOS)
            .scrollDismissesKeyboard(.interactively)
            #endif
            .overlay(alignment: .bottomTrailing) {
                if !following {
                    Button {
                        withAnimation(Theme.spring) {
                            proxy.scrollTo(bottom, anchor: .bottom)
                        }
                    } label: {
                        Image(systemName: "arrow.down")
                            .font(.footnote.weight(.bold))
                            .foregroundStyle(Theme.text)
                            .frame(width: 40, height: 40)
                            .background(.ultraThinMaterial, in: Circle())
                            .overlay(Circle().strokeBorder(Theme.border, lineWidth: 1))
                    }
                    .buttonStyle(.plain)
                    .padding(14)
                    .transition(.scale.combined(with: .opacity))
                }
            }
            .animation(Theme.quick, value: following)
            .onChange(of: items.count) { _, _ in follow(proxy) }
            .onChange(of: live.writing) { _, _ in follow(proxy, animated: false) }
            .onChange(of: live.activity) { _, _ in follow(proxy) }
            .onAppear { proxy.scrollTo(bottom, anchor: .bottom) }
        }
    }

    private func follow(_ proxy: ScrollViewProxy, animated: Bool = true) {
        guard following else { return }

        if animated {
            withAnimation(Theme.spring) {
                proxy.scrollTo(bottom, anchor: .bottom)
            }
        } else {
            proxy.scrollTo(bottom, anchor: .bottom)
        }
    }
}

private struct EmptyChat: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        VStack(spacing: 14) {
            LogoBadge(size: 56)

            Text("What should Cortex do?")
                .font(Theme.title(22))
                .foregroundStyle(Theme.text)

            Text(
                model.state.onPhone
                    ? "It runs on this phone, in this chat's project, and asks before it changes anything. Keep the app open while it works."
                    : "It runs on your computer, in this chat's project. You can leave: it carries on, and asks here before it changes anything."
            )
                .font(.callout)
                .foregroundStyle(Theme.textSecondary)
                .multilineTextAlignment(.center)
        }
        .frame(maxWidth: .infinity)
        .padding(.top, 60)
        .padding(.horizontal, 12)
    }
}

/// The response as it is being written.
private struct WritingNow: View {
    let thinking: String?
    let writing: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            if let thinking, writing == nil {
                HStack(alignment: .top, spacing: 8) {
                    Image(systemName: "brain")
                        .font(.footnote.weight(.semibold))
                        .foregroundStyle(Theme.model)
                        .symbolEffect(.pulse)

                    // Only the end of it: the thought moves on.
                    Text(String(thinking.suffix(260)))
                        .font(.footnote)
                        .foregroundStyle(Theme.textSecondary)
                        .lineLimit(4)
                }
            }

            if let writing {
                Prose(text: writing)

                Cursor()
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

private struct Cursor: View {
    @State private var on = true

    var body: some View {
        RoundedRectangle(cornerRadius: 2)
            .fill(Theme.accent)
            .frame(width: 9, height: 16)
            .opacity(on ? 1 : 0.15)
            .onAppear {
                withAnimation(.easeInOut(duration: 0.55).repeatForever(autoreverses: true)) {
                    on = false
                }
            }
    }
}

/// What the run is busy with while there is nothing to read.
private struct ActivityLine: View {
    let text: String
    let seconds: Float

    var body: some View {
        HStack(spacing: 10) {
            ThinkingDots()

            Text(text)
                .font(.subheadline)
                .foregroundStyle(Theme.textSecondary)
                .shimmer()

            if seconds >= 3 {
                Text("\(Int(seconds))s")
                    .font(.caption.monospacedDigit())
                    .foregroundStyle(Theme.textMuted)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .transition(.opacity)
    }
}

private struct ThinkingDots: View {
    @State private var phase = 0.0

    var body: some View {
        HStack(spacing: 4) {
            ForEach(0..<3, id: \.self) { index in
                Circle()
                    .fill(Theme.accent)
                    .frame(width: 6, height: 6)
                    .scaleEffect(1 + 0.5 * sin(phase + Double(index) * 0.9))
                    .opacity(0.55 + 0.45 * sin(phase + Double(index) * 0.9))
            }
        }
        .onAppear {
            withAnimation(.linear(duration: 1.2).repeatForever(autoreverses: false)) {
                phase = .pi * 2
            }
        }
    }
}

/// A tool's question, with its answers.
private struct ApprovalCard: View {
    @Environment(AppModel.self) private var model

    let chat: String
    let approval: ApprovalView

    /// How many more questions wait behind this one.
    let waiting: Int

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(spacing: 8) {
                Image(systemName: "hand.raised.fill")
                    .foregroundStyle(Theme.warning)

                Text("Cortex wants to use \(approval.tool.replacingOccurrences(of: "_", with: " "))")
                    .font(.subheadline.weight(.semibold))
                    .foregroundStyle(Theme.text)

                Spacer(minLength: 0)

                if waiting > 0 {
                    Text("+\(waiting)")
                        .font(.caption.weight(.semibold).monospacedDigit())
                        .foregroundStyle(Theme.textMuted)
                }
            }

            Text(approval.summary)
                .font(Theme.mono)
                .foregroundStyle(Theme.text)
                .lineLimit(8)
                .frame(maxWidth: .infinity, alignment: .leading)
                .textSelection(.enabled)
                .padding(10)
            .background(Theme.sunken, in: RoundedRectangle(cornerRadius: Theme.smallRadius, style: .continuous))

            HStack(spacing: 8) {
                answer("Deny", .deny, fill: Theme.raised, text: Theme.text)
                answer("Always", .alwaysAllow, fill: Theme.raised, text: Theme.text)
                answer("Allow", .allow, fill: Theme.accent, text: Theme.onAccent)
            }
        }
        .padding(14)
        .background(Theme.surface, in: RoundedRectangle(cornerRadius: Theme.radius, style: .continuous))
        .overlay(
            RoundedRectangle(cornerRadius: Theme.radius, style: .continuous)
                .strokeBorder(Theme.warning.opacity(0.55), lineWidth: 1)
        )
        .shadow(color: .black.opacity(0.18), radius: 18, y: 6)
    }

    private func answer(_ title: String, _ answer: Answer, fill: Color, text: Color) -> some View {
        Button {
            Haptics.tap()
            model.core.answer(chat: chat, id: approval.id, answer: answer)
        } label: {
            Text(title)
                .font(.subheadline.weight(.semibold))
                .foregroundStyle(text)
                .frame(maxWidth: .infinity, minHeight: 44)
                .background(fill, in: RoundedRectangle(cornerRadius: 12, style: .continuous))
        }
        .buttonStyle(PressableStyle())
    }
}

/// The loop of the run, one square per thing done, and its numbers.
private struct StatsStrip: View {
    let live: LiveView
    let model: String

    var body: some View {
        HStack(spacing: 10) {
            HStack(spacing: 3) {
                ForEach(Array(live.stats.cells.suffix(22).enumerated()), id: \.offset) { _, cell in
                    RoundedRectangle(cornerRadius: 2)
                        .fill(color(cell))
                        .frame(width: 6, height: 10)
                        .transition(.scale.combined(with: .opacity))
                }
            }
            .animation(Theme.quick, value: live.stats.cells.count)

            Spacer(minLength: 0)

            if let speed = live.speed {
                Text("\(Int(speed)) tok/s")
            }

            if live.stats.inputTokens + live.stats.outputTokens > 0 {
                Text("\(compact(live.stats.inputTokens)) in · \(compact(live.stats.outputTokens)) out")
            }

            if let cached = live.stats.cacheHit {
                Text("\(Int(cached * 100))% cached")
            }
        }
        .font(.caption2.monospacedDigit())
        .foregroundStyle(Theme.textMuted)
        .lineLimit(1)
        .frame(height: 14)
    }

    private func color(_ cell: CellView) -> Color {
        switch cell {
        case .thinking: Theme.model.opacity(0.45)
        case .model: Theme.model
        case .observation: Theme.accent
        case .failedObservation: Theme.bad
        case .checkPassed: Theme.good
        case .checkFailed: Theme.warning
        }
    }
}
