//! Where the model runs: the provider, its address, its key or sign-in.

use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

use eframe::egui::{self, Ui};

use super::rows::{self, FIELD_WIDTH, Filter};
use super::{Action, Form, Saved};
use crate::settings::{Provider, secrets};
use crate::theme::space;

/// How long what was read about the ChatGPT sign-in is believed.
const SIGN_IN_FRESH: Duration = Duration::from_secs(2);

pub fn show(ui: &mut Ui, form: &mut Form, filter: &Filter) -> Option<Action> {
    let before = form.draft.provider;

    rows::block(
        ui,
        filter,
        "Model provider",
        "Each provider keeps its own models, address and key.",
        |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(space::XS, space::XS);

                for provider in Provider::ALL {
                    let selected = form.draft.provider == provider;

                    if rows::choice(ui, provider.label(), selected).clicked() && !selected {
                        // What was typed for the one being left is kept.
                        form.read_models();

                        form.draft.provider = provider;
                    }
                }
            });
        },
    );

    let provider = form.draft.provider;

    if provider != before {
        form.models = form.draft.current().models.join("\n");
        form.key.clear();
        form.notice = None;
    }

    if provider.has_address() {
        let hint = match provider {
            Provider::Local => "http://127.0.0.1:11434/v1",
            _ => "https://api.example.com/v1",
        };

        rows::row(
            ui,
            filter,
            "Address of the API",
            "Where requests are sent. An OpenAI-compatible address ends in /v1.",
            |ui| rows::text_field(ui, &mut form.draft.current_mut().base_url, hint),
        );
    }

    if provider.needs_key() {
        key(ui, form, provider, filter);
    }

    if provider == Provider::Codex {
        sign_in(ui, form, filter);
    }

    None
}

/// Whether a key is saved for `provider`, once the keychain has said.
/// It is asked on another thread: it can take a while, and can stop to
/// ask the person, and the window must not stop with it.
fn has_key(ui: &Ui, form: &mut Form, provider: Provider) -> Option<bool> {
    let slot = &mut form.saved[Form::slot(provider)];

    match slot {
        Saved::Known(saved) => Some(*saved),

        Saved::Asking(answer) => match answer.try_recv() {
            Ok(saved) => {
                *slot = Saved::Known(saved);

                Some(saved)
            }

            // The thread ended without an answer: take it as no key.
            Err(TryRecvError::Disconnected) => {
                *slot = Saved::Known(false);

                Some(false)
            }

            Err(TryRecvError::Empty) => None,
        },

        Saved::Unknown => {
            let (outbox, inbox) = mpsc::channel();
            let context = ui.ctx().clone();

            thread::spawn(move || {
                let _ = outbox.send(secrets::load(provider).is_some());

                context.request_repaint();
            });

            *slot = Saved::Asking(inbox);

            None
        }
    }
}

fn key(ui: &mut Ui, form: &mut Form, provider: Provider, filter: &Filter) {
    let has_key = has_key(ui, form, provider);
    let fallback = provider.key_variable().unwrap_or_default();

    let status = match (has_key, secrets::from_environment(provider).is_some()) {
        (None, _) => "Looking in the keychain…".to_string(),
        (Some(true), _) => "A key is saved in the system keychain, not in a file.".to_string(),
        (Some(false), true) => format!("No key saved. Using the one in {fallback}."),
        (Some(false), false) => format!("No key saved, and {fallback} is not set."),
    };

    rows::block(ui, filter, "API key", &status, |ui| {
        let hint = if has_key == Some(true) {
            "Type a new key to replace the saved one"
        } else {
            "Paste your key"
        };

        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut form.key)
                    .password(true)
                    .hint_text(hint)
                    .desired_width((ui.available_width() - 150.0).clamp(120.0, FIELD_WIDTH))
                    .margin(egui::Margin::symmetric(8, 5)),
            );

            let typed = !form.key.trim().is_empty();

            if ui
                .add_enabled(typed, egui::Button::new("Save key"))
                .clicked()
            {
                form.notice = Some(match secrets::save(provider, &form.key) {
                    Ok(()) => {
                        form.saved[Form::slot(provider)] = Saved::Known(true);
                        form.key.clear();

                        ("Key saved. Apply to start using it.".to_string(), true)
                    }

                    Err(error) => (error, false),
                });
            }

            if has_key == Some(true) && ui.button("Remove").clicked() {
                form.notice = Some(match secrets::remove(provider) {
                    Ok(()) => {
                        form.saved[Form::slot(provider)] = Saved::Known(false);

                        ("Key removed.".to_string(), true)
                    }

                    Err(error) => (error, false),
                });
            }
        });
    });
}

fn sign_in(ui: &mut Ui, form: &mut Form, filter: &Filter) {
    // The outcome of a sign-in that was waiting for the browser.
    let outcome = match form.sign_in.as_ref().map(Receiver::try_recv) {
        Some(Ok(outcome)) => Some(outcome),
        Some(Err(TryRecvError::Disconnected)) => Some(Err("the sign-in stopped".to_string())),
        Some(Err(TryRecvError::Empty)) | None => None,
    };

    if let Some(outcome) = outcome {
        form.sign_in = None;
        form.signed_in = None;

        form.notice = Some(match outcome {
            Ok(()) => ("Signed in. Apply to start using it.".to_string(), true),
            Err(error) => (error, false),
        });
    }

    let waiting = form.sign_in.is_some();

    let status = if waiting {
        "Waiting for you to sign in, in your browser."
    } else {
        signed_in(form).unwrap_or("Not signed in.")
    };

    let pressed = rows::row(ui, filter, "ChatGPT account", status, |ui| {
        ui.add_enabled(!waiting, egui::Button::new("Sign in with ChatGPT"))
            .clicked()
    });

    if pressed == Some(true) {
        form.notice = None;
        form.sign_in = Some(start_sign_in());
    }

    if waiting {
        // Nothing else redraws the page while the browser has the focus.
        ui.ctx().request_repaint_after(Duration::from_millis(250));
    }
}

/// Which sign-in the provider would use. Finding out reads files, so
/// the answer is kept for a moment instead of being read on every frame.
fn signed_in(form: &mut Form) -> Option<&'static str> {
    if let Some((status, at)) = form.signed_in
        && at.elapsed() < SIGN_IN_FRESH
    {
        return status;
    }

    let status = cortex_providers::codex::sign_in_status();

    form.signed_in = Some((status, Instant::now()));

    status
}

/// Runs the sign-in away from the window, which must keep drawing while
/// the person is in the browser.
fn start_sign_in() -> Receiver<Result<(), String>> {
    let (outbox, inbox) = mpsc::channel();

    thread::spawn(move || {
        let outcome = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| error.to_string())
            .and_then(|runtime| {
                runtime
                    .block_on(cortex_providers::codex::login(|_| {}))
                    .map_err(|error| error.to_string())
            });

        // The page may have been left; the tokens are saved either way.
        let _ = outbox.send(outcome);
    });

    inbox
}
