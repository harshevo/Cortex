import PhotosUI
import SwiftUI

/// Where an instruction is written and sent, and a run is stopped.
struct Composer: View {
    @Environment(AppModel.self) private var model

    let chat: String
    let live: LiveView
    let ready: Bool

    @State private var draft = ""
    @State private var picked: [PhotosPickerItem] = []
    @State private var pictures: [Picture] = []
    @FocusState private var focused: Bool

    private var hasText: Bool {
        !draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            if live.canContinue && !live.running {
                Button {
                    Haptics.tap()
                    model.core.carryOn(chat: chat)
                } label: {
                    Label("Continue where it stopped", systemImage: "arrow.clockwise")
                        .font(.subheadline.weight(.medium))
                        .foregroundStyle(Theme.accent)
                        .padding(.horizontal, 12)
                        .frame(minHeight: 36)
                        .background(Theme.accent.opacity(0.12), in: Capsule())
                }
                .buttonStyle(PressableStyle())
                .transition(.opacity.combined(with: .move(edge: .bottom)))
            }

            // What the mind expects to be asked next.
            if let suggestion = live.suggestion, draft.isEmpty, !live.running {
                Button {
                    Haptics.tap()
                    draft = suggestion
                    focused = true
                } label: {
                    HStack(spacing: 6) {
                        Image(systemName: "sparkles")
                            .foregroundStyle(Theme.model)

                        Text(suggestion)
                            .foregroundStyle(Theme.textSecondary)
                            .lineLimit(1)
                    }
                    .font(.footnote)
                    .padding(.horizontal, 12)
                    .frame(minHeight: 34)
                    .background(Theme.surface, in: Capsule())
                    .overlay(Capsule().strokeBorder(Theme.border, lineWidth: 1))
                }
                .buttonStyle(PressableStyle())
                .transition(.opacity.combined(with: .move(edge: .bottom)))
            }

            if !pictures.isEmpty {
                ScrollView(.horizontal, showsIndicators: false) {
                    HStack(spacing: 8) {
                        ForEach(Array(pictures.enumerated()), id: \.offset) { index, picture in
                            if let image = PlatformImage(data: picture.data) {
                                Image(platform: image)
                                    .resizable()
                                    .scaledToFill()
                                    .frame(width: 56, height: 56)
                                    .clipShape(RoundedRectangle(cornerRadius: 10, style: .continuous))
                                    .overlay(alignment: .topTrailing) {
                                        Button {
                                            pictures.remove(at: index)
                                            picked.remove(at: min(index, picked.count - 1))
                                        } label: {
                                            Image(systemName: "xmark.circle.fill")
                                                .symbolRenderingMode(.palette)
                                                .foregroundStyle(.white, .black.opacity(0.6))
                                        }
                                        .offset(x: 6, y: -6)
                                    }
                            }
                        }
                    }
                    .padding(.top, 6)
                }
            }

            HStack(alignment: .bottom, spacing: 8) {
                PhotosPicker(selection: $picked, maxSelectionCount: 4, matching: .images) {
                    Image(systemName: "plus")
                        .font(.system(size: 17, weight: .medium))
                        .foregroundStyle(Theme.textSecondary)
                        .frame(width: 36, height: 36)
                }
                .buttonStyle(.plain)
                .disabled(live.running)

                TextField(placeholder, text: $draft, axis: .vertical)
                    .lineLimit(1...6)
                    .font(.body)
                    .foregroundStyle(Theme.text)
                    .focused($focused)
                    .padding(.vertical, 8)
                    .textFieldStyle(.plain)

                SendButton(mode: mode) {
                    act()
                }
            }
            .padding(.horizontal, 8)
            .padding(.vertical, 5)
            .background(Theme.surface, in: RoundedRectangle(cornerRadius: 24, style: .continuous))
            .overlay(
                RoundedRectangle(cornerRadius: 24, style: .continuous)
                    .strokeBorder(focused ? Theme.accent.opacity(0.6) : Theme.border, lineWidth: 1)
            )
            .animation(Theme.quick, value: focused)
        }
        .animation(Theme.spring, value: live.suggestion)
        .animation(Theme.spring, value: live.canContinue)
        .onChange(of: picked) { _, items in
            Task { await load(items) }
        }
    }

    private var placeholder: String {
        if !ready { return "Cortex is starting…" }

        return live.running ? "Tell Cortex something while it works" : "Ask Cortex to change something"
    }

    private var mode: SendButton.Mode {
        if live.running {
            return hasText ? .steer : .stop
        }

        return hasText && ready ? .send : .idle
    }

    private func act() {
        let text = draft.trimmingCharacters(in: .whitespacesAndNewlines)

        switch mode {
        case .send:
            Haptics.tap()
            model.core.submit(chat: chat, text: text, pictures: pictures)

            draft = ""
            pictures = []
            picked = []

        case .steer:
            Haptics.tap()
            model.core.steer(chat: chat, text: text)

            draft = ""

        case .stop:
            Haptics.warning()
            model.core.stop(chat: chat)

        case .idle:
            break
        }
    }

    /// Reads the chosen pictures, made small enough to send.
    private func load(_ items: [PhotosPickerItem]) async {
        var loaded: [Picture] = []

        for item in items {
            guard let data = try? await item.loadTransferable(type: Data.self) else { continue }

            if let small = shrink(data) {
                loaded.append(Picture(mediaType: "image/jpeg", data: small))
            }
        }

        pictures = loaded
    }

    /// At most 1600 points on its longer side, as JPEG.
    private func shrink(_ data: Data) -> Data? {
        #if os(iOS)
        guard let image = UIImage(data: data) else { return nil }

        let longest = max(image.size.width, image.size.height)
        let scale = min(1, 1600 / longest)
        let size = CGSize(width: image.size.width * scale, height: image.size.height * scale)

        let format = UIGraphicsImageRendererFormat()

        format.scale = 1

        let drawn = UIGraphicsImageRenderer(size: size, format: format).image { _ in
            image.draw(in: CGRect(origin: .zero, size: size))
        }

        return drawn.jpegData(compressionQuality: 0.8)
        #else
        guard
            let image = NSImage(data: data),
            let tiff = image.tiffRepresentation,
            let bitmap = NSBitmapImageRep(data: tiff)
        else { return nil }

        return bitmap.representation(using: .jpeg, properties: [.compressionFactor: 0.8])
        #endif
    }
}

/// One button that sends, steers or stops, and changes shape to say which.
private struct SendButton: View {
    enum Mode {
        case idle, send, steer, stop
    }

    let mode: Mode
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            ZStack {
                Circle()
                    .fill(fill)
                    .frame(width: 36, height: 36)

                Image(systemName: symbol)
                    .font(.system(size: 15, weight: .bold))
                    .foregroundStyle(mode == .idle ? Theme.textMuted : Theme.onAccent)
                    .contentTransition(.symbolEffect(.replace))
            }
        }
        .buttonStyle(PressableStyle())
        .disabled(mode == .idle)
        .animation(Theme.spring, value: mode)
        .accessibilityLabel(label)
    }

    private var symbol: String {
        switch mode {
        case .idle, .send: "arrow.up"
        case .steer: "arrow.turn.down.right"
        case .stop: "stop.fill"
        }
    }

    private var fill: Color {
        switch mode {
        case .idle: Theme.raised
        case .send, .steer: Theme.accent
        case .stop: Theme.bad
        }
    }

    private var label: String {
        switch mode {
        case .idle, .send: "Send"
        case .steer: "Tell the run"
        case .stop: "Stop"
        }
    }
}
