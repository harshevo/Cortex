//! The trading screen: an instrument live from Dhan, or bars from a file
//! or a chart picture, with the next ten candles as the chosen network
//! forecasts them, a plan for each case, how earlier forecasts came out,
//! and how every network measures up.
//!
//! The work is `cortex-market`'s; this draws its snapshot.

mod chart;
mod format;
mod panel;
mod tables;

use std::path::PathBuf;
use std::sync::Arc;

use cortex_market::{
    Brain, Connection, FeedState, Instrument, Interval, Market, ProviderOracle, Snapshot, Source,
    Work,
};
use eframe::egui::{self, Align, CornerRadius, Layout, RichText, Sense, Stroke, Ui, vec2};

use crate::settings::Settings;
use crate::theme::{self, palette, space};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Chart,
    Forecast,
    Outcomes,
    Metrics,
}

impl Tab {
    const ALL: [Tab; 4] = [Tab::Chart, Tab::Forecast, Tab::Outcomes, Tab::Metrics];

    fn name(self) -> &'static str {
        match self {
            Tab::Chart => "Chart",
            Tab::Forecast => "Forecast",
            Tab::Outcomes => "Outcomes",
            Tab::Metrics => "Metrics",
        }
    }
}

pub struct TradingView {
    market: Option<Market>,
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
}

impl Default for TradingView {
    fn default() -> Self {
        Self {
            market: None,
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
        }
    }
}

fn folder() -> PathBuf {
    std::env::home_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join(".cortex")
        .join("trading")
}

impl TradingView {
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
        let market = Market::new(&folder(), move || context.request_repaint());

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

        let market = market.clone();
        let settings = settings.clone();

        std::thread::spawn(move || {
            let key = crate::settings::secrets::resolve(settings.provider);
            let built = crate::engine::blueprint::build_provider(&settings, key.as_deref());

            market.set_oracle(built.ok().map(|provider| {
                Arc::new(ProviderOracle::new(provider)) as Arc<dyn cortex_market::Oracle>
            }));
        });
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

pub fn show(ui: &mut Ui, view: &mut TradingView, settings: &Settings) {
    let market = view.market(ui.ctx());

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

    egui::Panel::top(ui.id().with("trading header"))
        .show_separator_line(false)
        .frame(
            egui::Frame::new()
                .fill(palette::background())
                .inner_margin(egui::Margin {
                    left: 16,
                    right: 16,
                    top: 10,
                    bottom: 6,
                }),
        )
        .show(ui, |ui| header(ui, view, &market, &snapshot));

    connect_window(ui, view, &market, &snapshot);
    picker_window(ui, view, &market);

    if snapshot.source.is_none() {
        empty(ui, view, &market, &snapshot);

        return;
    }

    match view.tab {
        Tab::Chart => {
            egui::Panel::right(ui.id().with("trading side"))
                .exact_size(300.0)
                .show_separator_line(false)
                .frame(
                    egui::Frame::new()
                        .fill(palette::background())
                        .inner_margin(egui::Margin {
                            left: 4,
                            right: 16,
                            top: 4,
                            bottom: 12,
                        }),
                )
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .auto_shrink(false)
                        .show(ui, |ui| {
                            panel::show(ui, &market, &snapshot, &mut view.picture_scale())
                        });
                });

            egui::CentralPanel::default()
                .frame(egui::Frame::new().fill(palette::background()))
                .show(ui, |ui| chart::show(ui, &mut view.chart, &snapshot));
        }

        tab => {
            egui::CentralPanel::default()
                .frame(
                    egui::Frame::new()
                        .fill(palette::background())
                        .inner_margin(egui::Margin::symmetric(16, 8)),
                )
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .auto_shrink(false)
                        .show(ui, |ui| match tab {
                            Tab::Forecast => tables::forecast(ui, &snapshot),
                            Tab::Outcomes => tables::outcomes(ui, &market, &snapshot),
                            Tab::Metrics => tables::metrics(ui, &market, &snapshot),
                            Tab::Chart => {}
                        });
                });
        }
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
}

fn header(ui: &mut Ui, view: &mut TradingView, market: &Market, snapshot: &Snapshot) {
    ui.horizontal(|ui| {
        // The instrument, and its price.
        let title = snapshot
            .source
            .as_ref()
            .map(Source::title)
            .unwrap_or_else(|| "Trading".to_string());

        let pick = ui
            .add(
                egui::Button::new(
                    RichText::new(format!("{title}  ▾"))
                        .font(theme::strong(17.0))
                        .color(palette::text()),
                )
                .frame(false),
            )
            .on_hover_text("Choose an instrument");

        if pick.clicked() {
            if market.is_connected() {
                view.picking = true;
                view.query.clear();
            } else {
                view.connecting = true;
            }
        }

        if snapshot.now.price > 0.0 {
            ui.label(
                RichText::new(format::price(snapshot.now.price))
                    .font(theme::strong(17.0))
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
                        .font(theme::ui())
                        .color(colour),
                );
            }
        }

        status_pill(ui, snapshot);

        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let dhan_label = match &snapshot.connection {
                Connection::Connected(_) => "Dhan ✓",
                Connection::Checking => "Dhan…",
                _ => "Connect Dhan",
            };

            if ui.button(dhan_label).clicked() {
                view.connecting = true;
            }

            if ui
                .button("Import")
                .on_hover_text("A CSV of bars, or a picture of a chart (PNG, JPG)")
                .clicked()
                && let Some(path) = choose_file()
            {
                view.open_file(market, &path);
            }
        });
    });

    ui.add_space(space::S);

    ui.horizontal(|ui| {
        // The tabs.
        for tab in Tab::ALL {
            if segment(ui, tab.name(), view.tab == tab, None).clicked() {
                view.tab = tab;
            }
        }

        ui.add_space(space::L);

        // The candle size, for live and loaded series alike.
        let live = snapshot.source.as_ref().is_some_and(Source::is_live);

        for interval in Interval::ALL {
            let chosen = snapshot.interval == interval;

            let response = ui.add_enabled(
                live || chosen,
                egui::Button::selectable(
                    chosen,
                    RichText::new(interval.label()).font(theme::small()),
                ),
            );

            if response.clicked() {
                market.set_interval(interval);
            }
        }

        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            brains(ui, market, snapshot);
        });
    });

    if let Some(problem) = view.problem.as_ref().or(snapshot.problem.as_ref()) {
        ui.label(
            RichText::new(problem)
                .font(theme::small())
                .color(palette::bad()),
        );
    }
}

/// Which network forecasts: one segment each, the one in use filled.
fn brains(ui: &mut Ui, market: &Market, snapshot: &Snapshot) {
    for brain in Brain::ALL.into_iter().rev() {
        let chosen = snapshot.brain == brain;

        let help = match brain {
            Brain::Llm => format!(
                "{} — {}",
                snapshot.llm.model.as_deref().unwrap_or("no model set up"),
                brain.about()
            ),
            _ => format!("{} network — {}", brain.name(), brain.about()),
        };

        if segment(ui, brain.short(), chosen, Some(brain.name()))
            .on_hover_text(help)
            .clicked()
        {
            market.set_brain(brain);
        }
    }

    ui.label(
        RichText::new("Network")
            .font(theme::small())
            .color(palette::text_muted()),
    );
}

/// A rounded tab: filled with the accent when chosen.
fn segment(ui: &mut Ui, label: &str, chosen: bool, detail: Option<&str>) -> egui::Response {
    let text = match detail {
        Some(detail) if chosen => format!("{label} · {detail}"),
        _ => label.to_string(),
    };

    let galley = ui
        .painter()
        .layout_no_wrap(text, theme::strong(12.5), palette::text());
    let size = vec2(galley.size().x + 20.0, 26.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());

    let (fill, ink) = if chosen {
        (palette::accent(), palette::background())
    } else if response.hovered() {
        (palette::raised(), palette::text())
    } else {
        (palette::surface(), palette::text_secondary())
    };

    ui.painter().rect(
        rect,
        CornerRadius::same(13),
        fill,
        if chosen {
            Stroke::NONE
        } else {
            Stroke::new(1.0, palette::border())
        },
        egui::StrokeKind::Inside,
    );

    ui.painter()
        .galley(rect.center() - galley.size() / 2.0, galley, ink);

    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn status_pill(ui: &mut Ui, snapshot: &Snapshot) {
    let (text, colour) = match (&snapshot.work, &snapshot.feed) {
        (Work::Loading(what), _) => (format!("Loading {what}"), palette::text_muted()),
        (Work::Training(fraction), _) => (
            format!("Training {:.0}%", fraction * 100.0),
            palette::accent(),
        ),
        (Work::Failed(reason), _) => (reason.clone(), palette::bad()),
        (_, FeedState::Live) => ("Live".to_string(), palette::good()),
        (_, FeedState::Connecting) => ("Connecting".to_string(), palette::text_muted()),
        (_, FeedState::Retrying(_)) => ("Reconnecting".to_string(), palette::warning()),
        (_, FeedState::Stopped(reason)) => (reason.clone(), palette::bad()),
        _ => match &snapshot.source {
            Some(Source::Dhan { .. }) => ("Market closed".to_string(), palette::text_muted()),
            Some(_) => ("Offline".to_string(), palette::text_muted()),
            None => return,
        },
    };

    let full = text.clone();
    let short: String = if text.chars().count() > 60 {
        text.chars().take(57).collect::<String>() + "…"
    } else {
        text
    };

    ui.add_space(space::S);

    let response = egui::Frame::new()
        .fill(colour.gamma_multiply(0.12))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(egui::Margin::symmetric(8, 3))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;

                let (dot, _) = ui.allocate_exact_size(vec2(7.0, 7.0), Sense::hover());

                ui.painter().circle_filled(dot.center(), 3.5, colour);
                ui.label(RichText::new(short).font(theme::small()).color(colour));
            });
        })
        .response;

    if full.chars().count() > 60 {
        response.on_hover_text(full);
    }
}

/// Nothing chosen yet: the two ways in.
fn empty(ui: &mut Ui, view: &mut TradingView, market: &Market, snapshot: &Snapshot) {
    egui::CentralPanel::default()
        .frame(egui::Frame::new().fill(palette::background()))
        .show(ui, |ui| {
            ui.add_space(ui.available_height() * 0.22);

            ui.vertical_centered(|ui| {
                ui.label(
                    RichText::new("Forecast the next ten candles")
                        .font(theme::voice(26.0))
                        .color(palette::text()),
                );

                ui.add_space(space::XS);

                ui.label(
                    RichText::new("Liquid, dendritic and spiking networks, or your chat model")
                        .color(palette::text_secondary()),
                );

                ui.add_space(space::XL);

                ui.horizontal(|ui| {
                    let width = 520.0;

                    ui.add_space((ui.available_width() - width).max(0.0) / 2.0);

                    let connected = market.is_connected();

                    if big_button(
                        ui,
                        if connected {
                            "Choose an instrument"
                        } else {
                            "Connect Dhan"
                        },
                        "Live data in real time",
                    )
                    .clicked()
                    {
                        if connected {
                            view.picking = true;
                        } else {
                            view.connecting = true;
                        }
                    }

                    ui.add_space(space::M);

                    if big_button(ui, "Import", "A CSV, or a chart picture").clicked()
                        && let Some(path) = choose_file()
                    {
                        view.open_file(market, &path);
                    }
                });

                if let Connection::Failed(reason) = &snapshot.connection {
                    ui.add_space(space::M);
                    ui.label(RichText::new(reason).color(palette::bad()));
                }
            });
        });
}

fn big_button(ui: &mut Ui, title: &str, detail: &str) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(254.0, 74.0), Sense::click());

    let fill = if response.hovered() {
        palette::raised()
    } else {
        palette::surface()
    };

    ui.painter().rect(
        rect,
        CornerRadius::same(theme::radius::L),
        fill,
        Stroke::new(1.0, palette::border()),
        egui::StrokeKind::Inside,
    );

    ui.painter().text(
        rect.left_top() + vec2(16.0, 18.0),
        egui::Align2::LEFT_TOP,
        title,
        theme::strong(15.0),
        palette::text(),
    );

    ui.painter().text(
        rect.left_top() + vec2(16.0, 42.0),
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

    let modal = egui::Modal::new(ui.id().with("dhan")).show(ui.ctx(), |ui| {
        ui.set_width(380.0);

        ui.label(RichText::new("Dhan").font(theme::strong(18.0)));
        ui.add_space(space::XS);
        ui.label(
            RichText::new("web.dhan.co → My Profile → Access DhanHQ APIs. Tokens last a day; data needs Dhan's data plan.")
                .font(theme::small())
                .color(palette::text_muted()),
        );

        ui.add_space(space::M);

        ui.add(
            egui::TextEdit::singleline(&mut view.client_id)
                .hint_text("Client ID")
                .desired_width(f32::INFINITY),
        );
        ui.add(
            egui::TextEdit::singleline(&mut view.token)
                .hint_text("Access token")
                .password(true)
                .desired_width(f32::INFINITY),
        );

        ui.add_space(space::S);

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
                ui.label(RichText::new(reason).color(palette::bad()));
            }
            Connection::Off => {}
        }

        ui.add_space(space::S);

        ui.horizontal(|ui| {
            let ready = !view.client_id.trim().is_empty() && !view.token.trim().is_empty();

            if ui.add_enabled(ready, egui::Button::new("Connect")).clicked() {
                market.connect(&view.client_id, &view.token);
                view.token.clear();
            }

            if matches!(snapshot.connection, Connection::Connected(_)) && ui.button("Disconnect").clicked() {
                market.disconnect();
            }

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.button("Done").clicked() {
                    view.connecting = false;
                }
            });
        });
    });

    if modal.should_close() {
        view.connecting = false;
    }

    // Once connected and watching, the form has done its job.
    if matches!(snapshot.connection, Connection::Connected(_))
        && snapshot.source.is_some()
        && view.token.is_empty()
    {
        let opened_for = ui
            .ctx()
            .data(|data| data.get_temp::<u64>(ui.id().with("dhan opened")));

        if opened_for.is_none() {
            ui.ctx()
                .data_mut(|data| data.insert_temp(ui.id().with("dhan opened"), snapshot.revision));
        }
    }
}

/// Searching for an instrument.
fn picker_window(ui: &mut Ui, view: &mut TradingView, market: &Market) {
    if !view.picking {
        return;
    }

    let mut chosen: Option<Instrument> = None;

    let modal = egui::Modal::new(ui.id().with("instrument")).show(ui.ctx(), |ui| {
        ui.set_width(420.0);

        let field = ui.add(
            egui::TextEdit::singleline(&mut view.query)
                .hint_text("Search: RELIANCE, NIFTY, TCS…")
                .desired_width(f32::INFINITY),
        );

        field.request_focus();

        ui.add_space(space::S);

        let found = market.search(&view.query, 40);

        egui::ScrollArea::vertical()
            .max_height(360.0)
            .show(ui, |ui| {
                for instrument in &found {
                    let response = ui
                        .horizontal(|ui| {
                            ui.set_width(ui.available_width());

                            ui.label(RichText::new(&instrument.symbol).font(theme::strong(13.5)));
                            ui.label(
                                RichText::new(&instrument.name)
                                    .font(theme::small())
                                    .color(palette::text_muted()),
                            );

                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                ui.label(
                                    RichText::new(instrument.segment.exchange())
                                        .font(theme::small())
                                        .color(palette::text_muted()),
                                );
                            });
                        })
                        .response
                        .interact(Sense::click());

                    if response.hovered() {
                        ui.painter().rect_filled(
                            response.rect.expand(2.0),
                            CornerRadius::same(theme::radius::M),
                            palette::raised().gamma_multiply(0.6),
                        );
                    }

                    if response.clicked() {
                        chosen = Some(instrument.clone());
                    }
                }
            });

        if !market.has_instrument_list() {
            ui.add_space(space::S);

            if ui
                .button("Load Dhan's full list")
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
        market.watch(instrument);
        view.picking = false;
        view.chart = chart::ChartState::default();
        view.tab = Tab::Chart;
    } else if modal.should_close() {
        view.picking = false;
    }
}
