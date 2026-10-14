import SwiftUI

/// The first screen: what the app is, and how to pair it.
struct WelcomeScreen: View {
    @Environment(AppModel.self) private var model

    @State private var pairing = false
    @State private var appeared = false

    var body: some View {
        VStack(spacing: 0) {
            Spacer(minLength: 24)

            LogoBadge(size: 108, alive: true)
                .scaleEffect(appeared ? 1 : 0.8)
                .opacity(appeared ? 1 : 0)

            Text("Cortex")
                .font(Theme.title(40))
                .foregroundStyle(Theme.text)
                .padding(.top, 28)

            Text("Your coding agent, from wherever you are.")
                .font(.body)
                .foregroundStyle(Theme.textSecondary)
                .multilineTextAlignment(.center)
                .padding(.top, 6)

            VStack(alignment: .leading, spacing: 18) {
                Step(number: 1, text: "Open Cortex on your computer.")
                Step(number: 2, text: "Go to Settings, then Remote control, and switch it on.")
                Step(number: 3, text: "Show a pairing code and scan it here.")
            }
            .card(padding: 18)
            .padding(.top, 36)
            .opacity(appeared ? 1 : 0)
            .offset(y: appeared ? 0 : 16)

            Spacer(minLength: 24)

            ConnectionNote(connection: model.state.connection)
                .padding(.bottom, 12)

            Button {
                Haptics.tap()
                model.core.usePhone()
            } label: {
                Label("Work on this phone", systemImage: "iphone")
            }
            .buttonStyle(PrimaryButtonStyle())

            Button {
                Haptics.tap()
                pairing = true
            } label: {
                Label("Pair with a computer", systemImage: "qrcode.viewfinder")
            }
            .buttonStyle(SecondaryButtonStyle())
            .padding(.top, 10)

            Text("On this phone the agent, its memory and its networks run here. Paired, they stay on your computer and the connection is encrypted end to end.")
                .font(.footnote)
                .foregroundStyle(Theme.textMuted)
                .multilineTextAlignment(.center)
                .padding(.top, 14)
        }
        .padding(.horizontal, 24)
        .padding(.bottom, 20)
        .frame(maxWidth: 520)
        .frame(maxWidth: .infinity)
        .sheet(isPresented: $pairing) {
            PairingSheet()
                .macSheet()
        }
        .onAppear {
            withAnimation(Theme.spring.delay(0.1)) {
                appeared = true
            }
        }
    }
}

private struct Step: View {
    let number: Int
    let text: String

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 14) {
            Text("\(number)")
                .font(.footnote.weight(.bold).monospacedDigit())
                .foregroundStyle(Theme.accent)
                .frame(width: 24, height: 24)
                .background(Theme.accent.opacity(0.14), in: Circle())

            Text(text)
                .font(.callout)
                .foregroundStyle(Theme.text)
                .fixedSize(horizontal: false, vertical: true)
        }
    }
}

/// Says how connecting is going, when there is something to say.
struct ConnectionNote: View {
    let connection: Connection

    var body: some View {
        Group {
            switch connection {
            case .connecting(let host):
                Label {
                    Text("Connecting to \(host)…")
                } icon: {
                    ProgressView().controlSize(.small)
                }
                .foregroundStyle(Theme.textSecondary)

            case .reconnecting(let host, let reason):
                VStack(spacing: 4) {
                    Label {
                        Text("Trying to reach \(host)…")
                    } icon: {
                        ProgressView().controlSize(.small)
                    }
                    .foregroundStyle(Theme.textSecondary)

                    Text(reason)
                        .font(.caption)
                        .foregroundStyle(Theme.textMuted)
                        .multilineTextAlignment(.center)
                }

            case .failed(let reason):
                Label(reason, systemImage: "exclamationmark.triangle.fill")
                    .foregroundStyle(Theme.bad)
                    .multilineTextAlignment(.leading)

            case .idle, .connected:
                EmptyView()
            }
        }
        .font(.footnote)
        .transition(.opacity)
    }
}

/// Scanning the code on the computer's screen, or pasting it as text.
struct PairingSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss

    @State private var typed = ""
    @State private var typing = false

    /// Hosts known when the sheet opened: one more means it worked.
    @State private var known = 0

    var body: some View {
        NavigationStack {
            VStack(spacing: 20) {
                if typing {
                    VStack(alignment: .leading, spacing: 10) {
                        Text("On the computer, choose “Copy as text” under the pairing code, send it to this phone and paste it here.")
                            .font(.callout)
                            .foregroundStyle(Theme.textSecondary)

                        TextField("cortex-pair:…", text: $typed, axis: .vertical)
                            .font(Theme.mono)
                            .lineLimit(4...8)
                            .autocorrectionDisabled()
                        #if os(iOS)
                            .textInputAutocapitalization(.never)
                        #endif
                            .padding(12)
                            .background(Theme.sunken, in: RoundedRectangle(cornerRadius: Theme.smallRadius))

                        Button("Pair") {
                            pair(typed)
                        }
                        .buttonStyle(PrimaryButtonStyle())
                        .disabled(typed.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                    }
                } else {
                    Text("Point the camera at the code on your computer's screen.")
                        .font(.callout)
                        .foregroundStyle(Theme.textSecondary)
                        .multilineTextAlignment(.center)

                    ScannerFrame { code in
                        pair(code)
                    }
                }

                ConnectionNote(connection: model.state.connection)

                Spacer(minLength: 0)

                Button(typing ? "Scan the code instead" : "Enter the code as text") {
                    withAnimation(Theme.spring) {
                        typing.toggle()
                    }
                }
                .font(.callout.weight(.medium))
            }
            .padding(24)
            .background(Theme.background.ignoresSafeArea())
            .navigationTitle("Pair with a computer")
            #if os(iOS)
            .navigationBarTitleDisplayMode(.inline)
            #endif
            .toolbar {
                ToolbarItem(placement: .cancellationAction) {
                    Button("Cancel") { dismiss() }
                }
            }
        }
        .onAppear {
            known = model.state.hosts.count

            #if os(macOS)
            typing = true
            #endif
        }
        .onChange(of: model.state.hosts.count) { _, count in
            if count > known {
                Haptics.success()
                dismiss()
            }
        }
        .onChange(of: model.connected) { _, connected in
            if connected {
                Haptics.success()
                dismiss()
            }
        }
    }

    private func pair(_ code: String) {
        let code = code.trimmingCharacters(in: .whitespacesAndNewlines)

        guard code.hasPrefix("cortex-pair:") else { return }

        Haptics.tap()
        model.core.pair(code: code)
    }
}

/// The camera's picture, in a frame, with a line that sweeps it.
private struct ScannerFrame: View {
    let found: (String) -> Void

    @State private var sweep = false

    var body: some View {
        ZStack {
            #if os(iOS)
            CodeScanner(found: found)
            #else
            Theme.sunken
            #endif

            RoundedRectangle(cornerRadius: 28, style: .continuous)
                .strokeBorder(Theme.accent, lineWidth: 3)

            GeometryReader { proxy in
                LinearGradient(
                    colors: [.clear, Theme.accent.opacity(0.55), .clear],
                    startPoint: .top,
                    endPoint: .bottom
                )
                .frame(height: 70)
                .offset(y: sweep ? proxy.size.height - 70 : 0)
            }
            .allowsHitTesting(false)
        }
        .aspectRatio(1, contentMode: .fit)
        .clipShape(RoundedRectangle(cornerRadius: 28, style: .continuous))
        .onAppear {
            withAnimation(.easeInOut(duration: 1.8).repeatForever(autoreverses: true)) {
                sweep = true
            }
        }
    }
}
