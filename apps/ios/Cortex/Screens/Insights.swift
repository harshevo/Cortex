import Charts
import SwiftUI

/// What the agent on this phone has cost, by day, session and model, and
/// whether its learning parts help: runs in which each adviser spoke
/// against runs it was held out of.
struct InsightsScreen: View {
    @Environment(\.dismiss) private var dismiss

    enum Tab: Hashable {
        case tokens, effect
    }

    @State private var tab = Tab.tokens
    @State private var month = MonthShown.now
    @State private var usage: UsageMonthView?
    @State private var effect: EffectView?
    @State private var picked: String?

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 20) {
                Picker("Show", selection: $tab) {
                    Text("Tokens").tag(Tab.tokens)
                    Text("Does it help?").tag(Tab.effect)
                }
                .pickerStyle(.segmented)

                switch tab {
                case .tokens:
                    if let usage {
                        TokensView(usage: usage, month: $month, picked: $picked)
                    } else {
                        ProgressView().frame(maxWidth: .infinity)
                    }

                case .effect:
                    if let effect {
                        EffectSection(report: effect)
                    } else {
                        ProgressView().frame(maxWidth: .infinity)
                    }
                }
            }
            .padding(.horizontal, Theme.gutter)
            .padding(.vertical, 16)
            .frame(maxWidth: 720)
            .frame(maxWidth: .infinity)
        }
        .background(Theme.background.ignoresSafeArea())
        .navigationTitle("Insights")
        #if os(iOS)
        .navigationBarTitleDisplayMode(.inline)
        #endif
        .toolbar {
            ToolbarItem(placement: .confirmationAction) {
                Button("Done") { dismiss() }
            }
        }
        .task(id: month) { await read() }
        .refreshable { await read() }
    }

    /// Reads the records off the main thread: a long history is many
    /// lines.
    private func read() async {
        let folder = Rest.folder.path
        let offset = Int32(TimeZone.current.secondsFromGMT() / 60)
        let now = UInt64(Date().timeIntervalSince1970 * 1000)
        let (year, number) = (month.year, month.month)

        let (usage, effect) = await Task.detached {
            (
                usageMonth(folder: folder, year: year, month: number, utcOffsetMinutes: offset, nowMs: now),
                effectReport(folder: folder, utcOffsetMinutes: offset)
            )
        }.value

        self.usage = usage
        self.effect = effect
    }
}

struct MonthShown: Hashable {
    var year: Int32
    var month: UInt32

    static var now: MonthShown {
        let parts = Calendar.current.dateComponents([.year, .month], from: Date())

        return MonthShown(year: Int32(parts.year ?? 2026), month: UInt32(parts.month ?? 1))
    }

    func turned(by months: Int) -> MonthShown {
        let index = Int(year) * 12 + Int(month) - 1 + months

        return MonthShown(year: Int32(index / 12), month: UInt32(index % 12 + 1))
    }

    var name: String {
        "\(DateFormatter().monthSymbols[Int(month) - 1]) \(year)"
    }
}

private struct TokensView: View {
    let usage: UsageMonthView
    @Binding var month: MonthShown
    @Binding var picked: String?

    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            HStack {
                Button { month = month.turned(by: -1) } label: { Image(systemName: "chevron.left") }
                Text(month.name).font(.headline).foregroundStyle(Theme.text)
                Button { month = month.turned(by: 1) } label: { Image(systemName: "chevron.right") }
            }
            .buttonStyle(.plain)
            .foregroundStyle(Theme.accent)

            LazyVGrid(columns: [GridItem(.adaptive(minimum: 150), spacing: 10)], spacing: 10) {
                Tile(name: "Tokens", value: count(usage.tokens), note: "\(count(usage.inputTokens)) in · \(count(usage.outputTokens)) out")
                Tile(name: "Cost", value: usage.cost, note: usage.costNote)
                Tile(name: "Sessions", value: "\(usage.sessions)", note: "\(usage.calls) model calls")
                Tile(name: "Streak", value: "\(usage.streak) \(usage.streak == 1 ? "day" : "days")", note: "in a row with use")
                Tile(name: "Favourite model", value: usage.favouriteModel ?? "—", note: "most tokens went to it")
                Tile(name: "Peak day", value: usage.peakDay ?? "—", note: usage.peakDay == nil ? "" : "\(count(usage.peakDayTokens)) tokens")
                Tile(name: "Peak hour", value: usage.peakHour.map { String(format: "%02d:00", $0) } ?? "—", note: "of the day")
                Tile(name: "From the cache", value: usage.cacheHit.map { String(format: "%.0f%%", $0 * 100) } ?? "—", note: "of input, billed cheaper")
            }

            Text("By day").sectionHeading()
            CalendarGrid(usage: usage, picked: $picked)

            Text("By hour of the day").sectionHeading()

            Chart(Array(usage.hours.enumerated()), id: \.offset) { hour, tokens in
                BarMark(x: .value("Hour", hour), y: .value("Tokens", tokens))
                    .foregroundStyle(Theme.accent)
            }
            .chartXAxis {
                AxisMarks(values: [0, 6, 12, 18, 23])
            }
            .frame(height: 120)

            if !usage.models.isEmpty {
                Text("By model").sectionHeading()
            }

            ForEach(Array(usage.models.enumerated()), id: \.offset) { _, model in
                VStack(alignment: .leading, spacing: 4) {
                    HStack {
                        Text(model.model).font(.callout).foregroundStyle(Theme.text)
                        Spacer()
                        Text("\(count(model.tokens)) · \(model.cost)").font(Theme.monoSmall).foregroundStyle(Theme.textSecondary)
                    }

                    ProgressView(value: model.share).tint(Theme.accent)
                }
            }

            if !sessions.isEmpty {
                Text(picked.map { "Sessions on \($0)" } ?? "Sessions").sectionHeading()
            }

            ForEach(Array(sessions.enumerated()), id: \.offset) { _, session in
                HStack {
                    Text(session.title).font(.callout).foregroundStyle(Theme.text).lineLimit(1)
                    Spacer()
                    Text("\(count(session.tokens)) · \(session.cost)").font(Theme.monoSmall).foregroundStyle(Theme.textSecondary)
                }
            }

            Text("Every model call made on this phone is recorded in the app's own folder. A model without a known price is counted but not costed; ChatGPT sign-in use is covered by the subscription.")
                .font(.caption)
                .foregroundStyle(Theme.textMuted)
        }
    }

    private var sessions: [UsageSessionView] {
        guard let picked else { return usage.recent }

        return usage.recent.filter { day($0.firstMs.asSigned) <= picked && picked <= day($0.lastMs.asSigned) }
    }
}

/// The month as a grid of days, each as dark as the tokens used on it.
private struct CalendarGrid: View {
    let usage: UsageMonthView
    @Binding var picked: String?

    var body: some View {
        let most = max(usage.days.map { $0.inputTokens + $0.cachedTokens + $0.outputTokens }.max() ?? 1, 1)
        let columns = Array(repeating: GridItem(.flexible(), spacing: 5), count: 7)

        LazyVGrid(columns: columns, spacing: 5) {
            // Every cell of the one grid needs an identity of its own:
            // the same number for a heading and a day hides the day.
            ForEach(["M 0", "T 1", "W 2", "T 3", "F 4", "S 5", "S 6"], id: \.self) { heading in
                Text(heading.prefix(1))
                    .font(.caption2)
                    .foregroundStyle(Theme.textMuted)
            }

            ForEach((0..<Int(usage.firstWeekday)).map { "blank \($0)" }, id: \.self) { _ in
                Color.clear.frame(height: 34)
            }

            ForEach(Array(usage.days.enumerated()), id: \.element.date) { index, day in
                let tokens = day.inputTokens + day.cachedTokens + day.outputTokens
                let strength = (Double(tokens) / Double(most)).squareRoot()

                Button {
                    if tokens > 0 { picked = picked == day.date ? nil : day.date }
                } label: {
                    VStack(spacing: 1) {
                        Text("\(index + 1)").font(.caption2)

                        if tokens > 0 {
                            Text(count(tokens)).font(.system(size: 8))
                        }
                    }
                    .foregroundStyle(strength > 0.55 ? Theme.background : Theme.textSecondary)
                    .frame(maxWidth: .infinity, minHeight: 34)
                    .background(
                        tokens == 0 ? Theme.sunken : Theme.accent.opacity(0.18 + 0.82 * strength),
                        in: RoundedRectangle(cornerRadius: 6, style: .continuous)
                    )
                    .overlay(
                        RoundedRectangle(cornerRadius: 6, style: .continuous)
                            .strokeBorder(picked == day.date ? Theme.text : .clear, lineWidth: 2)
                    )
                }
                .buttonStyle(.plain)
                .accessibilityLabel("\(day.date): \(tokens) tokens, \(day.cost)")
            }
        }
    }
}

private struct EffectSection: View {
    let report: EffectView

    var body: some View {
        VStack(alignment: .leading, spacing: 18) {
            Text("In about a quarter of runs each adviser is held out at random: it still watches and learns, but the model is not shown what it says. Runs it spoke in are compared with runs it was held out of. \(report.runs) runs recorded, \(report.runsInExperiment) in the experiment; a verdict needs \(report.minPerArm) on each side.")
                .font(.footnote)
                .foregroundStyle(Theme.textMuted)

            if report.comparisons > 0 {
                Text("\(report.comparisons) comparisons have enough runs. About one in twenty shows a difference by chance alone.")
                    .font(.footnote)
                    .foregroundStyle(Theme.textMuted)
            }

            AdviserCard(adviser: report.overall, title: "All advisers together")

            ForEach(Array(report.advisers.enumerated()), id: \.offset) { _, adviser in
                AdviserCard(adviser: adviser, title: adviser.name)
            }

            if report.advisers.isEmpty {
                Text("No adviser has spoken in a recorded run yet. Turn on Intuition or the Dendritic network under Capabilities and use the agent: the comparison fills as runs finish.")
                    .font(.footnote)
                    .foregroundStyle(Theme.textSecondary)
                    .card()
            }
        }
    }
}

private struct AdviserCard: View {
    let adviser: AdviserView
    let title: String

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            Text(title).font(.headline).foregroundStyle(Theme.text)

            Text("Spoke in \(adviser.runsSpoke) runs, held out of \(adviser.runsHeldOut), nothing to say in \(adviser.runsNothingToSay).")
                .font(.caption)
                .foregroundStyle(Theme.textMuted)

            ForEach(Array(adviser.metrics.enumerated()), id: \.offset) { _, metric in
                MetricLine(metric: metric)
            }

            let weeks = adviser.weeks.filter { $0.withRate != nil || $0.withoutRate != nil }

            if weeks.count > 1 {
                Chart {
                    ForEach(Array(weeks.enumerated()), id: \.offset) { _, week in
                        if let rate = week.withRate {
                            LineMark(x: .value("Week", week.week), y: .value("Passed", rate), series: .value("Side", "with it"))
                                .foregroundStyle(Theme.accent)
                        }

                        if let rate = week.withoutRate {
                            LineMark(x: .value("Week", week.week), y: .value("Passed", rate), series: .value("Side", "without"))
                                .foregroundStyle(Theme.textMuted)
                        }
                    }
                }
                .chartYScale(domain: 0...1)
                .frame(height: 110)

                Text("Checks passed by week: accent with it, grey without.")
                    .font(.caption2)
                    .foregroundStyle(Theme.textMuted)
            }

            if adviser.comparableObservational > 0 {
                Text("Not causal: of \(adviser.comparableObservational) checkable pieces of advice, the model did it next \(adviser.followedObservational) times.")
                    .font(.caption)
                    .foregroundStyle(Theme.textMuted)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .card()
    }
}

private struct MetricLine: View {
    let metric: MetricView

    var body: some View {
        VStack(alignment: .leading, spacing: 3) {
            HStack {
                Text(metric.name).font(.callout).foregroundStyle(Theme.text)
                Spacer()
                Text(verdict).font(.caption.weight(.semibold)).foregroundStyle(colour)
            }

            Text("\(value(metric.withMean)) with (\(metric.withRuns)) · \(value(metric.withoutMean)) without (\(metric.withoutRuns))")
                .font(.caption)
                .foregroundStyle(Theme.textSecondary)

            Canvas { context, size in
                let middle = size.width / 2

                context.fill(Path(roundedRect: CGRect(origin: .zero, size: size), cornerRadius: 3), with: .color(Theme.sunken))

                var zero = Path()
                zero.move(to: CGPoint(x: middle, y: 0))
                zero.addLine(to: CGPoint(x: middle, y: size.height))
                context.stroke(zero, with: .color(Theme.textMuted), lineWidth: 1)

                guard let difference = metric.difference, let low = metric.low, let high = metric.high else { return }

                // Better is to the right whichever way the metric runs.
                let flip = metric.higherIsBetter ? 1.0 : -1.0
                let reach = metric.rate ? 1.0 : max(abs(low), abs(high), 1e-9)
                let at = { (value: Double) in middle + CGFloat(min(max(value * flip / reach, -1), 1)) * (middle - 4) }

                let left = min(at(low), at(high)), right = max(at(low), at(high))

                context.fill(Path(roundedRect: CGRect(x: left, y: 3, width: max(2, right - left), height: size.height - 6), cornerRadius: 2), with: .color(colour.opacity(0.45)))
                context.fill(Path(ellipseIn: CGRect(x: at(difference) - 4, y: size.height / 2 - 4, width: 8, height: 8)), with: .color(colour))
            }
            .frame(height: 14)
        }
    }

    private func value(_ mean: Double?) -> String {
        guard let mean else { return "—" }

        return metric.rate ? String(format: "%.0f%%", mean * 100) : count(UInt64(max(0, mean.rounded())))
    }

    private var verdict: String {
        switch metric.verdict {
        case .helps: "helps"
        case .hurts: "hurts"
        case .noDifferenceSeen: "no difference seen"
        case .tooFewRuns: "too few runs"
        }
    }

    private var colour: Color {
        switch metric.verdict {
        case .helps: Theme.good
        case .hurts: Theme.bad
        case .noDifferenceSeen: Theme.textSecondary
        case .tooFewRuns: Theme.textMuted
        }
    }
}

private struct Tile: View {
    let name: String
    let value: String
    let note: String

    var body: some View {
        VStack(alignment: .leading, spacing: 3) {
            Text(name.uppercased()).font(.caption2.weight(.semibold)).foregroundStyle(Theme.textMuted)
            Text(value).font(.headline).foregroundStyle(Theme.text).lineLimit(1).minimumScaleFactor(0.7)
            Text(note).font(.caption2).foregroundStyle(Theme.textSecondary).lineLimit(2)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .card(padding: 12)
    }
}

/// A count of tokens, short: 950, 12.4k, 3.1M.
func count(_ tokens: UInt64) -> String {
    switch tokens {
    case ..<1_000: "\(tokens)"
    case ..<1_000_000: String(format: "%.1fk", Double(tokens) / 1_000)
    case ..<1_000_000_000: String(format: "%.1fM", Double(tokens) / 1_000_000)
    default: String(format: "%.2fB", Double(tokens) / 1_000_000_000)
    }
}

extension UInt64 {
    var asSigned: Int64 { Int64(clamping: self) }
}
