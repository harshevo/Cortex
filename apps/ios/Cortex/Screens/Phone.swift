import SwiftUI

/// The agent on this phone: who its model is reached as, the projects it
/// keeps here, and what its networks did while the phone was at rest.
struct PhoneSheet: View {
    @Environment(AppModel.self) private var model
    @Environment(\.dismiss) private var dismiss

    @State private var key = ""
    @State private var address = ""
    @State private var project = ""

    var body: some View {
        let account = model.account
        let chat = model.state.chats.first

        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 20) {
                    Text("The agent, its memory and its networks run here. Only the model is elsewhere.")
                        .font(.callout)
                        .foregroundStyle(Theme.textSecondary)

                    if !model.state.onPhone {
                        Button("Work on this phone") { model.core.usePhone() }
                            .buttonStyle(PrimaryButtonStyle())
                    } else if let problem = chat?.problem {
                        Text(problem)
                            .font(.footnote)
                            .foregroundStyle(Theme.bad)
                    }

                    VStack(alignment: .leading, spacing: 10) {
                        Text("Model").sectionHeading()

                        Picker("Provider", selection: provider(account, chat)) {
                            ForEach(account.providers, id: \.name) { provider in
                                Text(provider.label).tag(provider.name)
                            }
                        }
                        .pickerStyle(.segmented)
                        .labelsHidden()
                        .disabled(chat == nil)

                        Text(account.model)
                            .font(Theme.monoSmall)
                            .foregroundStyle(Theme.textMuted)

                        if account.provider == "codex" {
                            signIn(account)
                        } else {
                            keys(account)
                        }
                    }

                    if model.state.onPhone {
                        VStack(alignment: .leading, spacing: 10) {
                            Text("New project").sectionHeading()

                            field("Name", text: $project)

                            Button("Create and open") {
                                model.core.newProject(name: project)
                                project = ""
                                dismiss()
                            }
                            .buttonStyle(SecondaryButtonStyle())
                            .disabled(project.trimmingCharacters(in: .whitespaces).isEmpty)
                        }
                    }

                    VStack(alignment: .leading, spacing: 6) {
                        Text("At rest").sectionHeading()

                        Text(Rest.last ?? "While the phone is idle, the living network goes over what it met. Nothing yet.")
                            .font(.footnote)
                            .foregroundStyle(Theme.textSecondary)
                    }
                }
                .padding(Theme.gutter)
            }
            .background(Theme.background.ignoresSafeArea())
            .navigationTitle("This phone")
            #if os(iOS)
            .navigationBarTitleDisplayMode(.inline)
            #endif
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                }
            }
            .onAppear { address = account.address }
        }
    }

    /// Choosing a provider chooses the first model it offers.
    private func provider(_ account: AccountView, _ chat: ChatView?) -> Binding<String> {
        Binding(
            get: { account.provider },
            set: { name in
                guard let chat,
                      let offered = model.settings[chat.id]?.models.first(where: { $0.provider == name })
                else { return }

                model.core.setModel(chat: chat.id, provider: name, model: offered.model)
            }
        )
    }

    @ViewBuilder
    private func signIn(_ account: AccountView) -> some View {
        if account.signingIn {
            Text("Waiting for the sign-in page…")
                .font(.footnote)
                .foregroundStyle(Theme.textSecondary)
        } else if account.signedIn {
            Text("Signed in to ChatGPT.")
                .font(.footnote)
                .foregroundStyle(Theme.good)

            Button("Sign out") { model.core.signOut() }
                .buttonStyle(SecondaryButtonStyle())
        } else {
            Button("Sign in to ChatGPT") { model.core.signIn() }
                .buttonStyle(PrimaryButtonStyle())
        }

        if let problem = account.signInProblem {
            Text(problem)
                .font(.footnote)
                .foregroundStyle(Theme.bad)
        }
    }

    @ViewBuilder
    private func keys(_ account: AccountView) -> some View {
        if account.provider == "hosted" {
            field("https://…/v1", text: $address)

            Button("Save the address") { model.core.setAddress(address: address) }
                .buttonStyle(SecondaryButtonStyle())
                .disabled(address.isEmpty)
        }

        Text(account.hasKey ? "A key is kept on this phone." : "No key yet.")
            .font(.footnote)
            .foregroundStyle(account.hasKey ? Theme.good : Theme.textSecondary)

        SecureField("API key", text: $key)
            .font(Theme.mono)
            .textFieldStyle(.plain)
            .padding(12)
            .background(Theme.sunken, in: RoundedRectangle(cornerRadius: Theme.smallRadius, style: .continuous))

        Button("Save the key") {
            model.core.setKey(provider: account.provider, key: key)
            key = ""
        }
        .buttonStyle(SecondaryButtonStyle())
        .disabled(key.isEmpty)
    }

    private func field(_ hint: String, text: Binding<String>) -> some View {
        TextField(hint, text: text)
            .font(Theme.mono)
            .textFieldStyle(.plain)
            .autocorrectionDisabled()
            #if os(iOS)
            .textInputAutocapitalization(.never)
            #endif
            .padding(12)
            .background(Theme.sunken, in: RoundedRectangle(cornerRadius: Theme.smallRadius, style: .continuous))
    }
}
