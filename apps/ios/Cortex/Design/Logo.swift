import SwiftUI

/// The Cortex mark: an open ring with a signal leaving it.
///
/// Drawn in a unit square so that the same numbers give the app icon,
/// the mark inside the app, and the Android drawable.
struct LogoMark: View {
    /// The colour of the mark itself.
    var color: Color = Theme.onAccent

    /// How far the signal has travelled from the centre to the gap, for
    /// the mark while it is waiting. One is where it rests.
    var signal: CGFloat = 1

    var body: some View {
        Canvas { context, size in
            let side = min(size.width, size.height)
            let centre = CGPoint(x: size.width / 2, y: size.height / 2)
            let radius = side * 0.27

            // The ring, open to the right.
            var ring = Path()

            ring.addArc(
                center: centre,
                radius: radius,
                startAngle: .degrees(42),
                endAngle: .degrees(318),
                clockwise: false
            )

            context.stroke(
                ring,
                with: .color(color),
                style: StrokeStyle(lineWidth: side * 0.085, lineCap: .round)
            )

            // The path the signal takes, and the signal on it.
            let gap = CGPoint(x: centre.x + radius, y: centre.y)

            var axon = Path()

            axon.move(to: centre)
            axon.addLine(to: gap)

            context.stroke(
                axon,
                with: .color(color.opacity(0.55)),
                style: StrokeStyle(lineWidth: side * 0.034, lineCap: .round)
            )

            let core = side * 0.068

            context.fill(
                Path(ellipseIn: CGRect(x: centre.x - core, y: centre.y - core, width: core * 2, height: core * 2)),
                with: .color(color)
            )

            let spark = side * 0.05
            let at = CGPoint(x: centre.x + radius * signal, y: centre.y)

            context.fill(
                Path(ellipseIn: CGRect(x: at.x - spark, y: at.y - spark, width: spark * 2, height: spark * 2)),
                with: .color(color)
            )
        }
        .aspectRatio(1, contentMode: .fit)
    }
}

/// The mark on its ground, as the app icon has it.
struct LogoBadge: View {
    var size: CGFloat = 96

    /// The signal travels out again and again.
    var alive = false

    @State private var signal: CGFloat = 1

    var body: some View {
        ZStack {
            RoundedRectangle(cornerRadius: size * 0.225, style: .continuous)
                .fill(
                    LinearGradient(
                        colors: [Color(hex: 0xE58E6A), Color(hex: 0xC25F3C)],
                        startPoint: .topLeading,
                        endPoint: .bottomTrailing
                    )
                )

            LogoMark(color: Color(hex: 0xFFF6EC), signal: signal)
        }
        .frame(width: size, height: size)
        .shadow(color: Color(hex: 0xC25F3C).opacity(0.35), radius: size * 0.18, y: size * 0.08)
        .onAppear {
            guard alive else { return }

            signal = 0

            withAnimation(.easeInOut(duration: 1.5).repeatForever(autoreverses: false)) {
                signal = 1
            }
        }
    }
}

/// The app icon, full bleed: the system rounds its corners itself.
struct AppIconArt: View {
    var body: some View {
        ZStack {
            LinearGradient(
                colors: [Color(hex: 0xE58E6A), Color(hex: 0xC25F3C)],
                startPoint: .topLeading,
                endPoint: .bottomTrailing
            )

            LogoMark(color: Color(hex: 0xFFF6EC))
        }
    }
}
