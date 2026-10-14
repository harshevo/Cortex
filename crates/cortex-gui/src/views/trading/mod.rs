//! The trading screen: an instrument live from Dhan, or bars from a file
//! or a chart picture, with the next ten candles as the chosen network
//! forecasts them, a plan for each case, how earlier forecasts came out,
//! and how every network measures up.
//!
//! The work is `cortex-market`'s; this draws its snapshot.

mod chart;
mod chat_panel;
mod flow_panel;
mod format;
mod panel;
mod tables;
mod widgets;

use std::path::PathBuf;
use std::sync::Arc;

use cortex_market::{
    Brain, Connection, FeedState, Instrument, Interval, Market, ProviderOracle, Snapshot, Source,
    Work,
};
use eframe::egui::{self, Align, CornerRadius, Layout, RichText, Sense, Stroke, Ui, pos2, vec2};

use crate::settings::Settings;
use crate::theme::{self, palette};
use crate::views::icons::Icon;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Chart,
    Forecast,
    Outcomes,
    Metrics,
    Flow,
}

impl Tab {
    const ALL: [Tab; 5] = [
        Tab::Chart,
        Tab::Forecast,
        Tab::Outcomes,
        Tab::Metrics,
        Tab::Flow,
    ];

    fn name(self) -> &'static str {
        match self {
            Tab::Chart => "Chart",
            Tab::Forecast => "Forecast",
            Tab::Outcomes => "Outcomes",
            Tab::Metrics => "Metrics",
            Tab::Flow => "Order flow",
        }
    }
}

pub struct TradingView {
    market: Option<Market>,
    subscription: Option<cortex_market::desktop::Subscription>,
    snapshot: Snapshot,
    tab: Tab,

    /// The instrument picker, while open, and what is typed in it.
    picking: bool,
    query: String,

    /// The Dhan form, while open.
    connecting: bool,
    client_id: String,
    token: String,

    /// The price scale typed for a picture.
    picture: Option<(String, Vec<u8>)>,
    top_price: String,
    bottom_price: String,

    problem: Option<String>,
    chart: chart::ChartState,

    /// The model settings the language model was last set up from.
    oracle_for: Option<String>,
    chat: Option<Arc<cortex_market::chat::TradingChat>>,
    chat_view: chat_panel::ChatView,
    layout: TradingLayout,

    /// Too narrow last frame for the chat to sit beside the chart: it
    /// then opens over it, and only when asked.
    narrow: bool,
    chat_overlay: bool,

    /// How wide the docked chat was last frame, for the bar above it.
    docked_chat: Option<f32>,
}

/// What the screen asks of the app around it.
pub enum Action {
    /// Go back to the screen shown before this one.
    Back,
}

/// Below this width the chat stops sitting beside the chart.
const CHAT_DOCK_WIDTH: f32 = 1000.0;

/// The chart needs at least this much beside the details to show them.
const DETAILS_WIDTH: f32 = 304.0;
const CHART_MIN_WIDTH: f32 = 480.0;

#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
struct TradingLayout {
    navigation: bool,
    chat: bool,
    width: f32,
    details: bool,
}
impl Default for TradingLayout {
    fn default() -> Self {
        Self {
            navigation: false,
            chat: true,
            width: 380.,
            details: true,
        }
    }
}

impl Default for TradingView {
    fn default() -> Self {
        Self {
            market: None,
            subscription: None,
            snapshot: Snapshot::default(),
            tab: Tab::Chart,
            picking: false,
            query: String::new(),
            connecting: false,
            client_id: String::new(),
            token: String::new(),
            picture: None,
            top_price: String::new(),
            bottom_price: String::new(),
            problem: None,
            chart: chart::ChartState::default(),
            oracle_for: None,
            chat: None,
            chat_view: Default::default(),
            layout: std::fs::read(folder().join("layout.json"))
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok())
                .unwrap_or_default(),
            narrow: false,
            chat_overlay: false,
            docked_chat: None,
        }
    }
}

fn folder() -> PathBuf {
    cortex_market::learning::desktop_folder()
}

impl TradingView {
    pub fn navigation_open(&self) -> bool {
        self.layout.navigation
    }
    /// Whether the chat is showing: beside the chart when there is room,
    /// over it when asked for in a narrow window.
    pub fn chat_open(&self) -> bool {
        if self.narrow {
            self.chat_overlay
        } else {
            self.layout.chat
        }
    }

    /// The width of the chat when it runs down the right-hand side, so
    /// that the bar above can take its colour there.
    pub fn docked_chat_width(&self) -> Option<f32> {
        self.docked_chat
    }
    pub fn set_navigation(&mut self, open: bool) {
        if self.layout.navigation != open {
            self.layout.navigation = open;
            self.save_layout();
        }
    }
    pub fn set_chat(&mut self, open: bool) {
        if self.narrow {
            self.chat_overlay = open;
        } else if self.layout.chat != open {
            self.layout.chat = open;
            self.save_layout();
        }
    }
    pub fn toggle_chat(&mut self) {
        self.set_chat(!self.chat_open());
    }
    fn save_layout(&mut self) {
        if let Err(error) =
            cortex_market::learning::checkpoint_write(&folder().join("layout.json"), &self.layout)
        {
            self.problem = Some(error);
        }
    }
    fn initialize_chat(
        &mut self,
        context: &egui::Context,
        market: &Market,
        coding_scope: &std::path::Path,
    ) {
        if self.chat.is_some() {
            return;
        }
        match cortex_market::learning::Learning::shared(&folder()) {
            Ok(learning) => {
                market.enable_learning(learning.clone());
                let context = context.clone();
                let chat = cortex_market::chat::TradingChat::new(
                    market.clone(),
                    learning,
                    coding_scope.to_string_lossy().into_owned(),
                    move || context.request_repaint(),
                );
                #[cfg(unix)]
                chat.listen(shared_brain());
                self.chat = Some(chat);
            }
            Err(error) => self.problem = Some(format!("Trading memory unavailable: {error}")),
        }
    }
    /// Shows the tab of this name, for testing.
    pub fn open_tab(&mut self, name: &str) {
        if let Some(tab) = Tab::ALL
            .into_iter()
            .find(|tab| tab.name().eq_ignore_ascii_case(name))
        {
            self.tab = tab;
        }
    }

    fn market(&mut self, context: &egui::Context) -> Market {
        if let Some(market) = &self.market {
            return market.clone();
        }

        let context = context.clone();
        let subscription =
            cortex_market::desktop::subscribe(&folder(), None, Interval::M5, move || {
                context.request_repaint()
            });
        let market = subscription.market();
        self.subscription = Some(subscription);

        self.client_id = market.client_id().unwrap_or_default();
        self.market = Some(market.clone());

        market
    }

    /// Sets the language model up from the chat's model settings, in the
    /// background: finding the key can wait on the keychain.
    fn follow_settings(&mut self, market: &Market, settings: &Settings) {
        let current = settings.current();
        let identity = format!(
            "{:?}|{}|{}",
            settings.provider, current.model, current.base_url
        );

        if self.oracle_for.as_deref() == Some(identity.as_str()) {
            return;
        }

        self.oracle_for = Some(identity);

        let identity = self.oracle_for.clone().unwrap();
        let chat = self.chat.clone();
        if let Some(chat) = &chat {
            chat.configure(&identity);
        }

        let market = market.clone();
        let settings = settings.clone();

        std::thread::spawn(move || {
            let key = crate::settings::secrets::resolve(settings.provider);
            let built = crate::engine::blueprint::build_provider(&settings, key.as_deref());

            if let Some(chat) = chat {
                if !chat.provider(
                    &identity,
                    built.as_ref().map(Arc::clone).map_err(|e| e.to_string()),
                ) {
                    return;
                }
            }
            market.set_oracle(built.ok().map(|provider| {
                Arc::new(ProviderOracle::new(provider)) as Arc<dyn cortex_market::Oracle>
            }));
        });
    }

    fn select_market(
        &mut self,
        context: &egui::Context,
        instrument: Instrument,
        interval: Interval,
    ) {
        let context = context.clone();
        let subscription =
            cortex_market::desktop::subscribe(&folder(), Some(instrument), interval, move || {
                context.request_repaint()
            });
        let market = subscription.market();
        if let Some(chat) = &self.chat {
            market.enable_learning(chat.learning.clone());
            chat.set_market(market.clone());
        }
        self.market = Some(market);
        self.subscription = Some(subscription);
        self.oracle_for = None;
        self.snapshot = Snapshot::default();
        self.chart = Default::default();
    }

    fn open_file(&mut self, market: &Market, path: &std::path::Path) {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default();

        let extension = path
            .extension()
            .map(|extension| extension.to_string_lossy().to_lowercase())
            .unwrap_or_default();

        let opened = match std::fs::read(path) {
            Err(error) => Err(format!("Cannot read {name}: {error}")),
            Ok(bytes) if matches!(extension.as_str(), "png" | "jpg" | "jpeg" | "webp") => {
                self.picture = Some((name.clone(), bytes.clone()));
                self.top_price.clear();
                self.bottom_price.clear();

                market.load_picture(&name, bytes, None).map(|_| ())
            }
            Ok(bytes) => market
                .load_csv(&name, &String::from_utf8_lossy(&bytes))
                .map(|_| ()),
        };

        self.problem = opened.err();
        self.chart = chart::ChartState::default();
    }
}

fn choose_file() -> Option<PathBuf> {
    rfd::FileDialog::new()
        .add_filter(
            "Price bars or chart picture",
            &["csv", "tsv", "txt", "png", "jpg", "jpeg", "webp"],
        )
        .pick_file()
}


pub fn show(
    ui: &mut Ui,
    view: &mut TradingView,
    settings: &Settings,
    coding_scope: &std::path::Path,
) -> Option<Action> {
    let market = view.market(ui.ctx());

    view.initialize_chat(ui.ctx(), &market, coding_scope);

    view.follow_settings(&market, settings);

    if market.revision() != view.snapshot.revision || view.snapshot.revision == 0 {
        view.snapshot = market.snapshot();
    }

    // A file dropped on the window is opened.
    let dropped = ui.input(|input| {
        input
            .raw
            .dropped_files
            .iter()
            .map(|file| file.path().to_path_buf())
            .next()
    });

    if let Some(path) = dropped {
        view.open_file(&market, &path);
    }

    let snapshot = view.snapshot.clone();
    let area = ui.max_rect();

    view.narrow = area.width() < CHAT_DOCK_WIDTH;

    // The chat first, so that it runs the whole height of the screen,
    // beside the header as well as the chart.
    chat(ui, view, &snapshot, area);

    let mut action = None;

    // Whether the details fit beside the chart, the chat taken out.
    let room = ui.available_width() >= DETAILS_WIDTH + CHART_MIN_WIDTH;

    egui::Panel::top(ui.id().with("trading header"))
        .show_separator_line(false)
        .frame(
            egui::Frame::new()
                .fill(palette::background())
                .inner_margin(egui::Margin {
                    left: 16,
                    right: 16,
                    top: 8,
                    bottom: 16,
                }),
        )
        .show(ui, |ui| {
            if header(ui, view, &market, &snapshot, room) {
                action = Some(Action::Back);
            }
        });

    connect_window(ui, view, &market, &snapshot);
    picker_window(ui, view, &market);

    if snapshot.source.is_none() {
        empty(ui, view, &market, &snapshot);

        return action;
    }

    match view.tab {
        Tab::Chart => {
            if view.layout.details && room {
                egui::Panel::right(ui.id().with("trading side"))
                    .exact_size(DETAILS_WIDTH)
                    .show_separator_line(false)
                    .frame(egui::Frame::new().fill(palette::background()).inner_margin(
                        egui::Margin {
                            left: 0,
                            right: 16,
                            top: 0,
                            bottom: 16,
                        },
                    ))
                    .show(ui, |ui| {
                        egui::ScrollArea::vertical()
                            .auto_shrink(false)
                            .show(ui, |ui| {
                                panel::show(ui, &market, &snapshot, &mut view.picture_scale())
                            });
                    });
            }

            egui::CentralPanel::default()
                .frame(egui::Frame::new().fill(palette::background()).inner_margin(
                    egui::Margin {
                        left: 16,
                        right: 16,
                        top: 0,
                        bottom: 16,
                    },
                ))
                .show(ui, |ui| {
                    egui::Frame::new()
                        .fill(palette::background())
                        .stroke(Stroke::new(1.0, palette::border()))
                        .corner_radius(CornerRadius::same(12))
                        .inner_margin(egui::Margin::same(4))
                        .show(ui, |ui| {
                            ui.set_min_size(ui.available_size());
                            chart::show(ui, &mut view.chart, &snapshot)
                        });
                });
        }

        tab => {
            egui::CentralPanel::default()
                .frame(egui::Frame::new().fill(palette::background()).inner_margin(
                    egui::Margin {
                        left: 16,
                        right: 16,
                        top: 0,
                        bottom: 0,
                    },
                ))
                .show(ui, |ui| {
                    egui::ScrollArea::both()
                        .auto_shrink(false)
                        .show(ui, |ui| {
                            egui::Frame::new()
                                .inner_margin(egui::Margin {
                                    left: 2,
                                    right: 8,
                                    top: 0,
                                    bottom: 8,
                                })
                                .show(ui, |ui| match tab {
                                    Tab::Forecast => tables::forecast(ui, &snapshot),
                                    Tab::Outcomes => tables::outcomes(ui, &market, &snapshot),
                                    Tab::Metrics => tables::metrics(ui, &market, &snapshot),
                                    Tab::Flow => flow_panel::show(ui, &market, &snapshot),
                                    Tab::Chart => {}
                                });
                        });
                });
        }
    }

    action
}

/// The trading chat: down the right-hand side when there is room, else
/// over the screen's right-hand side while it is asked for.
fn chat(ui: &mut Ui, view: &mut TradingView, snapshot: &Snapshot, area: egui::Rect) {
    view.docked_chat = None;

    let Some(chat) = view.chat.clone() else {
        return;
    };

    let frame = egui::Frame::new()
        .fill(palette::sidebar())
        .inner_margin(egui::Margin {
            left: 16,
            right: 16,
            top: 8,
            bottom: 16,
        });

    let mut close = false;

    if !view.narrow && view.layout.chat {
        let most = (area.width() * 0.45).clamp(320.0, 560.0);
        let width = view.layout.width.clamp(320.0, most);

        let panel = egui::Panel::right(ui.id().with("trading chat"))
            .default_size(width)
            .size_range(320.0..=most)
            .resizable(true)
            .frame(frame)
            .show(ui, |ui| {
                chat_panel::show(ui, &chat, &mut view.chat_view, snapshot)
            });

        close = panel.inner.is_some();

        let width = panel.response.rect.width();

        view.docked_chat = Some(width);

        if (width - view.layout.width).abs() > 1.0 && !ui.input(|input| input.pointer.any_down()) {
            view.layout.width = width;
            view.save_layout();
        }
    } else if view.narrow && view.chat_overlay {
        let width = (area.width() - 48.0).clamp(280.0, 400.0);
        let rect = egui::Rect::from_min_max(pos2(area.right() - width, area.top()), area.max);

        let shown = egui::Window::new("Trading chat")
            .id(ui.id().with("trading chat overlay"))
            .title_bar(false)
            .fixed_rect(rect)
            .frame(frame.shadow(egui::Shadow {
                offset: [-8, 0],
                blur: 24,
                spread: 0,
                color: egui::Color32::from_black_alpha(90),
            }))
            .show(ui.ctx(), |ui| {
                ui.set_min_size(ui.available_size());
                chat_panel::show(ui, &chat, &mut view.chat_view, snapshot)
            });

        close = shown.and_then(|shown| shown.inner).flatten().is_some();
    }

    if close {
        view.set_chat(false);
    }
}

/// What the side panel needs to set a picture's price scale.
pub struct PictureScale<'a> {
    pub picture: &'a Option<(String, Vec<u8>)>,
    pub top: &'a mut String,
    pub bottom: &'a mut String,
}

impl TradingView {
    fn picture_scale(&mut self) -> PictureScale<'_> {
        PictureScale {
            picture: &self.picture,
            top: &mut self.top_price,
            bottom: &mut self.bottom_price,
        }
    }

    fn choose_instrument(&mut self, market: &Market) {
        if market.is_connected() {
            self.picking = true;
            self.query.clear();
        } else {
            self.connecting = true;
        }
    }

    fn import(&mut self, market: &Market) {
        if let Some(path) = choose_file() {
            self.open_file(market, &path);
        }
    }
}

/// The tabs shown as segments; the rest are under "More".
const MAIN_TABS: [Tab; 3] = [Tab::Chart, Tab::Forecast, Tab::Outcomes];
const MORE_TABS: [Tab; 2] = [Tab::Metrics, Tab::Flow];

/// The header: back, the instrument and its price, its state, and the
/// screen's switches; then the tabs, the candle size and the network.
/// Returns whether back was pressed.
fn header(
    ui: &mut Ui,
    view: &mut TradingView,
    market: &Market,
    snapshot: &Snapshot,
    room: bool,
) -> bool {
    let mut back = false;
    let mut failure = None;

    ui.spacing_mut().item_spacing = vec2(8.0, 8.0);

    ui.horizontal(|ui| {
        ui.set_min_height(widgets::HEIGHT + 8.0);

        if widgets::icon_button(ui, widgets::Glyph::App(Icon::Back), "Back", false).clicked() {
            back = true;
        }

        ui.add_space(4.0);

        let title = snapshot
            .source
            .as_ref()
            .map(Source::title)
            .unwrap_or_else(|| "Trading".to_string());

        if instrument_button(ui, &title)
            .on_hover_text("Choose an instrument")
            .clicked()
        {
            view.choose_instrument(market);
        }

        if snapshot.now.price > 0.0 {
            ui.add_space(4.0);

            ui.label(
                RichText::new(format::price(snapshot.now.price))
                    .font(theme::strong(18.0))
                    .color(palette::text()),
            );

            if let (Some(change), Some(percent)) = (snapshot.now.change, snapshot.now.change_pct) {
                let colour = if change >= 0.0 {
                    palette::good()
                } else {
                    palette::bad()
                };

                ui.label(
                    RichText::new(format!("{:+.2}  {}", change, format::percent(percent)))
                        .font(theme::strong(13.0))
                        .color(colour),
                );
            }
        }

        ui.add_space(8.0);
        failure = status_pill(ui, snapshot);

        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let chat_tip = if view.chat_open() {
                "Hide the chat  ⌘E"
            } else {
                "Show the chat  ⌘E"
            };

            if widgets::icon_button(
                ui,
                widgets::Glyph::App(Icon::Chat),
                chat_tip,
                view.chat_open(),
            )
            .clicked()
            {
                view.toggle_chat();
            }

            if snapshot.source.is_some() && view.tab == Tab::Chart && room {
                let tip = if view.layout.details {
                    "Hide the details"
                } else {
                    "Show the details"
                };

                if widgets::icon_button(ui, widgets::Glyph::Panel, tip, view.layout.details)
                    .clicked()
                {
                    view.layout.details = !view.layout.details;
                    view.save_layout();
                }
            }

            widgets::overflow(ui, "More", false, |ui| {
                if widgets::menu_item(
                    ui,
                    "Import bars or a picture…",
                    Some(widgets::Glyph::Import),
                    Some("CSV, PNG or JPG"),
                    false,
                )
                .clicked()
                {
                    view.import(market);
                }

                let (dhan, detail) = match &snapshot.connection {
                    Connection::Connected(profile) => {
                        ("Dhan account…", format!("Connected as {}", profile.client_id))
                    }
                    Connection::Checking => ("Dhan account…", "Checking…".to_string()),
                    _ => ("Connect Dhan…", "Live data in real time".to_string()),
                };

                if widgets::menu_item(
                    ui,
                    dhan,
                    Some(widgets::Glyph::Link),
                    Some(&detail),
                    false,
                )
                .clicked()
                {
                    view.connecting = true;
                }
            });
        });
    });

    if snapshot.source.is_some() {
        ui.add_space(8.0);

        ui.horizontal(|ui| {
            tabs(ui, view);

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                brains(ui, market, snapshot);
                intervals(ui, view, snapshot);
            });
        });
    }

    if let Some(problem) = view.problem.as_ref().or(snapshot.problem.as_ref()) {
        ui.add_space(8.0);
        widgets::banner(ui, problem, palette::bad());
    }

    // The pill only names the trouble; the whole reason is here, with the
    // way to fix it when it is the account.
    if let Some(reason) = failure {
        ui.add_space(8.0);

        egui::Frame::new()
            .fill(palette::bad().gamma_multiply(0.10))
            .stroke(egui::Stroke::new(1.0, palette::bad().gamma_multiply(0.35)))
            .corner_radius(egui::CornerRadius::same(widgets::RADIUS))
            .inner_margin(egui::Margin::symmetric(12, 6))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());

                ui.horizontal(|ui| {
                    let account = reason.contains("Dhan") || reason.contains("token");

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if account
                            && widgets::button(ui, "Dhan account…", None, widgets::Kind::Secondary)
                                .clicked()
                        {
                            view.connecting = true;
                        }

                        ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                            ui.add(
                                egui::Label::new(
                                    RichText::new(&reason)
                                        .font(theme::small())
                                        .color(palette::bad()),
                                )
                                .wrap(),
                            );
                        });
                    });
                });
            });
    }

    back
}

/// The instrument's name, large, opening the picker.
fn instrument_button(ui: &mut Ui, title: &str) -> egui::Response {
    let galley = ui
        .painter()
        .layout_no_wrap(title.to_string(), theme::strong(18.0), palette::text());
    let (rect, response) = ui.allocate_exact_size(
        vec2(galley.size().x + 40.0, widgets::HEIGHT + 4.0),
        Sense::click(),
    );

    if response.hovered() {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(widgets::RADIUS), palette::raised());
    }

    ui.painter().galley(
        pos2(rect.left() + 10.0, rect.center().y - galley.size().y / 2.0),
        galley,
        palette::text(),
    );

    widgets::paint(
        ui,
        pos2(rect.right() - 16.0, rect.center().y),
        widgets::Glyph::Chevron,
        palette::text_muted(),
    );

    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// The tabs: the three most used as segments, the rest in a menu.
fn tabs(ui: &mut Ui, view: &mut TradingView) {
    egui::Frame::new()
        .fill(palette::sunken())
        .stroke(Stroke::new(1.0, palette::border()))
        .corner_radius(CornerRadius::same(widgets::RADIUS + 2))
        .inner_margin(egui::Margin::same(2))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;

            ui.horizontal(|ui| {
                for tab in MAIN_TABS {
                    if widgets::segment(ui, tab.name(), view.tab == tab, None).clicked() {
                        view.tab = tab;
                    }
                }

                let more = MORE_TABS.contains(&view.tab);
                let label = if more { view.tab.name() } else { "More" };
                let response =
                    widgets::segment(ui, label, more, Some(widgets::Glyph::Chevron));

                widgets::menu(&response, false, |ui| {
                    for tab in MORE_TABS {
                        if widgets::menu_item(ui, tab.name(), None, None, view.tab == tab)
                            .clicked()
                        {
                            view.tab = tab;
                        }
                    }
                });
            });
        });
}

/// The candle size, for live series; a loaded series keeps its own.
fn intervals(ui: &mut Ui, view: &mut TradingView, snapshot: &Snapshot) {
    let live = snapshot.source.as_ref().is_some_and(Source::is_live);

    if !live {
        widgets::button_enabled(
            ui,
            false,
            snapshot.interval.label(),
            None,
            widgets::Kind::Secondary,
        )
        .on_disabled_hover_text("The loaded series sets the candle size");

        return;
    }

    let mut chosen = None;

    widgets::dropdown(
        ui,
        snapshot.interval.label(),
        None,
        widgets::Kind::Secondary,
        false,
        false,
        |ui| {
            ui.set_min_width(140.0);

            for interval in Interval::ALL {
                if widgets::menu_item(ui, interval.label(), None, None, snapshot.interval == interval)
                    .clicked()
                {
                    chosen = Some(interval);
                }
            }
        },
    )
    .0
    .on_hover_text("Candle size");

    if let (Some(interval), Some(Source::Dhan { instrument })) = (chosen, &snapshot.source) {
        if interval != snapshot.interval {
            view.select_market(ui.ctx(), instrument.clone(), interval);
        }
    }
}

/// Which network forecasts, as a menu: each with what it is.
fn brains(ui: &mut Ui, market: &Market, snapshot: &Snapshot) {
    let current = snapshot.brain;
    let label = format!("{} · {}", current.short(), current.name());

    widgets::dropdown(
        ui,
        &label,
        None,
        widgets::Kind::Secondary,
        false,
        false,
        |ui| {
            ui.set_width(300.0);

            for brain in Brain::ALL {
                let detail = match brain {
                    Brain::Llm => format!(
                        "{} · {}",
                        snapshot.llm.model.as_deref().unwrap_or("No model set up"),
                        brain.about()
                    ),
                    _ => brain.about().to_string(),
                };

                if widgets::menu_item(
                    ui,
                    &format!("{} · {}", brain.short(), brain.name()),
                    None,
                    Some(&detail),
                    brain == current,
                )
                .clicked()
                {
                    market.set_brain(brain);
                }
            }
        },
    )
    .0
    .on_hover_text("The network that forecasts");
}

/// The state in a word or two; a failure's whole reason is given back
/// for the banner under the header.
fn status_pill(ui: &mut Ui, snapshot: &Snapshot) -> Option<String> {
    let (text, colour, failure) = match (&snapshot.work, &snapshot.feed) {
        (Work::Loading(what), _) => (format!("Loading {what}"), palette::text_muted(), None),
        (Work::Training(fraction), _) => (
            format!("Training {:.0}%", fraction * 100.0),
            palette::accent(),
            None,
        ),
        (Work::Failed(reason), _) => ("Failed".to_string(), palette::bad(), Some(reason.clone())),
        (_, FeedState::Live) => ("Live".to_string(), palette::good(), None),
        (_, FeedState::Connecting) => ("Connecting".to_string(), palette::text_muted(), None),
        (_, FeedState::Retrying(_)) => ("Reconnecting".to_string(), palette::warning(), None),
        (_, FeedState::Stopped(reason)) => {
            ("Feed stopped".to_string(), palette::bad(), Some(reason.clone()))
        }
        _ => match &snapshot.source {
            Some(Source::Dhan { .. }) => ("Market closed".to_string(), palette::text_muted(), None),
            Some(_) => ("Offline".to_string(), palette::text_muted(), None),
            None => return None,
        },
    };

    widgets::status(ui, &text, colour);

    failure
}

/// Nothing chosen yet: the two ways in.
fn empty(ui: &mut Ui, view: &mut TradingView, market: &Market, snapshot: &Snapshot) {
    egui::CentralPanel::default()
        .frame(egui::Frame::new().fill(palette::background()))
        .show(ui, |ui| {
            ui.add_space((ui.available_height() * 0.22).max(24.0));

            ui.vertical_centered(|ui| {
                ui.label(
                    RichText::new("Forecast the next ten candles")
                        .font(theme::voice(28.0))
                        .color(palette::text()),
                );

                ui.add_space(32.0);

                let connected = market.is_connected();
                let gap = 16.0;
                let card = 248.0;
                let side_by_side = ui.available_width() >= card * 2.0 + gap + 32.0;

                let live = |ui: &mut Ui| {
                    big_button(
                        ui,
                        widgets::Glyph::Link,
                        if connected {
                            "Choose an instrument"
                        } else {
                            "Connect Dhan"
                        },
                        "Live data in real time",
                    )
                };
                let file = |ui: &mut Ui| {
                    big_button(
                        ui,
                        widgets::Glyph::Import,
                        "Import",
                        "A CSV, or a chart picture",
                    )
                };

                let (live, file) = if side_by_side {
                    ui.horizontal(|ui| {
                        ui.add_space((ui.available_width() - card * 2.0 - gap).max(0.0) / 2.0);
                        ui.spacing_mut().item_spacing.x = gap;
                        (live(ui), file(ui))
                    })
                    .inner
                } else {
                    let live = live(ui);
                    ui.add_space(gap);
                    (live, file(ui))
                };

                if live.clicked() {
                    view.choose_instrument(market);
                }

                if file.clicked() {
                    view.import(market);
                }

                if let Connection::Failed(reason) = &snapshot.connection {
                    ui.add_space(16.0);
                    ui.allocate_ui(vec2(card * 2.0 + gap, 0.0), |ui| {
                        widgets::banner(ui, reason, palette::bad());
                    });
                }
            });
        });
}

fn big_button(ui: &mut Ui, glyph: widgets::Glyph, title: &str, detail: &str) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(248.0, 80.0), Sense::click());

    let (fill, stroke) = if response.hovered() {
        (palette::raised(), palette::accent().gamma_multiply(0.6))
    } else {
        (palette::surface(), palette::border())
    };

    ui.painter().rect(
        rect,
        CornerRadius::same(12),
        fill,
        Stroke::new(1.0, stroke),
        egui::StrokeKind::Inside,
    );

    let badge = egui::Rect::from_min_size(rect.left_top() + vec2(16.0, 20.0), vec2(40.0, 40.0));

    ui.painter().rect_filled(
        badge,
        CornerRadius::same(10),
        palette::accent().gamma_multiply(0.15),
    );
    widgets::paint(ui, badge.center(), glyph, palette::accent());

    ui.painter().text(
        rect.left_top() + vec2(72.0, 22.0),
        egui::Align2::LEFT_TOP,
        title,
        theme::strong(15.0),
        palette::text(),
    );

    ui.painter().text(
        rect.left_top() + vec2(72.0, 44.0),
        egui::Align2::LEFT_TOP,
        detail,
        theme::small(),
        palette::text_muted(),
    );

    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// The Dhan form: client ID and access token.
fn connect_window(ui: &mut Ui, view: &mut TradingView, market: &Market, snapshot: &Snapshot) {
    if !view.connecting {
        return;
    }

    let modal = egui::Modal::new(ui.id().with("dhan"))
        .frame(
            egui::Frame::new()
                .fill(palette::surface())
                .stroke(Stroke::new(1.0, palette::border()))
                .corner_radius(CornerRadius::same(14))
                .inner_margin(egui::Margin::same(24)),
        )
        .show(ui.ctx(), |ui| {
            ui.set_width(384.0);
            ui.spacing_mut().item_spacing.y = 8.0;

            ui.label(RichText::new("Dhan").font(theme::strong(18.0)));
            ui.label(
                RichText::new("web.dhan.co → My Profile → Access DhanHQ APIs. Tokens last a day; data needs Dhan's data plan.")
                    .font(theme::small())
                    .color(palette::text_muted()),
            );

            ui.add_space(8.0);

            ui.add(
                egui::TextEdit::singleline(&mut view.client_id)
                    .hint_text("Client ID")
                    .margin(egui::Margin::symmetric(12, 8))
                    .desired_width(f32::INFINITY),
            );
            ui.add(
                egui::TextEdit::singleline(&mut view.token)
                    .hint_text("Access token")
                    .password(true)
                    .margin(egui::Margin::symmetric(12, 8))
                    .desired_width(f32::INFINITY),
            );

            match &snapshot.connection {
                Connection::Checking => {
                    ui.label(RichText::new("Checking…").color(palette::text_muted()));
                }
                Connection::Connected(profile) => {
                    let plan = if profile.data_plan.is_empty() {
                        String::new()
                    } else {
                        format!(" · data plan {}", profile.data_plan)
                    };

                    ui.label(
                        RichText::new(format!("Connected as {}{plan}", profile.client_id))
                            .color(palette::good()),
                    );
                }
                Connection::Failed(reason) => {
                    widgets::banner(ui, reason, palette::bad());
                }
                Connection::Off => {}
            }

            ui.add_space(8.0);

            ui.horizontal(|ui| {
                let ready = !view.client_id.trim().is_empty() && !view.token.trim().is_empty();

                if widgets::button_enabled(ui, ready, "Connect", None, widgets::Kind::Primary)
                    .clicked()
                {
                    market.connect(&view.client_id, &view.token);
                    view.token.clear();
                }

                if matches!(snapshot.connection, Connection::Connected(_))
                    && widgets::button(ui, "Disconnect", None, widgets::Kind::Ghost).clicked()
                {
                    market.disconnect();
                }

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if widgets::button(ui, "Done", None, widgets::Kind::Secondary).clicked() {
                        view.connecting = false;
                    }
                });
            });
        });

    if modal.should_close() {
        view.connecting = false;
    }
}

/// Searching for an instrument.
fn picker_window(ui: &mut Ui, view: &mut TradingView, market: &Market) {
    if !view.picking {
        return;
    }

    let mut chosen: Option<Instrument> = None;

    let modal = egui::Modal::new(ui.id().with("instrument"))
        .frame(
            egui::Frame::new()
                .fill(palette::surface())
                .stroke(Stroke::new(1.0, palette::border()))
                .corner_radius(CornerRadius::same(14))
                .inner_margin(egui::Margin::same(16)),
        )
        .show(ui.ctx(), |ui| {
            ui.set_width(440.0);

            let field = ui.add(
                egui::TextEdit::singleline(&mut view.query)
                    .hint_text("Search: RELIANCE, NIFTY, TCS…")
                    .margin(egui::Margin::symmetric(12, 8))
                    .desired_width(f32::INFINITY),
            );

            field.request_focus();

            ui.add_space(8.0);

            let found = market.search(&view.query, 40);

            egui::ScrollArea::vertical()
                .max_height(360.0)
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;

                    for instrument in &found {
                        let (rect, response) = ui.allocate_exact_size(
                            vec2(ui.available_width(), 40.0),
                            Sense::click(),
                        );

                        if response.hovered() {
                            ui.painter().rect_filled(
                                rect,
                                CornerRadius::same(8),
                                palette::raised(),
                            );
                        }

                        let symbol = ui.painter().text(
                            pos2(rect.left() + 12.0, rect.center().y),
                            egui::Align2::LEFT_CENTER,
                            &instrument.symbol,
                            theme::strong(13.5),
                            palette::text(),
                        );

                        ui.painter().text(
                            pos2(symbol.right() + 12.0, rect.center().y),
                            egui::Align2::LEFT_CENTER,
                            &instrument.name,
                            theme::small(),
                            palette::text_muted(),
                        );

                        ui.painter().text(
                            pos2(rect.right() - 12.0, rect.center().y),
                            egui::Align2::RIGHT_CENTER,
                            instrument.segment.exchange(),
                            theme::small(),
                            palette::text_muted(),
                        );

                        if response
                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                            .clicked()
                        {
                            chosen = Some(instrument.clone());
                        }
                    }
                });

            if !market.has_instrument_list() {
                ui.add_space(8.0);

                if widgets::button(
                    ui,
                    "Load Dhan's full list",
                    Some(widgets::Glyph::Import),
                    widgets::Kind::Secondary,
                )
                .on_hover_text("Every share, index and future on Dhan (tens of megabytes, once)")
                .clicked()
                {
                    market.fetch_instrument_list();
                }
            }

            if ui.input(|input| input.key_pressed(egui::Key::Enter))
                && let Some(first) = found.first()
            {
                chosen = Some(first.clone());
            }
        });

    if let Some(instrument) = chosen {
        view.select_market(ui.ctx(), instrument, view.snapshot.interval);
        view.picking = false;
        view.chart = chart::ChartState::default();
        view.tab = Tab::Chart;
    } else if modal.should_close() {
        view.picking = false;
    }
}

/// Hands each line of the trading conversation to the one brain that the
/// chat and code sessions feed too. It connects on first use, and again
/// later if the background service could not be reached.
#[cfg(unix)]
fn shared_brain() -> cortex_market::chat::Listener {
    use std::sync::{Arc, Mutex};

    let brain: Arc<Mutex<Option<Arc<cortex_living::Living>>>> = Arc::default();

    Arc::new(move |session: &str, speaker: &str, text: &str| {
        let mut held = brain.lock().unwrap_or_else(|poisoned| poisoned.into_inner());

        if held.is_none() {
            match cortex_living::service::default_file()
                .and_then(|file| cortex_living::Living::connect(&file))
            {
                Ok(living) => *held = Some(living),
                Err(error) => {
                    eprintln!("warning: the brain is unavailable to trading: {error}");
                    return;
                }
            }
        }

        if let Some(living) = held.as_ref() {
            living.hear(session, speaker, text);
        }
    })
}
