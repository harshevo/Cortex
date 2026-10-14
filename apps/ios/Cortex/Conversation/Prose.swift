import SwiftUI

/// A model's text: paragraphs, headings, lists and code, as it wrote
/// them in Markdown.
struct Prose: View {
    let text: String

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            ForEach(Array(Block.parse(text).enumerated()), id: \.offset) { _, block in
                switch block {
                case .paragraph(let text):
                    Text(inline(text))
                        .font(.body)
                        .foregroundStyle(Theme.text)
                        .lineSpacing(3)
                        .fixedSize(horizontal: false, vertical: true)

                case .heading(let text):
                    Text(inline(text))
                        .font(.headline)
                        .foregroundStyle(Theme.text)
                        .padding(.top, 4)

                case .item(let marker, let text):
                    HStack(alignment: .firstTextBaseline, spacing: 8) {
                        Text(marker)
                            .font(.body.monospacedDigit())
                            .foregroundStyle(Theme.textMuted)

                        Text(inline(text))
                            .font(.body)
                            .foregroundStyle(Theme.text)
                            .lineSpacing(3)
                            .fixedSize(horizontal: false, vertical: true)
                    }

                case .code(let code):
                    CodeBlock(code: code)
                }
            }
        }
        .textSelection(.enabled)
    }

    /// Bold, italics, links and code inside a line.
    private func inline(_ text: String) -> AttributedString {
        let options = AttributedString.MarkdownParsingOptions(
            interpretedSyntax: .inlineOnlyPreservingWhitespace
        )

        guard var styled = try? AttributedString(markdown: text, options: options) else {
            return AttributedString(text)
        }

        for run in styled.runs where run.inlinePresentationIntent?.contains(.code) == true {
            styled[run.range].font = .system(.callout, design: .monospaced)
            styled[run.range].foregroundColor = Theme.code
        }

        return styled
    }
}

/// Code, in its own frame, scrolled sideways rather than wrapped.
struct CodeBlock: View {
    let code: String

    @State private var copied = false

    var body: some View {
        ZStack(alignment: .topTrailing) {
            ScrollView(.horizontal, showsIndicators: false) {
                Text(code)
                    .font(Theme.mono)
                    .foregroundStyle(Theme.text)
                    .padding(12)
                    .padding(.trailing, 36)
            }

            Button {
                copy(code)
                Haptics.tap()

                withAnimation(Theme.quick) { copied = true }

                Task {
                    try? await Task.sleep(for: .seconds(1.4))

                    withAnimation(Theme.quick) { copied = false }
                }
            } label: {
                Image(systemName: copied ? "checkmark" : "doc.on.doc")
                    .font(.caption.weight(.semibold))
                    .foregroundStyle(copied ? Theme.good : Theme.textSecondary)
                    .frame(width: 32, height: 32)
                    .contentTransition(.symbolEffect(.replace))
            }
            .buttonStyle(.plain)
            .accessibilityLabel("Copy")
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .background(Theme.sunken, in: RoundedRectangle(cornerRadius: Theme.smallRadius, style: .continuous))
        .overlay(
            RoundedRectangle(cornerRadius: Theme.smallRadius, style: .continuous)
                .strokeBorder(Theme.border.opacity(0.6), lineWidth: 1)
        )
    }
}

func copy(_ text: String) {
    #if os(iOS)
    UIPasteboard.general.string = text
    #else
    NSPasteboard.general.clearContents()
    NSPasteboard.general.setString(text, forType: .string)
    #endif
}

private enum Block {
    case paragraph(String)
    case heading(String)
    case item(String, String)
    case code(String)

    static func parse(_ text: String) -> [Block] {
        var blocks: [Block] = []
        var paragraph: [String] = []
        var code: [String]?

        func endParagraph() {
            if !paragraph.isEmpty {
                blocks.append(.paragraph(paragraph.joined(separator: "\n")))
                paragraph = []
            }
        }

        for line in text.components(separatedBy: "\n") {
            let trimmed = line.trimmingCharacters(in: .whitespaces)

            if trimmed.hasPrefix("```") {
                if let open = code {
                    blocks.append(.code(open.joined(separator: "\n")))
                    code = nil
                } else {
                    endParagraph()
                    code = []
                }

                continue
            }

            if code != nil {
                code?.append(line)

                continue
            }

            if trimmed.isEmpty {
                endParagraph()
            } else if trimmed.hasPrefix("#") {
                endParagraph()
                blocks.append(.heading(String(trimmed.drop { $0 == "#" || $0 == " " })))
            } else if trimmed.hasPrefix("- ") || trimmed.hasPrefix("* ") {
                endParagraph()
                blocks.append(.item("•", String(trimmed.dropFirst(2))))
            } else if let dot = trimmed.firstIndex(of: "."),
                      trimmed[..<dot].allSatisfy(\.isNumber),
                      !trimmed[..<dot].isEmpty,
                      trimmed[dot...].hasPrefix(". ") {
                endParagraph()
                blocks.append(.item(String(trimmed[...dot]), String(trimmed[dot...].dropFirst(2))))
            } else {
                paragraph.append(line)
            }
        }

        // A block still being written has no closing fence yet.
        if let open = code {
            blocks.append(.code(open.joined(separator: "\n")))
        }

        endParagraph()

        return blocks
    }
}
