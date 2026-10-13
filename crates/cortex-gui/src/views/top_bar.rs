//! The strip across the top: where you are and what you are looking at.

use eframe::egui::{self, Align, CornerRadius, Layout, Sense, Stroke, Ui, vec2};

use super::icons::{self, Icon};
use super::{Section, View};
use crate::theme::{self, palette};

/// Room left for the window's close, minimise and zoom buttons, which sit
/// on top of this bar.
const WINDOW_BUTTONS_WIDTH: f32 = 70.0;

/// A step through the chats that have been on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Back,
    Forward,
}

/// Which of the two steps there is somewhere to go with.
#[derive(Debug, Clone, Copy, Default)]
pub struct Steps {
    pub back: bool,
    pub forward: bool,
}

const SWITCH_PART: egui::Vec2 = vec2(30.0, 24.0);

/// What the bar says about where the agent is.
pub struct Place<'a> {
    /// The project's name, or `None` for a session outside any project.
    pub project: Option<&'a str>,

    /// The agent is the scripted stand-in, not a real model.
    pub demo: bool,
    pub busy: bool,
}

/// Draws the bar. `files_open` is whether the files of the project are
/// shown on the right; the button at the right-hand end switches it.
pub fn show(
    ui: &mut Ui,
    place: &Place<'_>,
    view: &mut View,
    section: &mut Section,
    sidebar_open: &mut bool,
    files_open: &mut bool,
    sidebar_width: f32,
    steps: Steps,
    workspace_menu: impl FnOnce(&mut Ui),
) -> Option<Step> {
    let mut step = None;

    ui.horizontal_centered(|ui| {
        // A full-screen window has no buttons of its own to make room for.
        let full_screen = ui.input(|input| input.viewport().fullscreen.unwrap_or(false));

        ui.add_space(if full_screen {
            4.0
        } else {
            WINDOW_BUTTONS_WIDTH
        });

        let tooltip = if *sidebar_open {
            "Hide the sidebar  ⌘B"
        } else {
            "Show the sidebar  ⌘B"
        };

        if icons::button(ui, Icon::Sidebar, tooltip, false).clicked() {
            *sidebar_open = !*sidebar_open;
        }

        ui.spacing_mut().item_spacing.x = 2.0;

        ui.add_enabled_ui(steps.back, |ui| {
            if icons::button(ui, Icon::Back, "Back to the session before  ⌘[", false).clicked() {
                step = Some(Step::Back);
            }
        });

        ui.add_enabled_ui(steps.forward, |ui| {
            if icons::button(ui, Icon::Forward, "Forward to the next session  ⌘]", false).clicked()
            {
                step = Some(Step::Forward);
            }
        });

        // The switch belongs to the sidebar: it sits at the sidebar's
        // right-hand edge and goes away with it.
        if *sidebar_open {
            let switch_width = SWITCH_PART.x * 2.0;
            let edge = ui.max_rect().left() + sidebar_width - 8.0;
            let gap = edge - switch_width - ui.cursor().left();

            ui.add_space(gap.max(8.0));

            section_switch(ui, section);
        }

        ui.add_space(14.0);
        ui.spacing_mut().item_spacing.x = 6.0;

        // The title opens the menu of chats and layouts.
        let title = title(ui);

        egui::Popup::menu(&title).show(workspace_menu);

        if place.busy {
            let (rect, response) = ui.allocate_exact_size(vec2(12.0, 18.0), Sense::hover());
            icons::activity_dot(ui, rect.center());
            response.on_hover_text("Task in progress");
        }

        chip(ui, place.project.unwrap_or("No project"));

        if place.demo {
            badge(ui, "Demo");
        }

        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add_space(6.0);

            let tooltip = if *files_open {
                "Hide the files  ⌘E"
            } else {
                "Show the files  ⌘E"
            };

            let files = icons::button(ui, Icon::Folder, tooltip, false);

            if *files_open {
                ui.painter().rect_stroke(
                    files.rect,
                    CornerRadius::same(6),
                    Stroke::new(1.0, palette::border()),
                    egui::StrokeKind::Inside,
                );
            }

            if files.clicked() {
                *files_open = !*files_open;
            }

            // The memory space has no tab any more, so this is the way
            // out of it.
            if matches!(*view, View::MemorySpace | View::Living)
                && icons::button(ui, Icon::Back, "Back to the chat", false).clicked()
            {
                *view = View::Conversation;
            }
        });
    });

    step
}

/// A laptop, the app's name and a small arrow, as one thing to click.
fn title(ui: &mut Ui) -> egui::Response {
    let text =
        ui.painter()
            .layout_no_wrap("Cortex".to_string(), theme::strong(13.0), palette::text());

    let width = 6.0 + 16.0 + 6.0 + text.size().x + 6.0 + 8.0 + 6.0;
    let (rect, response) = ui.allocate_exact_size(vec2(width, 24.0), Sense::click());

    if response.hovered() {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(6), palette::surface());
    }

    let mut left = rect.left() + 6.0;

    icons::paint(
        ui,
        egui::pos2(left + 8.0, rect.center().y),
        Icon::Laptop,
        palette::text_secondary(),
    );

    left += 16.0 + 6.0;

    let text_width = text.size().x;

    ui.painter().galley(
        egui::pos2(left, rect.center().y - text.size().y / 2.0),
        text,
        palette::text(),
    );

    let centre = egui::pos2(left + text_width + 6.0 + 4.0, rect.center().y + 0.5);

    ui.painter().line(
        vec![
            centre + vec2(-3.0, -1.5),
            centre + vec2(0.0, 1.5),
            centre + vec2(3.0, -1.5),
        ],
        Stroke::new(1.2, palette::text_secondary()),
    );

    response.on_hover_text("Chats and layout")
}

/// The two-part switch between chats and coding projects.
fn section_switch(ui: &mut Ui, section: &mut Section) {
    const PART: egui::Vec2 = SWITCH_PART;

    let (rect, _) = ui.allocate_exact_size(vec2(PART.x * 2.0, PART.y), Sense::hover());

    ui.painter()
        .rect_filled(rect, CornerRadius::same(6), palette::sunken());

    let parts = [
        (
            Section::Chats,
            Icon::Chat,
            "Chats: sessions outside any project",
        ),
        (
            Section::Code,
            Icon::Code,
            "Code: project folders and their sessions",
        ),
    ];

    for (position, (part, icon, tooltip)) in parts.into_iter().enumerate() {
        let part_rect =
            egui::Rect::from_min_size(rect.min + vec2(PART.x * position as f32, 0.0), PART);

        let response = ui.interact(
            part_rect,
            ui.id().with(("section", position)),
            Sense::click(),
        );

        let selected = *section == part;

        if selected {
            ui.painter().rect_filled(
                part_rect.shrink(2.0),
                CornerRadius::same(4),
                palette::raised(),
            );
        }

        if response.hovered() && !selected {
            ui.painter()
                .rect_filled(part_rect.shrink(2.0), 4, palette::surface());
        }
        let color = if selected || response.hovered() {
            palette::text()
        } else {
            palette::text_muted()
        };

        icons::paint(ui, part_rect.center(), icon, color);

        if response.on_hover_text(tooltip).clicked() {
            *section = part;
        }
    }
}

/// The project's name on a quiet rounded ground.
fn chip(ui: &mut Ui, label: &str) {
    let text =
        ui.painter()
            .layout_no_wrap(label.to_string(), theme::small(), palette::text_secondary());

    let (rect, _) = ui.allocate_exact_size(text.size() + vec2(14.0, 6.0), Sense::hover());

    ui.painter()
        .rect_filled(rect, CornerRadius::same(6), palette::surface());

    ui.painter()
        .galley(rect.min + vec2(7.0, 3.0), text, palette::text_secondary());
}

fn badge(ui: &mut Ui, label: &str) {
    let text = ui
        .painter()
        .layout_no_wrap(label.to_string(), theme::small(), palette::warning());

    let (rect, _) =
        ui.allocate_exact_size(text.size() + egui::vec2(12.0, 4.0), egui::Sense::hover());

    ui.painter().rect_filled(
        rect,
        CornerRadius::same(4),
        palette::warning().gamma_multiply(0.16),
    );

    ui.painter()
        .galley(rect.min + egui::vec2(6.0, 2.0), text, palette::warning());
}
