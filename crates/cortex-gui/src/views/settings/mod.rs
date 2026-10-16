//! The settings page: which model to use, its key, how the app looks and
//! what the agent may do.
//!
//! The page is a list of sections on the left and the chosen section on
//! the right. Each section is a module of its own that draws its rows
//! from the settings being edited and says what, if anything, was asked
//! for. A search box over the list filters every section's rows by their
//! names and explanations at once.

mod about;
mod appearance;
mod capabilities;
mod models;
mod permissions;
mod provider;
mod rows;
mod workspace;

use std::sync::mpsc::Receiver;
use std::time::Instant;

use eframe::egui::{
    self, Align, CornerRadius, Layout, Rect, RichText, ScrollArea, Sense, Stroke, Ui, UiBuilder,
    vec2,
};

use super::icons::{self, Icon};
use crate::settings::{Provider, Settings};
use crate::theme::{self, palette, radius, size, space};

use rows::Filter;
pub(super) use rows::caption;

/// Widest the page grows.
const PAGE_WIDTH: f32 = 880.0;

/// Width of the list of sections, and the narrowest page that still has
/// room for it beside the rows. Narrower, the list becomes a menu above
/// them, beside the search box.
const LIST_WIDTH: f32 = 184.0;
const ROOMY_WIDTH: f32 = 640.0;

const HEADER_HEIGHT: f32 = 76.0;
const FOOTER_HEIGHT: f32 = 58.0;
const LIST_ROW_HEIGHT: f32 = 30.0;

/// Narrowest margin kept on either side of the page.
const MARGIN: f32 = 20.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    Provider,
    Models,
    Appearance,
    Permissions,
    Capabilities,
    Workspace,
    About,
}

impl Section {
    const ALL: [Self; 7] = [
        Self::Provider,
        Self::Models,
        Self::Appearance,
        Self::Permissions,
        Self::Capabilities,
        Self::Workspace,
        Self::About,
    ];

    fn title(self) -> &'static str {
        match self {
            Self::Provider => "Provider",
            Self::Models => "Models",
            Self::Appearance => "Appearance",
            Self::Permissions => "Permissions",
            Self::Capabilities => "Capabilities",
            Self::Workspace => "Workspace",
            Self::About => "About",
        }
    }
}

/// Whether a key is saved for a provider.
pub(super) enum Saved {
    /// Not asked yet.
    Unknown,

    /// The keychain is being asked, on another thread.
    Asking(Receiver<bool>),
    Known(bool),
}

/// The settings as they are being edited, before they are applied.
pub struct Form {
    pub(super) draft: Settings,
    section: Section,

    /// What is typed in the search box.
    query: String,

    /// What has been typed into the key field. Never shown again once
    /// saved.
    pub(super) key: String,

    /// Whether a key is saved for each provider, looked up once per visit.
    pub(super) saved: [Saved; 4],

    /// The models of the provider being edited, one per line.
    pub(super) models: String,

    /// The outcome of the last save, to show beside the buttons.
    pub(super) notice: Option<(String, bool)>,

    /// A ChatGPT sign-in that is waiting for the browser, and where its
    /// outcome will arrive.
    pub(super) sign_in: Option<Receiver<Result<(), String>>>,

    /// What the ChatGPT sign-in was found to be, and when it was looked
    /// at: it is read from a file, which is not done on every frame.
    pub(super) signed_in: Option<(Option<&'static str>, Instant)>,

    /// The rarely wanted capabilities are unfolded.
    pub(super) advanced: bool,
}

impl Form {
    pub fn new(settings: &Settings) -> Self {
        Self {
            models: settings.current().models.join("\n"),
            draft: settings.clone(),
            section: Section::Provider,
            query: String::new(),
            key: String::new(),
            saved: [
                Saved::Unknown,
                Saved::Unknown,
                Saved::Unknown,
                Saved::Unknown,
            ],
            notice: None,
            sign_in: None,
            signed_in: None,
            advanced: false,
        }
    }

    /// Opens the page on its themes.
    pub fn show_appearance(&mut self) {
        self.section = Section::Appearance;
    }

    pub(super) fn slot(provider: Provider) -> usize {
        Provider::ALL
            .iter()
            .position(|other| *other == provider)
            .unwrap_or(0)
    }

    /// Carries the models box into the draft.
    pub(super) fn read_models(&mut self) {
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

    /// Use this colour theme, from now on.
    Theme(&'static str),

    /// Get the project ready ahead of any task.
    Prepare,
}

/// What the page shows about the session, besides the settings.
pub struct Workspace {
    /// The agent is free, so the project can be got ready.
    pub can_prepare: bool,
}

/// Returns the requested navigation or settings change.
pub fn show(ui: &mut Ui, form: &mut Form, workspace: &Workspace) -> Option<Action> {
    let mut action = None;

    let full = ui.available_rect_before_wrap();
    let width = (full.width() - 2.0 * MARGIN).clamp(1.0, PAGE_WIDTH);
    let left = full.center().x - width / 2.0;

    let page = Rect::from_min_size(egui::pos2(left, full.top()), vec2(width, full.height()));

    ui.allocate_rect(full, Sense::hover());

    let header = Rect::from_min_size(page.min, vec2(width, HEADER_HEIGHT));

    let footer = Rect::from_min_max(
        egui::pos2(page.left(), page.bottom() - FOOTER_HEIGHT),
        page.max,
    );

    let body = Rect::from_min_max(header.left_bottom(), footer.right_top());

    if heading(ui, header) {
        action = Some(Action::Back);
    }

    let roomy = width >= ROOMY_WIDTH;

    let (list, rows) = if roomy {
        let (list, rows) = body.split_left_right_at_x(body.left() + LIST_WIDTH);

        (list, rows.with_min_x(rows.left() + space::XL))
    } else {
        let (list, rows) = body.split_top_bottom_at_y(body.top() + 40.0);

        (list, rows)
    };

    let mut list = child(ui, "sections", list);

    if roomy {
        sections(&mut list, form);
    } else {
        narrow_sections(&mut list, form);
    }

    let mut rows = child(ui, "rows", rows);

    ScrollArea::vertical()
        .id_salt("settings rows")
        .auto_shrink(false)
        .show(&mut rows, |ui| {
            ui.set_width(ui.available_width() - space::S);

            if let Some(asked) = content(ui, form, workspace) {
                action = Some(asked);
            }

            ui.add_space(space::XL);
        });

    ui.painter().hline(
        footer.x_range(),
        footer.top(),
        Stroke::new(1.0, palette::border()),
    );

    let mut foot = child(ui, "apply", footer.shrink2(vec2(0.0, space::M)));

    if apply_row(&mut foot, form) {
        form.read_models();

        action = Some(Action::Apply(Box::new(form.draft.clone())));
    }

    action
}

/// A part of the page with a place of its own.
fn child(ui: &mut Ui, name: &str, rect: Rect) -> Ui {
    let mut child = ui.new_child(
        UiBuilder::new()
            .id_salt(name)
            .max_rect(rect)
            .layout(Layout::top_down(Align::Min)),
    );

    child.set_clip_rect(rect.intersect(ui.clip_rect()));

    child
}

/// The page's title, with the way back to the chat in front of it.
/// Returns whether that was taken.
fn heading(ui: &mut Ui, header: Rect) -> bool {
    let back = Rect::from_min_size(header.min + vec2(0.0, 22.0), vec2(26.0, 26.0));

    let pressed = icons::button_at(
        ui,
        back,
        ui.id().with("settings-back"),
        Icon::Back,
        "Back to chat",
        false,
    )
    .clicked();

    ui.painter().text(
        back.right_center() + vec2(space::S, 0.0),
        egui::Align2::LEFT_CENTER,
        "Settings",
        theme::strong(size::DISPLAY - 2.0),
        palette::text(),
    );

    ui.painter().hline(
        header.x_range(),
        header.bottom() - space::S,
        Stroke::new(1.0, palette::border()),
    );

    pressed
}

/// The search box and the list of sections.
fn sections(ui: &mut Ui, form: &mut Form) {
    ui.spacing_mut().item_spacing.y = space::XXS;

    ui.add(
        egui::TextEdit::singleline(&mut form.query)
            .hint_text("Search settings")
            .desired_width(ui.available_width())
            .margin(egui::Margin::symmetric(10, 6)),
    );

    ui.add_space(space::M);

    let searching = !form.query.trim().is_empty();

    {
        for section in Section::ALL {
            // While searching every section is shown, so none is the
            // chosen one.
            let selected = form.section == section && !searching;

            if list_row(ui, section.title(), selected).clicked() {
                form.section = section;
                form.query.clear();
            }
        }
    }
}

/// The search box and, beside it, the sections as a menu: the list for
/// a page too narrow to have it on the left.
fn narrow_sections(ui: &mut Ui, form: &mut Form) {
    ui.horizontal(|ui| {
        egui::ComboBox::from_id_salt("settings section")
            .selected_text(form.section.title())
            .width(150.0)
            .show_ui(ui, |ui| {
                for section in Section::ALL {
                    if ui
                        .selectable_label(form.section == section, section.title())
                        .clicked()
                    {
                        form.section = section;
                        form.query.clear();
                    }
                }
            });

        ui.add(
            egui::TextEdit::singleline(&mut form.query)
                .hint_text("Search settings")
                .desired_width(ui.available_width().min(260.0))
                .margin(egui::Margin::symmetric(8, 5)),
        );
    });
}

/// One section in the list.
fn list_row(ui: &mut Ui, title: &str, selected: bool) -> egui::Response {
    let text = ui
        .painter()
        .layout_no_wrap(title.to_string(), theme::ui(), palette::text());

    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), LIST_ROW_HEIGHT), Sense::click());

    if selected || response.hovered() {
        let fill = if selected {
            palette::raised()
        } else {
            palette::surface()
        };

        ui.painter()
            .rect_filled(rect, CornerRadius::same(radius::M), fill);
    }

    if response.has_focus() {
        ui.painter().rect_stroke(
            rect,
            CornerRadius::same(radius::M),
            theme::focus_ring(),
            egui::StrokeKind::Inside,
        );
    }

    let color = if selected || response.hovered() {
        palette::text()
    } else {
        palette::text_secondary()
    };

    ui.painter().galley(
        egui::pos2(
            rect.left() + space::M,
            rect.center().y - text.size().y / 2.0,
        ),
        text,
        color,
    );

    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// The rows of the chosen section, or, while searching, the rows of
/// every section that match.
fn content(ui: &mut Ui, form: &mut Form, workspace: &Workspace) -> Option<Action> {
    let mut action = None;

    let query = form.query.trim().to_lowercase();

    if query.is_empty() {
        let section = form.section;
        let filter = Filter::everything();

        ui.add_space(space::XS);
        ui.label(RichText::new(section.title()).font(theme::strong(size::TITLE)));

        ui.add_space(space::XS);

        return draw(ui, form, workspace, section, &filter);
    }

    let filter = Filter::matching(&query);

    for section in Section::ALL {
        filter.under(section.title());

        if let Some(asked) = draw(ui, form, workspace, section, &filter) {
            action = Some(asked);
        }
    }

    if filter.shown() == 0 {
        ui.add_space(space::L);

        caption(
            ui,
            &format!("No setting matches \"{}\".", form.query.trim()),
        );
    }

    action
}

fn draw(
    ui: &mut Ui,
    form: &mut Form,
    workspace: &Workspace,
    section: Section,
    filter: &Filter,
) -> Option<Action> {
    match section {
        Section::Provider => provider::show(ui, form, filter),
        Section::Models => models::show(ui, form, filter),
        Section::Appearance => appearance::show(ui, form, filter),
        Section::Permissions => permissions::show(ui, form, filter),
        Section::Capabilities => capabilities::show(ui, form, filter),
        Section::Workspace => workspace::show(ui, form, workspace, filter),
        Section::About => about::show(ui, filter),
    }
}

/// The apply button and what happened last. Returns whether it was
/// pressed.
fn apply_row(ui: &mut Ui, form: &mut Form) -> bool {
    ui.horizontal(|ui| {
        let apply = theme::primary_button("Apply settings").min_size(vec2(128.0, 32.0));

        let pressed = ui.add(apply).clicked();

        if let Some((text, good)) = &form.notice {
            let color = if *good {
                palette::good()
            } else {
                palette::bad()
            };
            ui.label(RichText::new(text).font(theme::small()).color(color));
        }

        pressed
    })
    .inner
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOTHING: Workspace = Workspace { can_prepare: false };

    /// Draws the page in a window of `size` and returns what was asked
    /// for and everything that was drawn.
    fn frame(
        context: &egui::Context,
        form: &mut Form,
        size: egui::Vec2,
        events: Vec<egui::Event>,
    ) -> (Option<Action>, egui::FullOutput) {
        let mut action = None;

        let mut output = context.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, size)),
                events,
                ..Default::default()
            },
            |ui| {
                egui::CentralPanel::default().show(ui, |ui| {
                    action = show(ui, form, &NOTHING);
                });
            },
        );

        output.textures_delta.clear();

        (action, output)
    }

    /// Where `label` was drawn, if it was drawn in sight.
    fn visible(output: &egui::FullOutput, label: &str) -> Option<egui::Pos2> {
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
    }

    fn click(at: egui::Pos2) -> Vec<egui::Event> {
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };

        vec![egui::Event::PointerMoved(at), button(true), button(false)]
    }

    fn context() -> egui::Context {
        let context = egui::Context::default();

        theme::apply(&context);

        context
    }

    #[test]
    fn the_page_fills_the_window_and_the_way_back_works() {
        let context = context();
        let mut form = Form::new(&Settings::default());
        let size = vec2(1000.0, 800.0);

        let (_, output) = frame(&context, &mut form, size, Vec::new());

        let title = visible(&output, "Settings").unwrap();

        assert!(visible(&output, "Provider").is_some());
        assert!(visible(&output, "Apply settings").unwrap().y > 700.0);

        // The arrow sits just left of the title.
        let back = title + vec2(-space::S - 13.0, 12.0);

        let (action, _) = frame(&context, &mut form, size, click(back));

        assert!(matches!(action, Some(Action::Back)));
    }

    #[test]
    fn every_section_can_be_opened_from_the_list() {
        let context = context();
        let mut form = Form::new(&Settings::default());
        let size = vec2(1000.0, 700.0);

        for section in Section::ALL {
            let (_, output) = frame(&context, &mut form, size, Vec::new());
            let row = visible(&output, section.title()).unwrap();

            frame(&context, &mut form, size, click(row + vec2(4.0, 4.0)));

            assert_eq!(form.section, section);

            // Its title heads the rows.
            let (_, output) = frame(&context, &mut form, size, Vec::new());

            assert!(visible(&output, section.title()).is_some(), "{section:?}");
        }
    }

    #[test]
    fn nothing_is_clipped_in_the_smallest_window() {
        let context = context();
        let mut form = Form::new(&Settings::default());

        // The smallest window, less the sidebar at its narrowest.
        let size = vec2(900.0 - 196.0, 560.0 - 32.0);

        for section in Section::ALL {
            form.section = section;

            let (_, output) = frame(&context, &mut form, size, Vec::new());

            assert!(visible(&output, "Apply settings").is_some(), "{section:?}");
            assert!(visible(&output, section.title()).is_some(), "{section:?}");
        }

        // Narrower still, the list becomes a menu above the rows, and
        // the rows are still there.
        let narrow = vec2(520.0, 560.0);

        form.section = Section::Permissions;

        let (_, output) = frame(&context, &mut form, narrow, Vec::new());

        assert!(visible(&output, "Run commands in a sandbox").is_some());
        assert!(visible(&output, "Apply settings").is_some());
    }

    #[test]
    fn searching_shows_the_matching_rows_of_every_section() {
        let context = context();
        let mut form = Form::new(&Settings::default());
        let size = vec2(1000.0, 800.0);

        form.query = "sandbox".to_string();

        let (_, output) = frame(&context, &mut form, size, Vec::new());

        assert!(visible(&output, "Run commands in a sandbox").is_some());
        assert!(visible(&output, "Share memory between projects").is_none());

        // Found by its explanation as well as by its name.
        form.query = "screenshots".to_string();

        let (_, output) = frame(&context, &mut form, size, Vec::new());

        assert!(visible(&output, "Browser").is_some());
        assert!(visible(&output, "Run commands in a sandbox").is_none());

        form.query = "no such setting anywhere".to_string();

        let (_, output) = frame(&context, &mut form, size, Vec::new());

        assert!(visible(&output, "No setting matches \"no such setting anywhere\".").is_some());
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
    fn applying_hands_back_every_setting_that_was_edited() {
        let context = context();

        let mut settings = Settings {
            max_steps: 25,
            theme: "Nord".to_string(),
            ..Settings::default()
        };

        let mut form = Form::new(&settings);
        let size = vec2(1000.0, 800.0);

        form.draft.sandbox = false;

        let (_, output) = frame(&context, &mut form, size, Vec::new());
        let apply = visible(&output, "Apply settings").unwrap();

        let (action, _) = frame(&context, &mut form, size, click(apply + vec2(4.0, 4.0)));

        let Some(Action::Apply(applied)) = action else {
            panic!("nothing was applied");
        };

        settings.sandbox = false;

        assert_eq!(*applied, settings);
    }
}
