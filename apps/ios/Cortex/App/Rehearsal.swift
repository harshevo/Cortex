import SwiftUI

/// Driving the app without a person, to try it and photograph it.
///
///     --pair-file <path>    pair with the code in this file
///     --phone               work with the agent on the phone instead
///     --submit <text>       send this to the first chat once it is ready
///     --allow               allow whatever the agent asks for
///     --icon <path>         write the app icon as a PNG and quit
///     --shots <folder>      photograph each screen along the way, then quit
///
/// None of it runs unless asked for on the command line.
enum Rehearsal {
    #if os(macOS)
    /// The window a rehearsal opened for itself, kept so it stays open.
    @MainActor private static var stage: NSWindow?
    #endif

    @MainActor
    static func run(_ model: AppModel) async {
        let arguments = CommandLine.arguments

        func value(_ flag: String) -> String? {
            guard let index = arguments.firstIndex(of: flag), index + 1 < arguments.count else {
                return nil
            }

            return arguments[index + 1]
        }

        if let path = value("--icon") {
            writeIcon(to: path)

            exit(0)
        }

        let onPhone = arguments.contains("--phone")

        guard let file = value("--pair-file") ?? (onPhone ? "" : nil) else { return }

        let shots = value("--shots")

        // Started behind another app, a Mac app opens no window until it
        // is brought forward.
        #if os(macOS)
        try? await Task.sleep(for: .milliseconds(600))

        NSApp.setActivationPolicy(.regular)
        NSApp.activate(ignoringOtherApps: true)

        try? await Task.sleep(for: .milliseconds(600))

        // Still none: the rehearsal opens one of its own.
        if NSApp.windows.isEmpty, shots != nil {
            let window = NSWindow(
                contentRect: NSRect(x: 80, y: 80, width: 393, height: 852),
                styleMask: [.titled],
                backing: .buffered,
                defer: false
            )

            window.contentView = NSHostingView(
                rootView: RootView()
                    .environment(model)
                    .tint(Theme.accent)
                    .frame(width: 393, height: 852)
            )

            window.isReleasedWhenClosed = false
            window.orderFrontRegardless()

            stage = window
        }
        #endif

        try? await Task.sleep(for: .seconds(1.2))

        shoot("1-welcome", into: shots)

        if onPhone {
            model.core.usePhone()

            try? await Task.sleep(for: .seconds(2.5))

            shoot("on-phone-home", into: shots)

            model.stage = .phone

            try? await Task.sleep(for: .seconds(1.5))

            shoot("this-phone", into: shots)

            if shots != nil {
                exit(0)
            }

            return
        }

        // The desktop writes the code once it is listening.
        var code: String?

        for _ in 0..<100 where code == nil {
            code = try? String(contentsOfFile: file, encoding: .utf8)

            if code == nil {
                try? await Task.sleep(for: .milliseconds(200))
            }
        }

        guard let code else { return }

        model.core.pair(code: code)

        let instruction = value("--submit")
        let allow = arguments.contains("--allow")

        var sent = false
        var askedOnce = false
        var ticks = 0

        while true {
            try? await Task.sleep(for: .milliseconds(150))

            guard let chat = model.state.chats.first else { continue }

            if let instruction, chat.ready, !sent {
                sent = true

                try? await Task.sleep(for: .seconds(1))

                shoot("2-home", into: shots)

                model.core.submit(chat: chat.id, text: instruction, pictures: [])
                model.stage = .chat
            }

            guard sent else { continue }

            ticks += 1

            if ticks == 12 {
                shoot("3-working", into: shots)
            }

            if let approval = model.live[chat.id]?.approvals.first {
                if !askedOnce {
                    askedOnce = true

                    try? await Task.sleep(for: .seconds(1))

                    shoot("4-question", into: shots)
                }

                if allow {
                    model.core.answer(chat: chat.id, id: approval.id, answer: .allow)
                }
            }

            let done = model.items[chat.id]?.contains { item in
                if case .summary = item { return true }

                return false
            } ?? false

            if done, shots != nil {
                try? await Task.sleep(for: .seconds(1.5))

                shoot("5-done", into: shots)

                model.stage = .capabilities

                try? await Task.sleep(for: .seconds(2))

                shoot("6-capabilities", into: shots)

                model.stage = .memory

                try? await Task.sleep(for: .seconds(4))

                shoot("7-memory", into: shots)

                exit(0)
            }
        }
    }

    /// Saves a picture of every window. Only the Mac window can do it.
    @MainActor
    private static func shoot(_ name: String, into folder: String?) {
        #if os(macOS)
        guard let folder else { return }

        try? FileManager.default.createDirectory(atPath: folder, withIntermediateDirectories: true)

        FileHandle.standardError.write(Data("shot \(name): \(NSApp.windows.count) windows\n".utf8))

        for (index, window) in NSApp.windows.enumerated() {
            guard
                window.isVisible,
                let view = window.contentView,
                let bitmap = view.bitmapImageRepForCachingDisplay(in: view.bounds)
            else { continue }

            view.cacheDisplay(in: view.bounds, to: bitmap)

            let suffix = index == 0 ? "" : "-\(index)"

            try? bitmap
                .representation(using: .png, properties: [:])?
                .write(to: URL(fileURLWithPath: "\(folder)/\(name)\(suffix).png"))
        }
        #endif
    }

    @MainActor
    private static func writeIcon(to path: String) {
        let renderer = ImageRenderer(content: AppIconArt().frame(width: 1024, height: 1024))

        renderer.scale = 1

        #if os(macOS)
        guard
            let image = renderer.nsImage,
            let tiff = image.tiffRepresentation,
            let bitmap = NSBitmapImageRep(data: tiff),
            let png = bitmap.representation(using: .png, properties: [:])
        else { return }
        #else
        guard let png = renderer.uiImage?.pngData() else { return }
        #endif

        try? png.write(to: URL(fileURLWithPath: path))
    }
}
