import SwiftUI

/// The model, how hard it thinks, whether it asks first, and every
/// optional capability of the engine, each as a switch.
struct CapabilitiesSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss

    let chat: String

    @State private var search = ""

    private var settings: SettingsView {
        model.settings[chat] ?? model.core.settings(chat: chat)
    }

    private var running: Bool {
        model.live[chat]?.running ?? false
    }

    /// The groups in the order the computer lists them.
    private var groups: [(String, [CapabilityView])] {
        var groups: [(String, [CapabilityView])] = []

        for capability in settings.capabilities where matches(capability) {
            if let index = groups.firstIndex(where: { $0.0 == capability.group }) {
                groups[index].1.append(capability)
            } else {
                groups.append((capability.group, [capability]))
            }
        }

        return groups
    }

    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 24) {
                    if running {
                        Label("Changes take effect when the run in progress has finished.", systemImage: "clock")
                            .font(.footnote)
                            .foregroundStyle(Theme.warning)
                    }

                    if search.isEmpty {
                        ModelSection(chat: chat, settings: settings)
                    }

                    ForEach(groups, id: \.0) { group, capabilities in
                        VStack(alignment: .leading, spacing: Theme.tight) {
                            HStack {
                                Text(group).sectionHeading()

                                Spacer()

                                Text("\(capabilities.filter(\.on).count) of \(capabilities.count) on")
                                    .font(.caption.monospacedDigit())
                                    .foregroundStyle(Theme.textMuted)
                            }

                            VStack(spacing: 0) {
                                ForEach(Array(capabilities.enumerated()), id: \.element.name) { index, capability in
                                    CapabilityRow(chat: chat, capability: capability)

                                    if index < capabilities.count - 1 {
                                        Divider().overlay(Theme.border.opacity(0.6))
                                    }
                                }
                            }
                            .card(padding: 0)
                        }
                    }
                }
                .padding(Theme.gutter)
                .padding(.bottom, 24)
            }
            .background(Theme.background.ignoresSafeArea())
            .searchable(text: $search, prompt: "Search capabilities")
            .navigationTitle("Capabilities")
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

    private func matches(_ capability: CapabilityView) -> Bool {
        guard !search.isEmpty else { return true }

        return [capability.label, capability.name, capability.help]
            .contains { $0.localizedCaseInsensitiveContains(search) }
    }
}

private struct ModelSection: View {
    @Environment(AppModel.self) private var model

    let chat: String
    let settings: SettingsView

    var body: some View {
        VStack(alignment: .leading, spacing: Theme.tight) {
            Text("Model").sectionHeading()

            VStack(spacing: 0) {
                Menu {
                    ForEach(providers, id: \.self) { provider in
                        Section(provider) {
                            ForEach(settings.models.filter { $0.providerLabel == provider }, id: \.self) { choice in
                                Button {
                                    Haptics.tap()
                                    model.core.setModel(chat: chat, provider: choice.provider, model: choice.model)
                                } label: {
                                    if choice.model == settings.model && choice.provider == settings.provider {
                                        Label(choice.model, systemImage: "checkmark")
                                    } else {
                                        Text(choice.model)
                                    }
                                }
                            }
                        }
                    }
                } label: {
                    HStack {
                        Image(systemName: "cpu")
                            .foregroundStyle(Theme.model)
                            .frame(width: 26)

                        Text(settings.model.isEmpty ? "No model chosen" : settings.model)
                            .font(.body.weight(.medium))
                            .foregroundStyle(Theme.text)
                            .lineLimit(1)

                        Spacer()

                        Image(systemName: "chevron.up.chevron.down")
                            .font(.caption.weight(.semibold))
                            .foregroundStyle(Theme.textMuted)
                    }
                    .padding(.horizontal, 14)
                    .frame(minHeight: 52)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)

                if settings.takesEffort {
                    Divider().overlay(Theme.border.opacity(0.6))

                    VStack(alignment: .leading, spacing: 8) {
                        Text("Effort")
                            .font(.subheadline)
                            .foregroundStyle(Theme.textSecondary)

                        Picker("Effort", selection: effort) {
                            ForEach(settings.efforts, id: \.self) { effort in
                                Text(Self.name(effort)).tag(effort)
                            }
                        }
                        .pickerStyle(.segmented)
                        .labelsHidden()
                    }
                    .padding(14)
                }

                Divider().overlay(Theme.border.opacity(0.6))

                HStack(spacing: 12) {
                    VStack(alignment: .leading, spacing: 2) {
                        Text("Act without asking")
                            .font(.body.weight(.medium))
                            .foregroundStyle(Theme.text)

                        Text("Run commands and edit files without waiting for an answer.")
                            .font(.footnote)
                            .foregroundStyle(Theme.textMuted)
                            .fixedSize(horizontal: false, vertical: true)
                    }

                    Spacer(minLength: 0)

                    Toggle("Act without asking", isOn: approveEverything)
                        .labelsHidden()
                        .toggleStyle(.switch)
                        .tint(Theme.accent)
                }
                .padding(14)
            }
            .card(padding: 0)
        }
    }

    private var providers: [String] {
        var seen: [String] = []

        for choice in settings.models where !seen.contains(choice.providerLabel) {
            seen.append(choice.providerLabel)
        }

        return seen
    }

    private var effort: Binding<String> {
        Binding(
            get: { settings.effort },
            set: { effort in
                Haptics.tap()
                model.core.setEffort(chat: chat, effort: effort)
            }
        )
    }

    private var approveEverything: Binding<Bool> {
        Binding(
            get: { settings.approveEverything },
            set: { on in
                Haptics.tap()
                model.core.setApproveEverything(chat: chat, on: on)
            }
        )
    }

    private static func name(_ effort: String) -> String {
        switch effort {
        case "xhigh": "Extra"
        default: effort.capitalized
        }
    }
}

private struct CapabilityRow: View {
    @Environment(AppModel.self) private var model

    let chat: String
    let capability: CapabilityView

    @State private var expanded = false

    /// The memory screen shows the agent's memory, so it is always kept.
    private var fixed: Bool { capability.name == "memory" }

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 12) {
                VStack(alignment: .leading, spacing: 3) {
                    Text(capability.label)
                        .font(.body.weight(.medium))
                        .foregroundStyle(Theme.text)

                    Text(capability.name)
                        .font(Theme.monoSmall)
                        .foregroundStyle(Theme.textMuted)
                }

                Spacer(minLength: 0)

                Toggle(capability.label, isOn: on)
                    .labelsHidden()
                    .toggleStyle(.switch)
                    .tint(Theme.accent)
                    .disabled(fixed)
            }

            Text(help)
                .font(.footnote)
                .foregroundStyle(Theme.textSecondary)
                .lineLimit(expanded ? nil : 2)
                .fixedSize(horizontal: false, vertical: true)
                .onTapGesture {
                    withAnimation(Theme.spring) { expanded.toggle() }
                }
        }
        .padding(14)
        .frame(maxWidth: .infinity, alignment: .leading)
        .contentShape(Rectangle())
    }

    /// The help, with the names it sets in backticks shown as code.
    private var help: AttributedString {
        let options = AttributedString.MarkdownParsingOptions(
            interpretedSyntax: .inlineOnlyPreservingWhitespace
        )

        return (try? AttributedString(markdown: capability.help, options: options))
            ?? AttributedString(capability.help)
    }

    private var on: Binding<Bool> {
        Binding(
            get: { capability.on },
            set: { on in
                Haptics.tap()
                model.core.setCapability(chat: chat, name: capability.name, on: on)
            }
        )
    }
}
