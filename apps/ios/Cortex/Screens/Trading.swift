import SwiftUI
import UniformTypeIdentifiers

/// Price bars read with the liquid and the dendritic network: the chart,
/// the verdict, and how the forecasts did on bars they were not trained
/// on. Nothing here is advice.
struct TradingScreen: View {
    @State private var desk = TradingDesk()
    @State private var importing = false
    @State private var horizon: UInt32 = 1

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 20) {
                header

                if desk.series.bars == 0 {
                    empty
                } else {
                    series
                    CandleChart(desk: desk)
                        .frame(height: 260)
                        .card(padding: 10)

                    if let report = desk.report {
                        Verdict(report: report)
                        Forecast(report: report)
                        Scores(report: report)
                        NetworkFacts(report: report)
                        Notes(report: report)
                    }
                }
            }
            .padding(.horizontal, Theme.gutter)
            .padding(.vertical, 16)
            .frame(maxWidth: 720)
            .frame(maxWidth: .infinity)
        }
        .background(Theme.background.ignoresSafeArea())
        .navigationTitle("Trading")
        .fileImporter(
            isPresented: $importing,
            allowedContentTypes: [.commaSeparatedText, .tabSeparatedText, .plainText]
        ) { result in
            if case .success(let url) = result {
                desk.open(url)
            }
        }
    }

    private var header: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("Trading")
                .font(Theme.title(28))
                .foregroundStyle(Theme.text)

            Text("A liquid network and the dendritic network read the bars together. They are trained on the first three quarters and scored on the last, which they never saw.")
                .font(.footnote)
                .foregroundStyle(Theme.textMuted)
        }
    }

    private var empty: some View {
        VStack(alignment: .leading, spacing: 14) {
            Text("Open a CSV of price bars: a time and a close column, and open, high, low and volume if it has them.")
                .font(.callout)
                .foregroundStyle(Theme.textSecondary)

            Button("Choose a file") { importing = true }
                .buttonStyle(PrimaryButtonStyle())

            if let problem = desk.problem {
                Text(problem)
                    .font(.footnote)
                    .foregroundStyle(Theme.bad)
            }
        }
        .card()
    }

    private var series: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("\(desk.series.name) · \(desk.series.bars) bars · \(day(desk.series.firstMs)) to \(day(desk.series.lastMs))")
                .font(.footnote)
                .foregroundStyle(Theme.textSecondary)

            ForEach(desk.series.notes, id: \.self) { note in
                Text(note)
                    .font(.caption)
                    .foregroundStyle(Theme.textMuted)
            }

            if let problem = desk.problem {
                Text(problem)
                    .font(.footnote)
                    .foregroundStyle(Theme.bad)
            }

            HStack(spacing: 8) {
                Text("Bars ahead")
                    .font(.footnote)
                    .foregroundStyle(Theme.textSecondary)

                Picker("Bars ahead", selection: $horizon) {
                    ForEach([UInt32(1), 3, 5, 10], id: \.self) { value in
                        Text("\(value)").tag(value)
                    }
                }
                .pickerStyle(.segmented)
                .frame(maxWidth: 200)
            }

            HStack(spacing: 10) {
                Button(desk.report == nil ? "Analyse" : "Analyse again") {
                    Haptics.tap()
                    desk.analyse(horizon: horizon)
                }
                .buttonStyle(PrimaryButtonStyle())
                .disabled(desk.working)

                Button("Another file") { importing = true }
                    .buttonStyle(SecondaryButtonStyle())
                    .disabled(desk.working)
            }

            switch desk.series.status {
            case .working(let stage, let fraction):
                ProgressView(value: Double(fraction)) {
                    Text(stage).font(.footnote).foregroundStyle(Theme.textSecondary)
                }
                .tint(Theme.accent)

            case .failed(let reason):
                Text(reason).font(.footnote).foregroundStyle(Theme.bad)

            default:
                EmptyView()
            }
        }
    }
}

/// Keeps the core's trading desk and fetches what it holds when it says
/// something changed.
@MainActor
@Observable
final class TradingDesk {
    private(set) var series: TradingSeries
    private(set) var report: TradingReport?
    private(set) var problem: String?

    let core: Trading
    private let ears: TradingEars

    init() {
        let ears = TradingEars()

        self.ears = ears
        self.core = Trading(listener: ears)
        self.series = core.series()

        ears.desk = self
    }

    var working: Bool {
        if case .working = series.status { return true }

        return false
    }

    func open(_ url: URL) {
        let allowed = url.startAccessingSecurityScopedResource()

        defer {
            if allowed { url.stopAccessingSecurityScopedResource() }
        }

        guard let text = try? String(contentsOf: url, encoding: .utf8) else {
            problem = "\(url.lastPathComponent) cannot be read as text."

            return
        }

        problem = core.loadCsv(name: url.lastPathComponent, text: text)
        refresh()
    }

    func analyse(horizon: UInt32) {
        core.analyse(horizon: horizon)
        refresh()
    }

    func candles(first: UInt32, count: UInt32, most: UInt32) -> [CandleView] {
        core.candles(first: first, count: count, most: most)
    }

    fileprivate func refresh() {
        series = core.series()
        report = core.report()
    }
}

private final class TradingEars: TradingListener, @unchecked Sendable {
    weak var desk: TradingDesk?

    func changed() {
        Task { @MainActor [weak self] in
            self?.desk?.refresh()
        }
    }
}

/// Candles, as many to one as the width needs, with a band over the bars
/// that were held out and a strip below for how the calls went there.
private struct CandleChart: View {
    let desk: TradingDesk

    var body: some View {
        GeometryReader { proxy in
            let size = proxy.size
            let most = UInt32(max(1, Int(size.width / 5)))
            let bars = desk.series.bars
            let candles = desk.candles(first: 0, count: bars, most: most)
            let trainBars = desk.report?.trainBars

            Canvas { context, size in
                draw(candles, trainBars: trainBars, bars: bars, in: &context, size: size)
            }
            // Read again when the analysis or the bars change.
            .id(desk.series.revision)
        }
    }

    private func draw(
        _ candles: [CandleView],
        trainBars: UInt32?,
        bars: UInt32,
        in context: inout GraphicsContext,
        size: CGSize
    ) {
        guard !candles.isEmpty, bars > 0 else { return }

        let strip: CGFloat = 8
        let plotHeight = size.height - strip - 6

        let high = candles.map(\.high).max() ?? 1
        let low = candles.map(\.low).min() ?? 0
        let span = max(high - low, abs(high) * 1e-6, 1e-12)

        let x = { (bar: Double) in CGFloat(bar / Double(bars)) * size.width }
        let y = { (price: Double) in plotHeight - CGFloat((price - low) / span) * (plotHeight - 8) - 4 }

        if let trainBars {
            let left = x(Double(trainBars))

            context.fill(
                Path(CGRect(x: left, y: 0, width: size.width - left, height: plotHeight)),
                with: .color(Theme.raised.opacity(0.6))
            )
        }

        let width = max(1, min(10, size.width / CGFloat(candles.count) * 0.7))

        for candle in candles {
            let middle = x(Double(candle.first) + Double(candle.count) / 2)
            let colour = candle.close >= candle.open ? Theme.good : Theme.bad

            var wick = Path()
            wick.move(to: CGPoint(x: middle, y: y(candle.high)))
            wick.addLine(to: CGPoint(x: middle, y: y(candle.low)))
            context.stroke(wick, with: .color(colour), lineWidth: 1)

            let top = y(max(candle.open, candle.close))
            let bottom = y(min(candle.open, candle.close))

            context.fill(
                Path(CGRect(x: middle - width / 2, y: top, width: width, height: max(1, bottom - top))),
                with: .color(colour)
            )

            if let right = candle.right {
                let cell = CGRect(
                    x: x(Double(candle.first)),
                    y: plotHeight + 6,
                    width: max(1, x(Double(candle.count))),
                    height: strip
                )

                context.fill(Path(cell), with: .color(Theme.bad.mix(Theme.good, share: Double(right))))
            }
        }
    }
}

private struct Verdict: View {
    let report: TradingReport

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text(title)
                .font(.headline)
                .foregroundStyle(colour)

            Text(report.verdict)
                .font(.callout)
                .foregroundStyle(Theme.textSecondary)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .card()
    }

    private var title: String {
        switch report.edge {
        case .none: "No edge found"
        case .weak: "A weak edge, perhaps"
        case .present: "An edge on the held-out bars"
        }
    }

    private var colour: Color {
        switch report.edge {
        case .none: Theme.textSecondary
        case .weak: Theme.warning
        case .present: Theme.good
        }
    }
}

private struct Forecast: View {
    let report: TradingReport

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("Forecast for the next \(report.horizon == 1 ? "bar" : "\(report.horizon) bars")")
                .sectionHeading()

            HStack(alignment: .firstTextBaseline, spacing: 10) {
                Text(direction)
                    .font(.title2.weight(.semibold))
                    .foregroundStyle(colour)

                Text(String(format: "%+.2f%% expected · %.0f%% sure of the direction", report.expectedReturn * 100, report.confidence * 100))
                    .font(.footnote)
                    .foregroundStyle(Theme.textSecondary)
            }

            if case .none = report.edge {
                Text("With no edge found this is no better than a coin; it shows what the networks say, not something to act on.")
                    .font(.caption)
                    .foregroundStyle(Theme.textMuted)
            }

            Fact(name: "Now", value: report.now.joined(separator: ", "))
            Fact(name: "Regime", value: report.regime)

            ForEach(Array(report.expects.enumerated()), id: \.offset) { _, expected in
                Fact(name: "Expects", value: String(format: "%@ · %.0f%%", expected.text, expected.confidence * 100))
            }

            ForEach(Array(report.imagines.enumerated()), id: \.offset) { step, imagined in
                Fact(name: "Imagines +\(step + 1)", value: String(format: "%@ · %.0f%%", imagined.text, imagined.confidence * 100))
            }
        }
    }

    private var direction: String {
        switch report.direction {
        case .up: "Up"
        case .down: "Down"
        case .flat: "Flat"
        }
    }

    private var colour: Color {
        switch report.direction {
        case .up: Theme.good
        case .down: Theme.bad
        case .flat: Theme.textSecondary
        }
    }
}

/// Each predictor's share of directions called right, against one half.
private struct Scores: View {
    let report: TradingReport

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text("Direction called right, on held-out bars").sectionHeading()

            ForEach(Array(report.scores.enumerated()), id: \.offset) { _, score in
                VStack(alignment: .leading, spacing: 4) {
                    HStack {
                        Text(score.name)
                            .font(.callout)
                            .foregroundStyle(Theme.text)

                        if score.toBeat {
                            Text("to beat").font(.caption).foregroundStyle(Theme.warning)
                        }

                        Spacer()

                        Text(score.accuracy.map { String(format: "%.1f%% of %d", $0 * 100, score.scored) } ?? "no calls")
                            .font(Theme.monoSmall)
                            .foregroundStyle(Theme.textSecondary)
                    }

                    ScoreBar(score: score)
                        .frame(height: 8)
                }
            }

            Text("The line is 50%. The lighter band is where the true share likely lies; where it crosses the line, the predictor is not told apart from a coin.")
                .font(.caption)
                .foregroundStyle(Theme.textMuted)
        }
    }
}

private struct ScoreBar: View {
    let score: ScoreView

    var body: some View {
        Canvas { context, size in
            let at = { (share: Double) in CGFloat(min(max((share - 0.3) / 0.4, 0), 1)) * size.width }
            let colour = score.learned ? Theme.accent : Theme.textMuted

            context.fill(Path(roundedRect: CGRect(origin: .zero, size: size), cornerRadius: 3), with: .color(Theme.sunken))

            if let low = score.accuracyLow, let high = score.accuracyHigh {
                context.fill(
                    Path(roundedRect: CGRect(x: at(low), y: 0, width: max(2, at(high) - at(low)), height: size.height), cornerRadius: 3),
                    with: .color(colour.opacity(0.35))
                )
            }

            if let accuracy = score.accuracy {
                context.fill(
                    Path(ellipseIn: CGRect(x: at(accuracy) - 4, y: size.height / 2 - 4, width: 8, height: 8)),
                    with: .color(colour)
                )
            }

            var half = Path()
            half.move(to: CGPoint(x: at(0.5), y: -2))
            half.addLine(to: CGPoint(x: at(0.5), y: size.height + 2))
            context.stroke(half, with: .color(Theme.textMuted), lineWidth: 1)
        }
    }
}

private struct NetworkFacts: View {
    let report: TradingReport

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("The dendritic network").sectionHeading()

            Fact(name: "Grew to", value: "\(report.neurons) neurons, \(report.branches) branches, \(report.synapses) synapses")
            Fact(name: "Expected", value: String(format: "%.0f%% of each bar (guessing the commonest: %.0f%%)", report.expectedRight * 100, report.baselineRight * 100))
        }
    }
}

private struct Notes: View {
    let report: TradingReport

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("To know").sectionHeading()

            ForEach(report.notes, id: \.self) { note in
                Text(note).font(.caption).foregroundStyle(Theme.textMuted)
            }

            Text(String(format: "Analysed in %.0f ms on this device's processor.", report.totalMs))
                .font(.caption)
                .foregroundStyle(Theme.textMuted)
        }
    }
}

/// A name and its value, side by side.
struct Fact: View {
    let name: String
    let value: String

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 10) {
            Text(name)
                .font(.footnote)
                .foregroundStyle(Theme.textSecondary)
                .frame(width: 92, alignment: .leading)

            Text(value)
                .font(.footnote)
                .foregroundStyle(Theme.text)
                .fixedSize(horizontal: false, vertical: true)
        }
    }
}

/// The day a time falls on, in UTC.
func day(_ ms: Int64) -> String {
    let formatter = DateFormatter()

    formatter.dateFormat = "yyyy-MM-dd"
    formatter.timeZone = TimeZone(identifier: "UTC")

    return formatter.string(from: Date(timeIntervalSince1970: TimeInterval(ms) / 1000))
}

extension Color {
    /// Part of the way from this colour to another.
    func mix(_ other: Color, share: Double) -> Color {
        #if os(iOS)
        let from = UIColor(self), to = UIColor(other)
        #else
        let from = NSColor(self).usingColorSpace(.sRGB) ?? .black, to = NSColor(other).usingColorSpace(.sRGB) ?? .black
        #endif

        var (r1, g1, b1, a1): (CGFloat, CGFloat, CGFloat, CGFloat) = (0, 0, 0, 0)
        var (r2, g2, b2, a2): (CGFloat, CGFloat, CGFloat, CGFloat) = (0, 0, 0, 0)

        from.getRed(&r1, green: &g1, blue: &b1, alpha: &a1)
        to.getRed(&r2, green: &g2, blue: &b2, alpha: &a2)

        let part = CGFloat(min(max(share, 0), 1))

        return Color(
            .sRGB,
            red: Double(r1 + (r2 - r1) * part),
            green: Double(g1 + (g2 - g1) * part),
            blue: Double(b1 + (b2 - b1) * part),
            opacity: 1
        )
    }
}
