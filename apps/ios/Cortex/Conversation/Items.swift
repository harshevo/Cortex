import SwiftUI

/// One thing in the conversation.
struct ItemRow: View {
    let chat: String
    let item: ItemView

    var body: some View {
        switch item {
        case .user(let id, let text, let images):
            UserBubble(chat: chat, item: id, text: text, images: Int(images))

        case .briefing(_, let source, let text):
            Fold(symbol: "sparkles", title: "Context from \(source)", tint: Theme.textSecondary) {
                Text(text)
                    .font(.footnote)
                    .foregroundStyle(Theme.textSecondary)
                    .textSelection(.enabled)
            }

        case .assistant(_, let text, let thinking, _, _):
            VStack(alignment: .leading, spacing: 10) {
                if !thinking.isEmpty {
                    Fold(symbol: "brain", title: "Thought", tint: Theme.model) {
                        Text(thinking)
                            .font(.footnote)
                            .foregroundStyle(Theme.textSecondary)
                            .textSelection(.enabled)
                    }
                }

                if !text.isEmpty {
                    Prose(text: text)
                        .padding(.vertical, 6)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)

        case .tools(_, let tools):
            ToolGroup(tools: tools)

        case .checks(_, let passed, let checks):
            Fold(
                symbol: passed ? "checkmark.seal.fill" : "xmark.seal.fill",
                title: passed ? "The project's checks pass" : "The project's checks fail",
                tint: passed ? Theme.good : Theme.bad,
                startsOpen: !passed
            ) {
                VStack(alignment: .leading, spacing: 10) {
                    ForEach(Array(checks.enumerated()), id: \.offset) { _, check in
                        VStack(alignment: .leading, spacing: 6) {
                            Label(check.command, systemImage: check.passed ? "checkmark" : "xmark")
                                .font(Theme.mono)
                                .foregroundStyle(check.passed ? Theme.good : Theme.bad)

                            if !check.output.isEmpty {
                                CodeBlock(code: check.output)
                            }
                        }
                    }
                }
            }

        case .review(_, let sentBack, let requirements):
            Fold(
                symbol: "checklist",
                title: sentBack ? "Reviewed: sent back" : "Reviewed against the request",
                tint: sentBack ? Theme.warning : Theme.good
            ) {
                VStack(alignment: .leading, spacing: 8) {
                    ForEach(Array(requirements.enumerated()), id: \.offset) { _, requirement in
                        HStack(alignment: .firstTextBaseline, spacing: 8) {
                            Image(systemName: requirement.met ? "checkmark.circle.fill" : "circle.dashed")
                                .foregroundStyle(requirement.met ? Theme.good : Theme.warning)

                            VStack(alignment: .leading, spacing: 2) {
                                Text(requirement.text)
                                    .foregroundStyle(Theme.text)

                                if !requirement.why.isEmpty {
                                    Text(requirement.why).foregroundStyle(Theme.textMuted)
                                }
                            }
                        }
                        .font(.footnote)
                    }
                }
            }

        case .notice(_, let text, let bad):
            Text(text)
                .font(.footnote)
                .foregroundStyle(bad ? Theme.bad : Theme.textMuted)
                .frame(maxWidth: .infinity, alignment: .leading)
                .textSelection(.enabled)

        case .summary(_, let headline, let tone, let steps, let tokens, let seconds):
            SummaryLine(headline: headline, tone: tone, steps: steps, tokens: tokens, seconds: seconds)
        }
    }
}

private struct UserBubble: View {
    @Environment(AppModel.self) private var model

    let chat: String
    let item: UInt64
    let text: String
    let images: Int

    var body: some View {
        VStack(alignment: .trailing, spacing: 6) {
            if images > 0 {
                HStack(spacing: 6) {
                    ForEach(0..<images, id: \.self) { index in
                        if let data = model.core.picture(chat: chat, item: item, index: UInt32(index)),
                           let image = PlatformImage(data: data) {
                            Image(platform: image)
                                .resizable()
                                .scaledToFill()
                                .frame(width: 72, height: 72)
                                .clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
                        }
                    }
                }
            }

            Text(text)
                .font(.body)
                .foregroundStyle(Theme.text)
                .padding(.horizontal, 14)
                .padding(.vertical, 10)
                .background(Theme.raised, in: RoundedRectangle(cornerRadius: 20, style: .continuous))
                .textSelection(.enabled)
        }
        .frame(maxWidth: .infinity, alignment: .trailing)
        .padding(.leading, 44)
        .padding(.top, 10)
        .padding(.bottom, 8)
    }
}

/// A line that unfolds to show what is behind it.
struct Fold<Content: View>: View {
    let symbol: String
    let title: String
    let tint: Color
    var detail: String? = nil
    var startsOpen = false

    @ViewBuilder let content: () -> Content

    @State private var open: Bool?

    private var isOpen: Bool { open ?? startsOpen }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Button {
                Haptics.tap()

                withAnimation(Theme.spring) {
                    open = !isOpen
                }
            } label: {
                HStack(spacing: 8) {
                    Image(systemName: symbol)
                        .font(.footnote.weight(.semibold))
                        .foregroundStyle(tint)
                        .frame(width: 18)

                    Text(title)
                        .font(.subheadline.weight(.medium))
                        .foregroundStyle(Theme.textSecondary)
                        .lineLimit(1)

                    if let detail {
                        Text(detail)
                            .font(.caption.monospacedDigit())
                            .foregroundStyle(Theme.textMuted)
                    }

                    Image(systemName: "chevron.right")
                        .font(.caption2.weight(.bold))
                        .foregroundStyle(Theme.textMuted)
                        .rotationEffect(.degrees(isOpen ? 90 : 0))

                    Spacer(minLength: 0)
                }
                .frame(minHeight: 32)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)

            if isOpen {
                content()
                    .padding(.leading, 26)
                    .transition(.opacity.combined(with: .move(edge: .top)))
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}

/// Tool calls that came one after another, behind one line.
private struct ToolGroup: View {
    let tools: [ToolView]

    private var running: Bool { tools.contains { $0.output == nil } }
    private var failed: Int { tools.filter(\.failed).count }

    var body: some View {
        Fold(
            symbol: running ? "circle.dotted" : "wrench.and.screwdriver.fill",
            title: title,
            tint: failed > 0 ? Theme.warning : Theme.accent,
            detail: failed > 0 ? "\(failed) failed" : nil
        ) {
            VStack(alignment: .leading, spacing: 4) {
                ForEach(Array(tools.enumerated()), id: \.offset) { _, tool in
                    ToolRow(tool: tool)
                }
            }
        }
    }

    /// What was done, in the words a person would use.
    private var title: String {
        var counts: [(String, Int)] = []

        for tool in tools {
            let kind = Self.kind(tool.name)

            if let index = counts.firstIndex(where: { $0.0 == kind }) {
                counts[index].1 += 1
            } else {
                counts.append((kind, 1))
            }
        }

        let parts = counts.map { kind, count -> String in
            switch kind {
            case "read": "read \(count) file\(count == 1 ? "" : "s")"
            case "search": "searched \(count) time\(count == 1 ? "" : "s")"
            case "edit": "made \(count) edit\(count == 1 ? "" : "s")"
            case "run": "ran \(count) command\(count == 1 ? "" : "s")"
            case "browse": "used the browser \(count) time\(count == 1 ? "" : "s")"
            default: "used \(count) tool\(count == 1 ? "" : "s")"
            }
        }

        let line = parts.joined(separator: ", ")

        return line.prefix(1).uppercased() + line.dropFirst()
    }

    private static func kind(_ name: String) -> String {
        switch name {
        case "read_file", "outline", "read_symbol", "list_files": "read"
        case "search", "semantic_search", "find_references", "file_dependencies", "recall": "search"
        case "edit_file", "write_file": "edit"
        case "shell", "checks": "run"
        case "browser", "browser_view": "browse"
        default: "other"
        }
    }
}

private struct ToolRow: View {
    let tool: ToolView

    @State private var open = false

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Button {
                withAnimation(Theme.spring) { open.toggle() }
            } label: {
                HStack(spacing: 8) {
                    Group {
                        if tool.output == nil {
                            ProgressView().controlSize(.mini)
                        } else {
                            Image(systemName: tool.failed ? "xmark" : "checkmark")
                                .font(.caption2.weight(.bold))
                                .foregroundStyle(tool.failed ? Theme.bad : Theme.good)
                        }
                    }
                    .frame(width: 14)

                    Text(tool.name)
                        .font(Theme.monoSmall.weight(.semibold))
                        .foregroundStyle(Theme.textSecondary)

                    Text(tool.summary)
                        .font(Theme.monoSmall)
                        .foregroundStyle(Theme.textMuted)
                        .lineLimit(1)
                        .truncationMode(.middle)

                    Spacer(minLength: 0)
                }
                .frame(minHeight: 30)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)

            if open {
                VStack(alignment: .leading, spacing: 8) {
                    CodeBlock(code: tool.input)

                    if let output = tool.output, !output.isEmpty {
                        CodeBlock(code: output)
                    }
                }
                .transition(.opacity)
            }
        }
    }
}

private struct SummaryLine: View {
    let headline: String
    let tone: SummaryTone
    let steps: UInt32
    let tokens: UInt64
    let seconds: Float

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 8) {
                Image(systemName: symbol)
                    .foregroundStyle(tint)

                Text(headline)
                    .font(.subheadline.weight(.semibold))
                    .foregroundStyle(tint)
            }

            Text("\(steps) steps · \(compact(tokens)) tokens · \(String(format: "%.1f", seconds)) s")
                .font(.caption.monospacedDigit())
                .foregroundStyle(Theme.textMuted)

            Divider().overlay(Theme.border)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(.top, 10)
    }

    private var tint: Color {
        switch tone {
        case .good: Theme.good
        case .warning: Theme.warning
        case .bad: Theme.bad
        }
    }

    private var symbol: String {
        switch tone {
        case .good: "checkmark.circle.fill"
        case .warning: "exclamationmark.circle.fill"
        case .bad: "xmark.circle.fill"
        }
    }
}

/// A count in few characters: 6.3k, 1.2M.
func compact(_ number: UInt64) -> String {
    switch number {
    case 1_000_000...: String(format: "%.1fM", Double(number) / 1_000_000)
    case 1_000...: String(format: "%.1fk", Double(number) / 1_000)
    default: "\(number)"
    }
}

#if os(iOS)
typealias PlatformImage = UIImage
#else
typealias PlatformImage = NSImage
#endif

extension Image {
    init(platform image: PlatformImage) {
        #if os(iOS)
        self.init(uiImage: image)
        #else
        self.init(nsImage: image)
        #endif
    }
}

extension ItemView: Identifiable {
    public var id: UInt64 {
        switch self {
        case .user(let id, _, _), .briefing(let id, _, _), .assistant(let id, _, _, _, _),
             .tools(let id, _), .checks(let id, _, _), .review(let id, _, _),
             .notice(let id, _, _), .summary(let id, _, _, _, _, _):
            id
        }
    }
}
