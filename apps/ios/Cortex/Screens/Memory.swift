import SwiftUI

/// What the agent remembers, drawn as dots that settle near what they
/// are tied to. Touch one to read it.
struct MemorySheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss

    let chat: String

    @State private var layout = Layout()
    @State private var selected: Int?

    private var memory: MemoryView {
        model.memory[chat] ?? model.core.memory(chat: chat)
    }

    var body: some View {
        NavigationStack {
            ZStack(alignment: .bottom) {
                Theme.sunken.ignoresSafeArea()

                if memory.nodes.isEmpty {
                    VStack(spacing: 10) {
                        Image(systemName: "brain")
                            .font(.system(size: 40))
                            .foregroundStyle(Theme.textMuted)

                        Text("Nothing remembered yet")
                            .font(Theme.title(20))
                            .foregroundStyle(Theme.text)

                        Text("Tasks done in this project appear here, joined to the ones they resemble.")
                            .font(.callout)
                            .foregroundStyle(Theme.textSecondary)
                            .multilineTextAlignment(.center)
                    }
                    .padding(32)
                    .frame(maxHeight: .infinity)
                } else {
                    GeometryReader { proxy in
                        TimelineView(.animation) { timeline in
                            Canvas { context, size in
                                layout.step(memory: memory, size: size)

                                draw(&context, at: timeline.date.timeIntervalSinceReferenceDate)
                            }
                        }
                        .contentShape(Rectangle())
                        .onTapGesture { point in
                            withAnimation(Theme.spring) {
                                selected = layout.nearest(to: point, within: 28)
                            }

                            if selected != nil { Haptics.tap() }
                        }
                        .onAppear {
                            layout.seed(memory: memory, size: proxy.size)
                        }
                    }

                    VStack(spacing: 10) {
                        if let selected, selected < memory.nodes.count {
                            NodeCard(node: memory.nodes[selected])
                                .transition(.move(edge: .bottom).combined(with: .opacity))
                        }

                        Legend(memory: memory)
                    }
                    .padding(Theme.gutter)
                }
            }
            .navigationTitle("Memory")
            #if os(iOS)
            .navigationBarTitleDisplayMode(.inline)
            #endif
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                }
            }
        }
    }

    private func draw(_ context: inout GraphicsContext, at time: Double) {
        let points = layout.points

        guard points.count == memory.nodes.count else { return }

        let tied: Set<Int> = selected.map { chosen in
            Set(memory.links.compactMap { link -> Int? in
                if Int(link.from) == chosen { return Int(link.to) }
                if Int(link.to) == chosen { return Int(link.from) }

                return nil
            })
        } ?? []

        for link in memory.links {
            let from = Int(link.from), to = Int(link.to)

            guard from < points.count, to < points.count else { continue }

            let lit = selected == from || selected == to

            var line = Path()

            line.move(to: points[from])
            line.addLine(to: points[to])

            context.stroke(
                line,
                with: .color(lit ? Theme.accent.opacity(0.9) : Theme.textMuted.opacity(selected == nil ? 0.28 : 0.1)),
                lineWidth: lit ? 1.6 : 0.8
            )
        }

        for (index, node) in memory.nodes.enumerated() {
            let colour = Self.colour(node.kind)
            let chosen = selected == index
            let dimmed = selected != nil && !chosen && !tied.contains(index)

            // Each dot breathes a little, out of step with the others.
            let breath = 1 + 0.12 * sin(time * 1.4 + Double(index) * 0.7)
            let radius = (node.kind == .fact ? 4.5 : 6.0) * (chosen ? 1.7 : breath)

            let rect = CGRect(
                x: points[index].x - radius,
                y: points[index].y - radius,
                width: radius * 2,
                height: radius * 2
            )

            context.fill(
                Path(ellipseIn: rect.insetBy(dx: -radius * 0.9, dy: -radius * 0.9)),
                with: .color(colour.opacity(dimmed ? 0.03 : 0.16))
            )

            context.fill(Path(ellipseIn: rect), with: .color(colour.opacity(dimmed ? 0.25 : 1)))
        }
    }

    static func colour(_ kind: MemoryKindView) -> Color {
        switch kind {
        case .working: Theme.accent
        case .episode: Theme.model
        case .fact: Theme.good
        }
    }
}

private struct NodeCard: View {
    let node: MemoryNodeView

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 8) {
                Circle()
                    .fill(MemorySheet.colour(node.kind))
                    .frame(width: 8, height: 8)

                Text(node.title)
                    .font(.subheadline.weight(.semibold))
                    .foregroundStyle(Theme.text)
                    .lineLimit(2)
            }

            ScrollView {
                Text(node.text)
                    .font(.footnote)
                    .foregroundStyle(Theme.textSecondary)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .textSelection(.enabled)
            }
            .frame(maxHeight: 140)
        }
        .padding(14)
        .background(.ultraThinMaterial, in: RoundedRectangle(cornerRadius: Theme.radius, style: .continuous))
        .overlay(
            RoundedRectangle(cornerRadius: Theme.radius, style: .continuous)
                .strokeBorder(Theme.border, lineWidth: 1)
        )
    }
}

private struct Legend: View {
    let memory: MemoryView

    var body: some View {
        HStack(spacing: 16) {
            entry("Earlier tasks", .episode)
            entry("Facts and lessons", .fact)

            Spacer(minLength: 0)
        }
        .font(.caption)
        .foregroundStyle(Theme.textSecondary)
    }

    private func entry(_ name: String, _ kind: MemoryKindView) -> some View {
        HStack(spacing: 6) {
            Circle()
                .fill(MemorySheet.colour(kind))
                .frame(width: 7, height: 7)

            Text("\(memory.nodes.filter { $0.kind == kind }.count) \(name.lowercased())")
        }
    }
}

/// Where the dots are. They push each other apart, are pulled together
/// by their ties and towards the middle, and soon come to rest.
private final class Layout {
    private(set) var points: [CGPoint] = []

    private var velocities: [CGVector] = []
    private var size: CGSize = .zero

    /// How much the dots still move. Once it is small, nothing is done.
    private var heat: CGFloat = 1

    func seed(memory: MemoryView, size: CGSize) {
        self.size = size

        heat = 1

        let count = memory.nodes.count
        let centre = CGPoint(x: size.width / 2, y: size.height / 2)
        let spread = min(size.width, size.height) * 0.36

        // A sunflower's arrangement: even, and the same every time.
        points = (0..<count).map { index in
            let distance = spread * sqrt(CGFloat(index) + 0.5) / sqrt(CGFloat(max(count, 1)))
            let angle = CGFloat(index) * 2.399963

            return CGPoint(x: centre.x + cos(angle) * distance, y: centre.y + sin(angle) * distance)
        }

        velocities = Array(repeating: .zero, count: count)
    }

    func step(memory: MemoryView, size: CGSize) {
        if points.count != memory.nodes.count || size != self.size {
            seed(memory: memory, size: size)
        }

        guard heat > 0.02, points.count > 1 else { return }

        let count = points.count
        let centre = CGPoint(x: size.width / 2, y: size.height / 2)
        let ideal = sqrt(size.width * size.height / CGFloat(count)) * 0.62

        var forces = Array(repeating: CGVector.zero, count: count)

        for a in 0..<count {
            for b in (a + 1)..<count {
                let dx = points[a].x - points[b].x
                let dy = points[a].y - points[b].y
                let distance = max(sqrt(dx * dx + dy * dy), 1)
                let push = ideal * ideal / distance / distance * 0.9

                forces[a].dx += dx / distance * push
                forces[a].dy += dy / distance * push
                forces[b].dx -= dx / distance * push
                forces[b].dy -= dy / distance * push
            }

            forces[a].dx += (centre.x - points[a].x) * 0.012
            forces[a].dy += (centre.y - points[a].y) * 0.012
        }

        for link in memory.links {
            let a = Int(link.from), b = Int(link.to)

            guard a < count, b < count else { continue }

            let dx = points[b].x - points[a].x
            let dy = points[b].y - points[a].y
            let distance = max(sqrt(dx * dx + dy * dy), 1)
            let pull = (distance - ideal * 0.8) * 0.02

            forces[a].dx += dx / distance * pull
            forces[a].dy += dy / distance * pull
            forces[b].dx -= dx / distance * pull
            forces[b].dy -= dy / distance * pull
        }

        let margin: CGFloat = 24

        for index in 0..<count {
            velocities[index].dx = (velocities[index].dx + forces[index].dx) * 0.82
            velocities[index].dy = (velocities[index].dy + forces[index].dy) * 0.82

            points[index].x = min(max(points[index].x + velocities[index].dx * heat, margin), size.width - margin)
            points[index].y = min(max(points[index].y + velocities[index].dy * heat, margin), size.height - 150)
        }

        heat *= 0.992
    }

    func nearest(to point: CGPoint, within reach: CGFloat) -> Int? {
        let found = points.enumerated().min { a, b in
            distance(a.element, point) < distance(b.element, point)
        }

        guard let found, distance(found.element, point) <= reach else { return nil }

        return found.offset
    }

    private func distance(_ a: CGPoint, _ b: CGPoint) -> CGFloat {
        hypot(a.x - b.x, a.y - b.y)
    }
}
