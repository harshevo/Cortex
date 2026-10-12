//! The settings page: which model to use, its key, and what the agent may
//! do.

use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;
use std::time::Duration;

use clap::ValueEnum;
use cortex_cli::features::Feature;
use eframe::egui::{self, Color32, CornerRadius, RichText, ScrollArea, Stroke, Ui};

use super::icons::{self, Icon};
use crate::settings::{self, Provider, Settings, secrets};
use crate::theme::{self, palette};

/// Widest the page grows.
const PAGE_WIDTH: f32 = 760.0;

const FIELD_WIDTH: f32 = 380.0;

#[derive(Clone, Copy, PartialEq)]
enum Section {
    Provider,
    Permissions,
    Capabilities,
}

/// The settings as they are being edited, before they are applied.
pub struct Form {
    draft: Settings,
    section: Section,

    /// What has been typed into the key field. Never shown again once
    /// saved.
    key: String,

    /// Whether a key is saved for each provider, looked up once per visit.
    saved: [Option<bool>; 4],

    /// The models of the provider being edited, one per line.
    models: String,

    /// The outcome of the last save, to show beside the buttons.
    notice: Option<(String, bool)>,

    /// A ChatGPT sign-in that is waiting for the browser, and where its
    /// outcome will arrive.
    sign_in: Option<Receiver<Result<(), String>>>,
}

impl Form {
    pub fn new(settings: &Settings) -> Self {
        Self {
            models: settings.current().models.join("\n"),
            draft: settings.clone(),
            section: Section::Provider,
            key: String::new(),
            saved: [None; 4],
            notice: None,
            sign_in: None,
        }
    }

    fn slot(provider: Provider) -> usize {
        Provider::ALL
            .iter()
            .position(|other| *other == provider)
            .unwrap_or(0)
    }

    /// Whether a key is saved for `provider`. The keychain is asked once.
    fn has_key(&mut self, provider: Provider) -> bool {
        *self.saved[Self::slot(provider)].get_or_insert_with(|| secrets::load(provider).is_some())
    }

    /// Carries the models box into the draft.
    fn read_models(&mut self) {
        let models: Vec<String> = self
            .models
            .lines()
            .map(str::trim)
            .filter(|model| !model.is_empty())
            .map(str::to_string)
            .collect();

        let current = self.draft.current_mut();

        // The model in use has to be one that still exists.
        if !models.contains(&current.model) {
            current.model = models.first().cloned().unwrap_or_default();
        }

        current.models = models;
    }
}

pub enum Action {
    Apply(Box<Settings>),
    Back,
}

/// Returns the requested navigation or settings change.
pub fn show(ui: &mut Ui, form: &mut Form) -> Option<Action> {
    let mut applied = None;
    let width = (ui.available_width() - 72.0).clamp(1.0, PAGE_WIDTH);
    let margin = ((ui.available_width() - width) / 2.0).max(0.0);
    let height = ui.available_height();
    let bottom = ui.max_rect().bottom();

    ui.horizontal(|ui| {
        ui.add_space(margin);
        ui.vertical(|ui| {
            ui.set_width(width);
            ui.set_height(height);
            ui.add_space(22.0);
            let heading = ui.label(RichText::new("Settings").font(theme::strong(24.0)));
            let back_rect = egui::Rect::from_center_size(
                heading.rect.left_center() - egui::vec2(23.0, 0.0),
                egui::vec2(26.0, 26.0),
            );
            if icons::button_at(
                ui,
                back_rect,
                ui.id().with("settings-back"),
                Icon::Back,
                "Back to chat",
                false,
            )
            .clicked()
            {
                applied = Some(Action::Back);
            }
            caption(ui, "Configure your models and how Cortex works with you.");
            ui.add_space(20.0);
            ui.horizontal_wrapped(|ui| {
                for (section, label) in [
                    (Section::Provider, "Models & accounts"),
                    (Section::Permissions, "Permissions"),
                    (Section::Capabilities, "Capabilities"),
                ] {
                    if choice(ui, label, form.section == section).clicked() {
                        form.section = section;
                    }
                }
            });
            ui.add_space(12.0);
            ui.separator();

            ScrollArea::vertical()
                .id_salt(("settings-content", form.section as u8))
                .auto_shrink(false)
                .max_height((bottom - ui.cursor().top() - 94.0).max(80.0))
                .show(ui, |ui| {
                    ui.set_width(width);
                    match form.section {
                        Section::Provider => provider_section(ui, form),
                        Section::Permissions => permissions_section(ui, form),
                        Section::Capabilities => capabilities_section(ui, form),
                    }
                    ui.add_space(24.0);
                });

            ui.separator();
            ui.add_space(10.0);
            if apply_row(ui, form) {
                form.read_models();
                applied = Some(Action::Apply(Box::new(form.draft.clone())));
            }
            ui.add_space(12.0);
        });
    });

    applied
}

fn section(ui: &mut Ui, title: &str) {
    ui.add_space(22.0);
    ui.label(RichText::new(title).font(theme::strong(14.0)));
    ui.add_space(2.0);
}

fn caption(ui: &mut Ui, text: &str) {
    ui.label(
        RichText::new(text)
            .font(theme::small())
            .color(palette::TEXT_MUTED),
    );
}

fn field_label(ui: &mut Ui, text: &str) {
    ui.add_space(6.0);

    ui.label(
        RichText::new(text)
            .font(theme::small())
            .color(palette::TEXT_SECONDARY),
    );
}

fn text_field(ui: &mut Ui, value: &mut String, hint: &str) {
    ui.add(
        egui::TextEdit::singleline(value)
            .hint_text(hint)
            .desired_width(ui.available_width().min(FIELD_WIDTH + 160.0))
            .margin(egui::Margin::symmetric(8, 6)),
    );
}

fn provider_section(ui: &mut Ui, form: &mut Form) {
    section(ui, "Model provider");
    caption(
        ui,
        "Choose a provider, then configure its connection and available models.",
    );
    ui.add_space(12.0);

    let before = form.draft.provider;

    ui.horizontal_wrapped(|ui| {
        for provider in Provider::ALL {
            let selected = form.draft.provider == provider;

            if choice(ui, provider.label(), selected).clicked() {
                form.read_models();

                form.draft.provider = provider;
            }
        }
    });

    let provider = form.draft.provider;

    if provider != before {
        form.models = form.draft.current().models.join("\n");
        form.key.clear();
        form.notice = None;
    }

    if provider.has_address() {
        field_label(ui, "Address of the API");

        let hint = match provider {
            Provider::Local => "http://127.0.0.1:11434/v1",
            _ => "https://api.example.com/v1",
        };

        text_field(ui, &mut form.draft.current_mut().base_url, hint);
    }

    if provider.needs_key() {
        key_field(ui, form, provider);
    }

    if provider == Provider::Codex {
        sign_in_field(ui, form);
    }

    field_label(ui, "Models, one per line");

    ui.add(
        egui::TextEdit::multiline(&mut form.models)
            .font(theme::mono())
            .desired_rows(5)
            .desired_width(ui.available_width().min(FIELD_WIDTH + 160.0))
            .margin(egui::Margin::symmetric(8, 6)),
    );

    caption(
        ui,
        "These are offered in the model selector under the message box.",
    );
}

fn key_field(ui: &mut Ui, form: &mut Form, provider: Provider) {
    field_label(ui, "API key");

    let has_key = form.has_key(provider);

    ui.vertical(|ui| {
        let hint = if has_key {
            "A key is saved. Type a new one to replace it."
        } else {
            "Paste your key"
        };

        ui.add(
            egui::TextEdit::singleline(&mut form.key)
                .password(true)
                .hint_text(hint)
                .desired_width(ui.available_width().min(FIELD_WIDTH + 160.0))
                .margin(egui::Margin::symmetric(8, 6)),
        );

        let typed = !form.key.trim().is_empty();

        ui.horizontal(|ui| {
            if ui
                .add_enabled(typed, egui::Button::new("Save key"))
                .clicked()
            {
                form.notice = Some(match secrets::save(provider, &form.key) {
                    Ok(()) => {
                        form.saved[Form::slot(provider)] = Some(true);
                        form.key.clear();

                        ("Key saved. Apply to start using it.".to_string(), true)
                    }

                    Err(error) => (error, false),
                });
            }

            if has_key && ui.button("Remove").clicked() {
                form.notice = Some(match secrets::remove(provider) {
                    Ok(()) => {
                        form.saved[Form::slot(provider)] = Some(false);

                        ("Key removed.".to_string(), true)
                    }

                    Err(error) => (error, false),
                });
            }
        });
    });

    let fallback = provider.key_variable().unwrap_or_default();

    let status = match (has_key, secrets::from_environment(provider).is_some()) {
        (true, _) => "Kept in your Mac's keychain, not in a file.".to_string(),
        (false, true) => format!("No key saved. Using the one in {fallback}."),
        (false, false) => format!("No key saved, and {fallback} is not set."),
    };

    caption(ui, &status);
}

fn sign_in_field(ui: &mut Ui, form: &mut Form) {
    field_label(ui, "Account");

    // The outcome of a sign-in that was waiting for the browser.
    let outcome = match form.sign_in.as_ref().map(Receiver::try_recv) {
        Some(Ok(outcome)) => Some(outcome),
        Some(Err(TryRecvError::Disconnected)) => Some(Err("the sign-in stopped".to_string())),
        Some(Err(TryRecvError::Empty)) | None => None,
    };

    if let Some(outcome) = outcome {
        form.sign_in = None;

        form.notice = Some(match outcome {
            Ok(()) => ("Signed in. Apply to start using it.".to_string(), true),
            Err(error) => (error, false),
        });
    }

    let waiting = form.sign_in.is_some();

    if ui
        .add_enabled(!waiting, egui::Button::new("Sign in with ChatGPT"))
        .clicked()
    {
        form.notice = None;
        form.sign_in = Some(start_sign_in());
    }

    if waiting {
        caption(ui, "Waiting for you to sign in, in your browser.");

        // Nothing else redraws the page while the browser has the focus.
        ui.ctx().request_repaint_after(Duration::from_millis(250));
    } else {
        let status = cortex_providers::codex::sign_in_status().unwrap_or("Not signed in.");

        caption(ui, status);
    }
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

fn permissions_section(ui: &mut Ui, form: &mut Form) {
    section(ui, "Permissions");
    caption(
        ui,
        "Control approval requests and the amount of work an agent can do.",
    );
    ui.add_space(12.0);

    switch_row(
        ui,
        &mut form.draft.approve_everything,
        RichText::new("Edit files and run commands without asking"),
        "Off, Cortex asks before each change. On, it does not ask at all.",
        true,
    );

    field_label(ui, "Most model calls per instruction");

    ui.add(
        egui::DragValue::new(&mut form.draft.max_steps)
            .range(1..=500)
            .speed(0.5),
    );
    caption(
        ui,
        "The agent pauses when it reaches this limit. Allowed range: 1–500 calls.",
    );
}

fn capabilities_section(ui: &mut Ui, form: &mut Form) {
    section(ui, "Capabilities");

    caption(
        ui,
        "Enable the tools and reasoning features your agents can use.",
    );

    ui.add_space(12.0);

    for feature in Feature::value_variants() {
        // The memory space shows the agent's memory, so it is always kept.
        let always_on = *feature == Feature::Memory;

        let mut on = always_on || form.draft.has(*feature);

        let name = RichText::new(settings::capability_name(*feature)).font(theme::mono());
        let help = one_sentence(&settings::capability_help(*feature));

        if switch_row(ui, &mut on, name, &help, !always_on) {
            form.draft.set(*feature, on);
        }

        if *feature == Feature::Rollouts && on {
            attempts_rows(ui, form);
        }
        ui.add_space(10.0);
        ui.separator();
    }
}

/// How many attempts the `rollouts` capability makes, and how.
fn attempts_rows(ui: &mut Ui, form: &mut Form) {
    ui.indent("attempts", |ui| {
        field_label(ui, "Attempts at most");

        ui.add(
            egui::DragValue::new(&mut form.draft.rollouts)
                .range(2..=8)
                .speed(0.1),
        );

        switch_row(
            ui,
            &mut form.draft.rollouts_parallel,
            RichText::new("Make the attempts at the same time"),
            "Off, a second attempt is made only when the first is not convincing. On, all are \
             made at once and the best is kept: faster, and it always costs every attempt.",
            true,
        );
    });
}

/// A switch with a title and a line of explanation beside it. Returns
/// whether the switch was flipped.
fn switch_row(ui: &mut Ui, on: &mut bool, title: RichText, help: &str, enabled: bool) -> bool {
    ui.add_space(4.0);

    ui.horizontal(|ui| {
        let flipped = ui.add_enabled_ui(enabled, |ui| switch(ui, on)).inner;

        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 1.0;

            ui.label(title.color(palette::TEXT));

            // The explanation wraps inside the page rather than running on.
            ui.add(
                egui::Label::new(
                    RichText::new(help)
                        .font(theme::small())
                        .color(palette::TEXT_MUTED),
                )
                .wrap(),
            );
        });

        flipped
    })
    .inner
}

/// An on/off switch. Returns whether it was flipped.
fn switch(ui: &mut Ui, on: &mut bool) -> bool {
    const SIZE: egui::Vec2 = egui::vec2(30.0, 18.0);

    let (rect, response) = ui.allocate_exact_size(SIZE, egui::Sense::click());

    if response.clicked() {
        *on = !*on;
    }

    // The knob slides rather than jumps.
    let position = ui.ctx().animate_bool(response.id, *on);

    let track = if *on {
        palette::ACCENT
    } else {
        palette::RAISED
    };
    let track = if ui.is_enabled() {
        track
    } else {
        track.gamma_multiply(0.5)
    };

    let radius = rect.height() / 2.0;

    ui.painter().rect_filled(rect, radius, track);

    let knob = egui::pos2(
        egui::lerp(rect.left() + radius..=rect.right() - radius, position),
        rect.center().y,
    );

    ui.painter()
        .circle_filled(knob, radius - 2.5, palette::TEXT);

    response.clicked()
}

/// The first sentence of a longer explanation.
fn one_sentence(text: &str) -> String {
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");

    match flat.split_once(". ") {
        Some((first, _)) => format!("{first}."),
        None => flat,
    }
}

/// The apply button and what happened last. Returns whether it was
/// pressed.
fn apply_row(ui: &mut Ui, form: &mut Form) -> bool {
    ui.horizontal_wrapped(|ui| {
        let apply = egui::Button::new(RichText::new("Apply settings").color(palette::SUNKEN))
            .min_size(egui::vec2(120.0, 34.0))
            .fill(palette::ACCENT);

        let pressed = ui.add(apply).clicked();

        if let Some((text, good)) = &form.notice {
            let color = if *good { palette::GOOD } else { palette::BAD };

            ui.label(RichText::new(text).font(theme::small()).color(color));
        }

        pressed
    })
    .inner
}

/// One option of a row of choices, of which one is selected.
fn choice(ui: &mut Ui, label: &str, selected: bool) -> egui::Response {
    let (fill, text, stroke) = if selected {
        (
            palette::RAISED,
            palette::TEXT,
            Stroke::new(1.0, palette::ACCENT),
        )
    } else {
        (
            Color32::TRANSPARENT,
            palette::TEXT_SECONDARY,
            Stroke::new(1.0, palette::BORDER),
        )
    };

    ui.add(
        egui::Button::new(RichText::new(label).color(text))
            .fill(fill)
            .stroke(stroke)
            .corner_radius(CornerRadius::same(7)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_body_uses_the_window_height() {
        let context = egui::Context::default();
        theme::apply(&context);
        let mut form = Form::new(&Settings::default());
        let mut output = context.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1000.0, 800.0),
                )),
                ..Default::default()
            },
            |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    show(ui, &mut form);
                });
            },
        );
        output.textures_delta.clear();
        let visible_label = |label: &str| {
            output.shapes.iter().find_map(|clipped| {
                if let egui::epaint::Shape::Text(text) = &clipped.shape
                    && text.galley.job.text == label
                    && clipped.clip_rect.contains(text.pos)
                {
                    Some(text.pos)
                } else {
                    None
                }
            })
        };

        let back = visible_label("Settings").unwrap() + egui::vec2(-23.0, 14.0);
        assert!(visible_label("Models, one per line").is_some());
        assert!(visible_label("Apply settings").unwrap().y > 650.0);
        let mut action = None;
        let _ = context.run_ui(
            egui::RawInput {
                events: vec![
                    egui::Event::PointerMoved(back),
                    egui::Event::PointerButton {
                        pos: back,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: egui::Modifiers::NONE,
                    },
                    egui::Event::PointerButton {
                        pos: back,
                        button: egui::PointerButton::Primary,
                        pressed: false,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
                ..Default::default()
            },
            |ui| {
                egui::CentralPanel::default().show(ui, |ui| action = show(ui, &mut form));
            },
        );
        assert!(matches!(action, Some(Action::Back)));
    }

    #[test]
    fn the_models_box_becomes_the_list_and_keeps_a_valid_choice() {
        let mut form = Form::new(&Settings::default());

        form.models = "  model-a \n\nmodel-b\n".to_string();
        form.read_models();

        let current = form.draft.current();

        assert_eq!(current.models, ["model-a", "model-b"]);
        assert_eq!(current.model, "model-a");
    }

    #[test]
    fn help_is_cut_to_its_first_sentence() {
        assert_eq!(
            one_sentence("Trim old output. It saves\ntokens."),
            "Trim old output."
        );
        assert_eq!(one_sentence("No full stop"), "No full stop");
    }
}
