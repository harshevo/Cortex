//! The strip across the top: where you are and what you are looking at.

use eframe::egui::{self, Align, Color32, CornerRadius, Layout, RichText, Sense, Stroke, Ui, vec2};

use super::icons::{self, Icon};
use super::{Section, View};
use crate::theme::{self, palette};

/// Room left for the window's close, minimise and zoom buttons, which sit
/// on top of this bar.
const WINDOW_BUTTONS_WIDTH: f32 = 70.0;

/// What the bar says about where the agent is.
pub struct Place<'a> {
    /// The project's name, or `None` for a session outside any project.
    pub project: Option<&'a str>,

    /// The agent is the scripted stand-in, not a real model.
    pub demo: bool,
    pub memories: usize,
    pub busy: bool,
}

/// Draws the bar. Returns whether the person asked for the project to be
/// got ready, which `can_prepare` allows.
pub fn show(
    ui: &mut Ui,
    place: &Place<'_>,
    view: &mut View,
    section: &mut Section,
    sidebar_open: &mut bool,
    can_prepare: bool,
    workspace_menu: impl FnOnce(&mut Ui),
) -> bool {
    let mut prepare = false;

    ui.horizontal_centered(|ui| {
        ui.add_space(WINDOW_BUTTONS_WIDTH);

        let tooltip = if *sidebar_open {
            "Hide the sidebar  ⌘B"
        } else {
            "Show the sidebar  ⌘B"
        };

        if icons::button(ui, Icon::Sidebar, tooltip, false).clicked() {
            *sidebar_open = !*sidebar_open;
        }

        section_switch(ui, section);
        workspace_menu(ui);

        ui.add_space(4.0);

        ui.label(
            RichText::new("Cortex")
                .font(theme::strong(14.0))
                .color(palette::TEXT),
        );

        if place.busy {
            let (rect, response) = ui.allocate_exact_size(vec2(12.0, 18.0), Sense::hover());
            icons::activity_dot(ui, rect.center());
            response.on_hover_text("Task in progress");
        }

        ui.label(
            RichText::new(place.project.unwrap_or("No project")).color(palette::TEXT_SECONDARY),
        );

        if place.demo {
            badge(ui, "Demo");
        }

        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add_space(6.0);

            let memory_label = format!("Memory space  {}", place.memories);

            if tab(ui, &memory_label, *view == View::MemorySpace).clicked() {
                *view = View::MemorySpace;
            }

            if tab(ui, "Conversation", *view == View::Conversation).clicked() {
                *view = View::Conversation;
            }

            ui.add_space(8.0);

            let button = ui
                .add_enabled(can_prepare, |ui: &mut Ui| tab(ui, "Prepare", false))
                .on_hover_text(
                    "Get this project ready ahead of any task: parse it, embed it, and have \
                     the model write a short description that every later task is shown.",
                );

            prepare = button.clicked();
        });
    });

    prepare
}

/// The two-part switch between chats and coding projects.
fn section_switch(ui: &mut Ui, section: &mut Section) {
    const PART: egui::Vec2 = vec2(26.0, 24.0);

    let (rect, _) = ui.allocate_exact_size(vec2(PART.x * 2.0, PART.y), Sense::hover());

    ui.painter()
        .rect_filled(rect, CornerRadius::same(6), palette::SUNKEN);

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
                palette::RAISED,
            );
        }

        if response.hovered() && !selected {
            ui.painter()
                .rect_filled(part_rect.shrink(2.0), 4, palette::SURFACE);
        }
        let color = if selected || response.hovered() {
            palette::TEXT
        } else {
            palette::TEXT_MUTED
        };

        icons::paint(ui, part_rect.center(), icon, color);

        if response.on_hover_text(tooltip).clicked() {
            *section = part;
        }
    }
}

fn tab(ui: &mut Ui, label: &str, selected: bool) -> egui::Response {
    let (fill, text) = if selected {
        (palette::RAISED, palette::TEXT)
    } else {
        (Color32::TRANSPARENT, palette::TEXT_SECONDARY)
    };

    let button = egui::Button::new(RichText::new(label).color(text))
        .fill(fill)
        .stroke(Stroke::NONE)
        .corner_radius(CornerRadius::same(7));

    ui.add(button)
}

fn badge(ui: &mut Ui, label: &str) {
    let text = ui
        .painter()
        .layout_no_wrap(label.to_string(), theme::small(), palette::WARNING);

    let (rect, _) =
        ui.allocate_exact_size(text.size() + egui::vec2(12.0, 4.0), egui::Sense::hover());

    ui.painter().rect_filled(
        rect,
        CornerRadius::same(4),
        palette::WARNING.gamma_multiply(0.16),
    );

    ui.painter()
        .galley(rect.min + egui::vec2(6.0, 2.0), text, palette::WARNING);
}
