//! The strip across the top: where you are and what you are looking at.

use eframe::egui::{self, Align, CornerRadius, Layout, Sense, Stroke, Ui, vec2};

use super::icons::{self, Icon};
use super::menu::{self, Opens};
use super::{Section, View};
use crate::theme::{self, control, palette, radius, space};

/// Below this much room right of the title, the views go into one menu.
const NARROW_WIDTH: f32 = 300.0;

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
/// shown on the right, and `details_open` whether the details panel is;
/// the two buttons at the right-hand end switch them.
#[allow(clippy::too_many_arguments)]
pub fn show(
    ui: &mut Ui,
    place: &Place<'_>,
    view: &mut View,
    section: &mut Section,
    sidebar_open: &mut bool,
    files_open: &mut bool,
    details_open: &mut bool,
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

        // The arrows show only once there is a chat to step to.
        if steps.back || steps.forward {
            ui.add_enabled_ui(steps.back, |ui| {
                if icons::button(ui, Icon::Back, "Back to the session before  ⌘[", false).clicked()
                {
                    step = Some(Step::Back);
                }
            });

            ui.add_enabled_ui(steps.forward, |ui| {
                if icons::button(ui, Icon::Forward, "Forward to the next session  ⌘]", false)
                    .clicked()
                {
                    step = Some(Step::Forward);
                }
            });
        }

        // The switch belongs to the sidebar: it sits at the sidebar's
        // right-hand edge and goes away with it.
        if *sidebar_open {
            let switch_width = SWITCH_PART.x * 2.0;
            let edge = ui.max_rect().left() + sidebar_width - 8.0;
            let gap = edge - switch_width - ui.cursor().left();

            ui.add_space(gap.max(8.0));

            section_switch(ui, section);
        }

        ui.add_space(space::L);
        ui.spacing_mut().item_spacing.x = space::S;

        // The title opens the menu of chats and layouts.
        let title = title(ui, place.project);

        menu::attach(&title, Opens::Down, |ui| {
            ui.set_min_width(menu::MIN_WIDTH + 40.0);
            workspace_menu(ui);
        });

        if place.busy {
            let (rect, response) = ui.allocate_exact_size(vec2(12.0, 18.0), Sense::hover());
            icons::activity_dot(ui, rect.center());
            response.on_hover_text("Working");
        }

        if place.demo {
            badge(ui, "Demo");
        }

        let narrow = ui.available_width() < NARROW_WIDTH;

        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = space::XS;
            ui.add_space(space::XS);

            toggle(
                ui,
                Icon::PanelRight,
                details_open,
                "Hide the details  ⇧⌘D",
                "Show the details  ⇧⌘D",
            );

            {
                let tooltip = if *files_open {
                    "Hide the files  ⌘E"
                } else {
                    "Show the files  ⌘E"
                };

                let files = icons::button(ui, Icon::Folder, tooltip, false);

                if *files_open {
                    ui.painter().rect_stroke(
                        files.rect,
                        CornerRadius::same(crate::theme::radius::M),
                        Stroke::new(1.0, palette::border()),
                        egui::StrokeKind::Inside,
                    );
                }

                if files.clicked() {
                    *files_open = !*files_open;
                }
            }

            ui.add_space(space::S);

            views(ui, view, narrow);
        });
    });

    step
}

/// The views the middle of the window can show. The two used most are
/// always in sight; the rest share the third place, which names the one
/// on screen and opens a menu of the others. In a narrow window all of
/// them go into one menu.
fn views(ui: &mut Ui, view: &mut View, narrow: bool) {
    const SHOWN: [(View, &str, &str); 1] = [(View::Conversation, "Chat", "The conversation")];

    const TUCKED: [(View, &str); 1] = [(View::Insights, "Insights")];

    // Settings belong to the chat they were opened from.
    let on_screen = if *view == View::Settings {
        View::Conversation
    } else {
        *view
    };

    let name_of = |wanted: View| {
        SHOWN
            .iter()
            .map(|(view, name, _)| (*view, *name))
            .chain(TUCKED)
            .find(|(view, _)| *view == wanted)
            .map_or("Chat", |(_, name)| name)
    };

    if narrow {
        let (_, chosen) =
            menu::dropdown(ui, None, name_of(on_screen), true, Opens::DownLeft, |ui| {
                let mut chosen = None;

                for (tab, name) in SHOWN
                    .iter()
                    .map(|(view, name, _)| (*view, *name))
                    .chain(TUCKED)
                {
                    if menu::item(ui, None, name, "", tab == on_screen).clicked() {
                        chosen = Some(tab);
                    }
                }

                chosen
            });

        if let Some(Some(chosen)) = chosen {
            *view = chosen;
        }

        return;
    }

    // Drawn right to left: the menu first, then the two in sight.
    let tucked = TUCKED.iter().find(|(tab, _)| *tab == on_screen);
    let label = tucked.map_or("More", |(_, name)| name);

    let (more, _) = segment(ui, label, tucked.is_some(), true);

    let chosen = menu::attach(&more, Opens::DownLeft, |ui| {
        let mut chosen = None;

        for (tab, name) in TUCKED {
            if menu::item(ui, None, name, "", tab == on_screen).clicked() {
                chosen = Some(tab);
            }
        }

        chosen
    });

    if let Some(Some(chosen)) = chosen {
        *view = chosen;
    }

    for (tab, name, help) in SHOWN.iter().rev() {
        let (response, _) = segment(ui, name, on_screen == *tab, false);

        if response.on_hover_text(*help).clicked() {
            *view = *tab;
        }
    }
}

/// One tab of the row of views: a pill, filled while it is the one on
/// screen. `menu` adds a chevron, for the tab that opens a menu.
fn segment(ui: &mut Ui, label: &str, chosen: bool, menu: bool) -> (egui::Response, egui::Rect) {
    let color = if chosen {
        palette::text()
    } else {
        palette::text_secondary()
    };

    let galley = ui
        .painter()
        .layout_no_wrap(label.to_string(), theme::small(), color);

    let chevron = if menu { space::S + 6.0 } else { 0.0 };
    let width = galley.size().x + 2.0 * space::M + chevron;

    let (rect, response) = ui.allocate_exact_size(vec2(width, control::SMALL), Sense::click());

    let open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&response));

    if chosen || open || response.hovered() {
        ui.painter().rect_filled(
            rect,
            CornerRadius::same(radius::M),
            if chosen {
                palette::raised()
            } else {
                palette::surface()
            },
        );
    }

    let text_color = if chosen || response.hovered() {
        palette::text()
    } else {
        palette::text_secondary()
    };

    ui.painter().galley(
        egui::pos2(
            rect.left() + space::M,
            rect.center().y - galley.size().y / 2.0,
        ),
        galley,
        text_color,
    );

    if menu {
        icons::chevron(
            ui,
            egui::pos2(rect.right() - space::M - 1.0, rect.center().y + 0.5),
            palette::text_muted(),
        );
    }

    let rect = response.rect;

    (
        response.on_hover_cursor(egui::CursorIcon::PointingHand),
        rect,
    )
}

/// A laptop, the app's name and a small arrow, as one thing to click.
fn title(ui: &mut Ui, project: Option<&str>) -> egui::Response {
    let text =
        ui.painter()
            .layout_no_wrap("Cortex".to_string(), theme::strong(13.0), palette::text());

    let width = 6.0 + 16.0 + 6.0 + text.size().x + 6.0 + 8.0 + 6.0;
    let (rect, response) = ui.allocate_exact_size(vec2(width, control::SMALL), Sense::click());

    let open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&response));

    if response.hovered() || open {
        ui.painter().rect_filled(
            rect,
            CornerRadius::same(crate::theme::radius::M),
            palette::surface(),
        );
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

    icons::chevron(
        ui,
        egui::pos2(left + text_width + 6.0 + 4.0, rect.center().y + 0.5),
        palette::text_muted(),
    );

    response.on_hover_text(match project {
        Some(project) => format!("Chats and layout  ·  {project}"),
        None => "Chats and layout".to_string(),
    })
}

/// The two-part switch between chats and coding projects.
fn section_switch(ui: &mut Ui, section: &mut Section) {
    const PART: egui::Vec2 = SWITCH_PART;

    let (rect, _) = ui.allocate_exact_size(vec2(PART.x * 2.0, PART.y), Sense::hover());

    ui.painter().rect_filled(
        rect,
        CornerRadius::same(crate::theme::radius::M),
        palette::sunken(),
    );

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
                CornerRadius::same(crate::theme::radius::S),
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

fn badge(ui: &mut Ui, label: &str) {
    let text = ui
        .painter()
        .layout_no_wrap(label.to_string(), theme::small(), palette::warning());

    let (rect, _) =
        ui.allocate_exact_size(text.size() + egui::vec2(12.0, 4.0), egui::Sense::hover());

    ui.painter().rect_filled(
        rect,
        CornerRadius::same(crate::theme::radius::S),
        palette::warning().gamma_multiply(0.16),
    );

    ui.painter()
        .galley(rect.min + egui::vec2(6.0, 2.0), text, palette::warning());
}

/// A button that switches a panel, outlined while the panel is shown.
fn toggle(ui: &mut Ui, icon: Icon, open: &mut bool, hide: &str, show: &str) {
    let response = icons::button(ui, icon, if *open { hide } else { show }, false);

    if *open {
        ui.painter().rect_stroke(
            response.rect,
            CornerRadius::same(radius::M),
            Stroke::new(1.0, palette::border()),
            egui::StrokeKind::Inside,
        );
    }

    if response.clicked() {
        *open = !*open;
    }
}
