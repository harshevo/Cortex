import SwiftUI

/// The app's colours, type and spacing. Everything on screen takes them
/// from here, so the two themes and the Android app stay in step.
enum Theme {
    // Grounds, darkest to lightest in the dark theme.
    static let background = Color(light: 0xFAF8F3, dark: 0x1F1E1D)
    static let sunken = Color(light: 0xF1EEE6, dark: 0x171615)
    static let surface = Color(light: 0xFFFFFF, dark: 0x292826)
    static let raised = Color(light: 0xF1EEE6, dark: 0x343330)
    static let border = Color(light: 0xE4E0D6, dark: 0x3A3936)

    static let text = Color(light: 0x1F1E1D, dark: 0xECE9E1)
    static let textSecondary = Color(light: 0x5E5B54, dark: 0xA9A69D)
    static let textMuted = Color(light: 0x8A867C, dark: 0x96938A)

    /// Code set inside a sentence.
    static let code = Color(light: 0x9A5B2A, dark: 0xE6C9A8)

    /// What the agent does, and what the person can do next.
    static let accent = Color(light: 0xC96442, dark: 0xD97757)

    static let good = Color(light: 0x2E9E6E, dark: 0x5DCA9A)
    static let bad = Color(light: 0xD0453C, dark: 0xE5675F)
    static let warning = Color(light: 0xB7791F, dark: 0xE2AB52)

    /// A model response.
    static let model = Color(light: 0x6A5FD6, dark: 0x8F87E8)

    /// Text on the accent colour.
    static let onAccent = Color(hex: 0xFFF8F2)

    // Spacing, on a grid of four.
    static let gutter: CGFloat = 16
    static let gap: CGFloat = 12
    static let tight: CGFloat = 8

    static let radius: CGFloat = 16
    static let smallRadius: CGFloat = 10

    static let spring = Animation.spring(response: 0.38, dampingFraction: 0.82)
    static let quick = Animation.easeOut(duration: 0.18)

    /// Titles are set in a serif, as the desktop app's are.
    static func title(_ size: CGFloat) -> Font {
        .system(size: size, weight: .semibold, design: .serif)
    }

    static let mono = Font.system(.footnote, design: .monospaced)
    static let monoSmall = Font.system(.caption2, design: .monospaced)
}

extension Color {
    init(hex: UInt32) {
        self.init(
            .sRGB,
            red: Double((hex >> 16) & 0xFF) / 255,
            green: Double((hex >> 8) & 0xFF) / 255,
            blue: Double(hex & 0xFF) / 255,
            opacity: 1
        )
    }

    /// One colour for each appearance.
    init(light: UInt32, dark: UInt32) {
        #if os(iOS)
        self.init(uiColor: UIColor { traits in
            UIColor(Color(hex: traits.userInterfaceStyle == .dark ? dark : light))
        })
        #else
        self.init(nsColor: NSColor(name: nil) { appearance in
            let isDark = appearance.bestMatch(from: [.darkAqua, .aqua]) == .darkAqua

            return NSColor(Color(hex: isDark ? dark : light))
        })
        #endif
    }
}

/// A touch the hand can feel, where the phone has the means.
enum Haptics {
    static func tap() {
        #if os(iOS)
        UIImpactFeedbackGenerator(style: .light).impactOccurred()
        #endif
    }

    static func success() {
        #if os(iOS)
        UINotificationFeedbackGenerator().notificationOccurred(.success)
        #endif
    }

    static func warning() {
        #if os(iOS)
        UINotificationFeedbackGenerator().notificationOccurred(.warning)
        #endif
    }
}

/// A card: the ground most things sit on.
struct Card: ViewModifier {
    var padding: CGFloat = 14

    func body(content: Content) -> some View {
        content
            .padding(padding)
            .background(Theme.surface, in: RoundedRectangle(cornerRadius: Theme.radius, style: .continuous))
            .overlay(
                RoundedRectangle(cornerRadius: Theme.radius, style: .continuous)
                    .strokeBorder(Theme.border.opacity(0.7), lineWidth: 1)
            )
    }
}

extension View {
    func card(padding: CGFloat = 14) -> some View {
        modifier(Card(padding: padding))
    }

    /// On a Mac a sheet is as small as its contents allow, which for a
    /// scrolling screen is nothing. A phone's sheet needs no size.
    func macSheet() -> some View {
        #if os(macOS)
        frame(width: 393, height: 780)
        #else
        self
        #endif
    }

    /// A small heading above a group.
    func sectionHeading() -> some View {
        font(.footnote.weight(.semibold))
            .textCase(.uppercase)
            .kerning(0.6)
            .foregroundStyle(Theme.textMuted)
    }
}

/// The app's main button.
struct PrimaryButtonStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.body.weight(.semibold))
            .foregroundStyle(Theme.onAccent)
            .frame(maxWidth: .infinity, minHeight: 52)
            .background(Theme.accent, in: RoundedRectangle(cornerRadius: Theme.radius, style: .continuous))
            .opacity(configuration.isPressed ? 0.85 : 1)
            .scaleEffect(configuration.isPressed ? 0.98 : 1)
            .animation(Theme.quick, value: configuration.isPressed)
    }
}

struct SecondaryButtonStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(.body.weight(.medium))
            .foregroundStyle(Theme.text)
            .frame(maxWidth: .infinity, minHeight: 52)
            .background(Theme.surface, in: RoundedRectangle(cornerRadius: Theme.radius, style: .continuous))
            .overlay(
                RoundedRectangle(cornerRadius: Theme.radius, style: .continuous)
                    .strokeBorder(Theme.border, lineWidth: 1)
            )
            .opacity(configuration.isPressed ? 0.7 : 1)
            .animation(Theme.quick, value: configuration.isPressed)
    }
}

/// A light that sweeps across something that is waiting.
struct Shimmer: ViewModifier {
    @State private var phase: CGFloat = -1

    func body(content: Content) -> some View {
        content
            .overlay(
                GeometryReader { proxy in
                    LinearGradient(
                        colors: [.clear, Theme.text.opacity(0.35), .clear],
                        startPoint: .leading,
                        endPoint: .trailing
                    )
                    .frame(width: proxy.size.width * 0.6)
                    .offset(x: phase * proxy.size.width * 1.4)
                    .blendMode(.plusLighter)
                }
                .mask(content)
                .allowsHitTesting(false)
            )
            .onAppear {
                withAnimation(.linear(duration: 1.6).repeatForever(autoreverses: false)) {
                    phase = 1
                }
            }
    }
}

extension View {
    func shimmer() -> some View {
        modifier(Shimmer())
    }
}
