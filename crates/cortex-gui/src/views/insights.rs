//! Token usage, cost estimates, and session activity.

use std::sync::mpsc::{Receiver, channel};
use std::time::{SystemTime, UNIX_EPOCH};

use cortex_core::ledger::{
    self, CostEstimate, DayUse, Ledger, MonthReport, PriceTable, SessionUse,
};
use eframe::egui::{self, Align2, Rect, RichText, ScrollArea, Sense, Stroke, Ui, pos2, vec2};

use crate::theme::{self, palette};

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

const WEEKDAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

/// How long what was read is shown before it is read again, while the
/// view is open.
const FRESH_FOR: f64 = 30.0;
const MODEL_ROW_HEIGHT: f32 = 80.0;
const VISIBLE_MODELS: usize = 5;

/// Everything read from the records at once.
struct Read {
    month: MonthReport,
    streak: u32,
}

pub struct InsightsView {
    /// The month shown, on the person's own calendar.
    year: i32,
    month: u32,

    /// Minutes the person's clock is ahead of UTC.
    offset: i32,

    read: Option<Read>,
    reading: Option<Receiver<Read>>,

    /// When what is shown was read, on the window's clock.
    read_at: f64,

    /// The day picked on the calendar, `YYYY-MM-DD`.
    day: Option<String>,
    sessions_expanded: bool,
}

impl Default for InsightsView {
    fn default() -> Self {
        let offset = cortex_cli::commands::usage::utc_offset_minutes().unwrap_or(0);
        let (year, month) = ledger::local_month(now_ms(), offset);

        Self {
            year,
            month,
            offset,
            read: None,
            reading: None,
            read_at: f64::NEG_INFINITY,
            day: None,
            sessions_expanded: false,
        }
    }
}

impl InsightsView {
    /// Reads the records again, on a thread of its own, unless that is
    /// already under way.
    fn refresh(&mut self, context: &egui::Context, now: f64) {
        if self.reading.is_some() {
            return;
        }

        let (year, month, offset) = (self.year, self.month, self.offset);
        let (tell, told) = channel();
        let context = context.clone();

        std::thread::spawn(move || {
            let read = read(year, month, offset);

            let _ = tell.send(read);

            context.request_repaint();
        });

        self.reading = Some(told);
        self.read_at = now;
    }

    fn receive(&mut self) {
        let Some(reading) = &self.reading else {
            return;
        };

        if let Ok(read) = reading.try_recv() {
            self.read = Some(read);
            self.reading = None;
        }
    }

    fn turn_month(&mut self, by: i32, context: &egui::Context, now: f64) {
        let index = self.year * 12 + self.month as i32 - 1 + by;

        self.year = index.div_euclid(12);
        self.month = (index.rem_euclid(12) + 1) as u32;
        self.day = None;
        self.sessions_expanded = false;
        self.reading = None;

        self.refresh(context, now);
    }
}

fn read(year: i32, month: u32, offset: i32) -> Read {
    let prices = PriceTable::at_home();

    let (month, streak) = match Ledger::at_home() {
        Some(ledger) => (
            ledger.month(year, month, offset, &prices),
            ledger.streak_days(now_ms(), offset),
        ),

        None => (MonthReport::default(), 0),
    };

    Read { month, streak }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as u64)
}

pub fn show(ui: &mut Ui, view: &mut InsightsView) {
    let now = ui.input(|input| input.time);

    view.receive();

    if view.reading.is_none() && now - view.read_at > FRESH_FOR {
        view.refresh(ui.ctx(), now);
    }

    // Read again while open, so a run that finishes shows up.
    ui.ctx()
        .request_repaint_after(std::time::Duration::from_secs_f64(FRESH_FOR));

    ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        ui.add_space(theme::space::XXL);

        let width = ui.available_width().min(980.0);
        let margin = ((ui.available_width() - width) / 2.0).max(theme::space::L);

        ui.horizontal(|ui| {
            ui.add_space(margin);

            ui.vertical(|ui| {
                ui.set_width(width - 2.0 * theme::space::L);

                let Some(read) = &view.read else {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        muted(ui, "Loading…");
                    });

                    return;
                };

                let month = read.month.clone();
                let streak = read.streak;
                usage(ui, view, &month, streak, now);

                ui.add_space(30.0);
            });
        });
    });
}

fn month_picker(ui: &mut Ui, view: &mut InsightsView, now: f64) {
    let label = format!("{} {}", MONTHS[(view.month - 1) as usize], view.year);
    let galley = ui
        .painter()
        .layout_no_wrap(label, theme::strong(16.0), palette::text());
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::hover());
    let button = 28.0;
    let group = Rect::from_center_size(
        rect.center(),
        vec2(
            galley.size().x + 2.0 * (button + theme::space::M),
            rect.height(),
        ),
    );
    let previous = Rect::from_min_size(group.min, vec2(button, rect.height()));
    let next = Rect::from_min_size(
        pos2(group.right() - button, group.top()),
        vec2(button, rect.height()),
    );
    ui.painter().galley(
        pos2(
            rect.center().x - galley.size().x / 2.0,
            rect.center().y - galley.size().y / 2.0,
        ),
        galley,
        palette::text(),
    );
    if ui
        .put(previous, egui::Button::new("‹"))
        .on_hover_text("Previous month")
        .clicked()
    {
        view.turn_month(-1, ui.ctx(), now);
    }
    if ui
        .put(next, egui::Button::new("›"))
        .on_hover_text("Next month")
        .clicked()
    {
        view.turn_month(1, ui.ctx(), now);
    }
    if view.reading.is_some() {
        ui.put(
            Rect::from_center_size(
                pos2(group.right() + theme::space::L, rect.center().y),
                vec2(12.0, 12.0),
            ),
            egui::Spinner::new(),
        );
    }
}

fn heading(ui: &mut Ui, text: &str) {
    ui.label(
        RichText::new(text)
            .font(theme::strong(theme::size::UI))
            .color(palette::text()),
    );
    ui.add_space(theme::space::S);
}

fn card(ui: &mut Ui, title: Option<&str>, body: impl FnOnce(&mut Ui)) {
    egui::Frame::new()
        .fill(palette::surface())
        .corner_radius(egui::CornerRadius::same(theme::radius::XL))
        .inner_margin(egui::Margin::same(18))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = theme::space::S;
            if let Some(title) = title {
                heading(ui, title);
            }
            body(ui);
        });
}

fn activity(ui: &mut Ui, view: &mut InsightsView, report: &MonthReport) {
    card(ui, Some("Activity"), |ui| {
        calendar(ui, view, report);
        ui.add_space(theme::space::XL);
        heading(ui, "Hourly usage");
        hours(
            ui,
            &report
                .summary
                .hours
                .iter()
                .map(|hour| hour.tokens)
                .collect::<Vec<_>>(),
        );
    });
}

fn models(ui: &mut Ui, report: &MonthReport) {
    card(ui, Some("Models"), |ui| {
        ui.spacing_mut().scroll.floating = false;
        model_scroll(ui, report);
    });
}

fn model_scroll(ui: &mut Ui, report: &MonthReport) -> egui::scroll_area::ScrollAreaOutput<()> {
    let height = (report.summary.models.len().min(VISIBLE_MODELS) as f32 * MODEL_ROW_HEIGHT)
        .max(theme::control::SMALL);
    ScrollArea::vertical()
        .id_salt("insights models")
        .max_height(height)
        .min_scrolled_height(height)
        .auto_shrink([false, true])
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
        .show(ui, |ui| model_list(ui, report))
}

fn model_list(ui: &mut Ui, report: &MonthReport) {
    ui.spacing_mut().item_spacing.y = 0.0;
    if report.summary.models.is_empty() {
        muted(ui, "No usage yet");
    }
    for model in &report.summary.models {
        let name = if model.model.is_empty() {
            "Unknown model"
        } else {
            &model.model
        };
        let (rect, response) =
            ui.allocate_exact_size(vec2(ui.available_width(), MODEL_ROW_HEIGHT), Sense::hover());
        let mut job = egui::text::LayoutJob::simple(
            name.to_string(),
            theme::ui(),
            palette::text(),
            rect.width(),
        );
        job.wrap.max_rows = 1;
        let title = ui.painter().layout_job(job);
        ui.painter()
            .galley(rect.min + vec2(0.0, 4.0), title, palette::text());
        ui.painter().text(
            rect.min + vec2(0.0, 30.0),
            Align2::LEFT_TOP,
            format!("{} tokens", count(model.totals.tokens())),
            theme::small(),
            palette::text_secondary(),
        );
        let share = if model.share > 0.0 && model.share < 0.01 {
            "<1%".to_string()
        } else {
            format!("{:.0}%", model.share * 100.0)
        };
        ui.painter().text(
            pos2(rect.right(), rect.top() + 30.0),
            Align2::RIGHT_TOP,
            share,
            theme::small(),
            palette::text_muted(),
        );
        let bar = Rect::from_min_size(rect.min + vec2(0.0, 54.0), vec2(rect.width(), 3.0));
        ui.painter().rect_filled(bar, 2.0, palette::sunken());
        ui.painter().rect_filled(
            Rect::from_min_size(
                bar.min,
                vec2(
                    bar.width() * model.share.clamp(0.0, 1.0) as f32,
                    bar.height(),
                ),
            ),
            2.0,
            palette::text_muted(),
        );
        response.on_hover_text(format!(
            "{name}\n{} · {}\n{}",
            model.provider,
            cost_text(&model.cost),
            cost_note(&model.cost)
        ));
    }
}

fn usage(ui: &mut Ui, view: &mut InsightsView, report: &MonthReport, streak: u32, now: f64) {
    let summary = &report.summary;

    month_picker(ui, view, now);
    ui.add_space(theme::space::L);

    let mut figures = Vec::new();
    figure(
        &mut figures,
        "Tokens",
        &count(summary.totals.tokens()),
        &format!(
            "{} in · {} out",
            count(summary.totals.all_input()),
            count(summary.totals.output_tokens)
        ),
    );
    figure(
        &mut figures,
        "Cost",
        &cost_text(&summary.cost),
        &cost_note(&summary.cost),
    );
    figure(
        &mut figures,
        "Sessions",
        &summary.session_count.to_string(),
        &format!("{} calls", summary.totals.calls),
    );
    figure(
        &mut figures,
        "Cache hit",
        &summary
            .cache_hit_share
            .map_or("—".to_string(), |share| format!("{:.0}%", share * 100.0)),
        "Share of input tokens served from cache",
    );
    card(ui, None, |ui| metrics(ui, &figures));

    ui.add_space(theme::space::L);
    if ui.available_width() >= 780.0 {
        let rail_width = 260.0;
        let gap = theme::space::XL;
        let chart_width = ui.available_width() - rail_width - gap;
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            ui.allocate_ui_with_layout(
                vec2(chart_width, 0.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_width(chart_width);
                    activity(ui, view, report);
                },
            );
            ui.allocate_ui_with_layout(
                vec2(rail_width, 0.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_width(rail_width);
                    models(ui, report);
                },
            );
        });
    } else {
        activity(ui, view, report);
        ui.add_space(theme::space::L);
        models(ui, report);
    }

    let picked = view.day.clone();

    ui.add_space(theme::space::L);
    let session_title = picked
        .as_ref()
        .map_or("Sessions".to_string(), |day| format!("Sessions · {day}"));
    card(ui, Some(&session_title), |ui| {
        if picked.is_some() && ui.small_button("All sessions").clicked() {
            view.day = None;
        }

        let shown: Vec<&SessionUse> = summary
            .sessions
            .iter()
            .filter(|session| match &picked {
                Some(day) => touches(session, day, view.offset),
                None => true,
            })
            .take(60)
            .collect();

        if shown.is_empty() {
            muted(ui, "No sessions.");
        }

        if !shown.is_empty() {
            let visible = if view.sessions_expanded {
                shown.len()
            } else {
                shown.len().min(8)
            };
            session_table(ui, &shown[..visible]);
            if shown.len() > 8 {
                let label = if view.sessions_expanded {
                    "Show fewer".to_string()
                } else {
                    format!("Show all ({})", shown.len())
                };
                if ui.small_button(label).clicked() {
                    view.sessions_expanded = !view.sessions_expanded;
                }
            }
        }
    });

    ui.add_space(theme::space::L);
    card(ui, None, |ui| {
        ui.collapsing("Details", |ui| {
            let mut details = Vec::new();
            figure(&mut details, "Streak", &format!("{streak} days"), "");
            figure(
                &mut details,
                "Top model",
                summary.favourite_model.as_deref().unwrap_or("—"),
                "",
            );
            figure(
                &mut details,
                "Peak day",
                &summary
                    .peak_day
                    .as_ref()
                    .map_or("—".to_string(), |peak| peak.date.clone()),
                "",
            );
            figure(
                &mut details,
                "Peak hour",
                &summary
                    .peak_hour
                    .map_or("—".to_string(), |hour| format!("{:02}:00", hour.hour)),
                "",
            );
            metrics(ui, &details);
            if let Some(per_call) = summary.per_call {
                muted(
                    ui,
                    &format!(
                        "Average call: {} in · {} out",
                        count(per_call.input_tokens as u64),
                        count(per_call.output_tokens as u64)
                    ),
                );
            }
            muted(
                ui,
                "Costs use list prices; subscriptions are counted separately.",
            );
            muted(
                ui,
                "Prices: ~/.cortex/prices.json · Records: ~/.cortex/usage",
            );
        });
    });
}

/// The month as a grid of days, each as dark as the tokens used on it.
fn calendar(ui: &mut Ui, view: &mut InsightsView, report: &MonthReport) {
    let gap = 6.0;
    let cell = ((ui.available_width() - gap * 6.0) / 7.0).max(12.0);
    let height = (cell * 0.5).clamp(28.0, 48.0);

    let most = report
        .days
        .iter()
        .map(|day| day.totals.tokens())
        .max()
        .unwrap_or(0)
        .max(1);

    let rows = (report.first_weekday as usize + report.days.len()).div_ceil(7);

    let (rect, _) = ui.allocate_exact_size(
        vec2(cell * 7.0 + gap * 6.0, 18.0 + rows as f32 * (height + gap)),
        Sense::hover(),
    );

    let painter = ui.painter_at(rect);

    for (column, name) in WEEKDAYS.iter().enumerate() {
        painter.text(
            pos2(rect.left() + column as f32 * (cell + gap) + 4.0, rect.top()),
            Align2::LEFT_TOP,
            *name,
            theme::small(),
            palette::text_muted(),
        );
    }

    for (at, day) in report.days.iter().enumerate() {
        let slot = report.first_weekday as usize + at;
        let (row, column) = (slot / 7, slot % 7);

        let cell_rect = Rect::from_min_size(
            pos2(
                rect.left() + column as f32 * (cell + gap),
                rect.top() + 18.0 + row as f32 * (height + gap),
            ),
            vec2(cell, height),
        );

        let tokens = day.totals.tokens();

        // On a square-root scale, so that a quiet day is still visible
        // beside a heavy one.
        let strength = (tokens as f32 / most as f32).sqrt();

        let fill = if tokens == 0 {
            palette::sunken()
        } else {
            activity_fill(strength)
        };

        let picked = view.day.as_deref() == Some(day.date.as_str());

        painter.rect_filled(cell_rect, 6.0, fill);

        if picked {
            painter.rect_stroke(
                cell_rect,
                6.0,
                Stroke::new(2.0, palette::text()),
                egui::StrokeKind::Inside,
            );
        }

        let text = palette::text();

        painter.text(
            cell_rect.left_top() + vec2(6.0, 4.0),
            Align2::LEFT_TOP,
            (at + 1).to_string(),
            theme::small(),
            text,
        );

        if tokens > 0 {
            painter.text(
                cell_rect.right_bottom() - vec2(6.0, 4.0),
                Align2::RIGHT_BOTTOM,
                count(tokens),
                theme::small(),
                text,
            );
        }

        let response = ui.interact(
            cell_rect,
            ui.id().with(("insights day", at)),
            Sense::click(),
        );

        if response.clicked() && tokens > 0 {
            view.day = (!picked).then(|| day.date.clone());
        }

        if tokens > 0 {
            response.on_hover_text(day_text(day));
        }
    }
}

fn day_text(day: &DayUse) -> String {
    let totals = &day.totals;

    format!(
        "{}\n{} tokens: {} sent ({} from the cache), {} received\n{} calls in {} sessions\n{}",
        day.date,
        count(totals.tokens()),
        count(totals.all_input()),
        count(totals.cached_input_tokens),
        count(totals.output_tokens),
        totals.calls,
        day.sessions,
        cost_text(&day.cost)
    )
}

/// Whether a session made calls on `day`, by its first and last call.
fn touches(session: &SessionUse, day: &str, offset: i32) -> bool {
    let date = |ms: u64| {
        let local = ms as i64 + i64::from(offset) * 60_000;
        let (year, month, day) = ledger::civil(local.div_euclid(86_400_000));

        format!("{year:04}-{month:02}-{day:02}")
    };

    date(session.first_ms).as_str() <= day && day <= date(session.last_ms).as_str()
}

/// Twenty-four bars, midnight first.
fn hours(ui: &mut Ui, tokens: &[u64]) {
    if tokens.iter().all(|tokens| *tokens == 0) {
        muted(ui, "No usage yet");

        return;
    }

    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 92.0), Sense::hover());
    let painter = ui.painter_at(rect);

    let most = tokens.iter().copied().max().unwrap_or(1).max(1);
    let each = rect.width() / 24.0;

    for (hour, tokens) in tokens.iter().enumerate() {
        let height = (*tokens as f32 / most as f32) * (rect.height() - 18.0);
        let left = rect.left() + hour as f32 * each;

        painter.rect_filled(
            Rect::from_min_max(
                pos2(left + 1.5, rect.bottom() - 16.0 - height.max(1.0)),
                pos2(left + each - 1.5, rect.bottom() - 16.0),
            ),
            2.0,
            if *tokens == most {
                palette::accent()
            } else {
                palette::accent().gamma_multiply(0.55)
            },
        );

        if hour % 3 == 0 {
            painter.text(
                pos2(left + 2.0, rect.bottom() - 13.0),
                Align2::LEFT_TOP,
                format!("{hour:02}"),
                theme::small(),
                palette::text_muted(),
            );
        }
    }
}

fn session_table(ui: &mut Ui, sessions: &[&SessionUse]) {
    let width = ui.available_width().max(480.0);
    let model = (width * 0.2).clamp(100.0, 180.0);
    let widths = [width - model - 160.0, model, 68.0, 92.0];
    ScrollArea::horizontal()
        .id_salt("sessions table")
        .show(ui, |ui| {
            ui.set_min_width(width);
            ui.spacing_mut().item_spacing.y = 0.0;
            table_row(
                ui,
                &widths,
                ["Session", "Model", "Tokens", "Cost"],
                true,
                false,
            );
            for (index, session) in sessions.iter().enumerate() {
                let tokens = count(session.totals.tokens());
                let cost = cost_text(&session.cost);
                let cost_label = match cost.as_str() {
                    "subscription" => "Plan",
                    "unpriced" => "—",
                    _ => &cost,
                };
                let title = session.title.as_deref().unwrap_or(&session.id);
                table_row(
                    ui,
                    &widths,
                    [title, &session.model, &tokens, cost_label],
                    false,
                    index % 2 == 0,
                )
                .on_hover_text(format!(
                    "{title}\n{} · {cost}\n{}",
                    session.model,
                    cost_note(&session.cost)
                ));
            }
        });
}

fn table_row(
    ui: &mut Ui,
    widths: &[f32; 4],
    values: [&str; 4],
    header: bool,
    shaded: bool,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(widths.iter().sum(), 30.0), Sense::hover());
    if header || shaded || response.hovered() {
        ui.painter().rect_filled(
            rect,
            egui::CornerRadius::same(theme::radius::M),
            if header || response.hovered() {
                palette::raised()
            } else {
                palette::background()
            },
        );
    }
    let mut left = rect.left() + theme::space::S;
    for (column, (width, value)) in widths.iter().zip(values).enumerate() {
        let font = if header { theme::small() } else { theme::ui() };
        let color = if header {
            palette::text_muted()
        } else if column == 0 {
            palette::text()
        } else {
            palette::text_secondary()
        };
        let mut job =
            egui::text::LayoutJob::simple(value.to_string(), font, color, (width - 12.0).max(0.0));
        job.wrap.max_rows = 1;
        let galley = ui.painter().layout_job(job);
        let x = if column >= 2 {
            left + width - 2.0 * theme::space::S - galley.size().x
        } else {
            left
        };
        ui.painter().galley(
            pos2(x, rect.center().y - galley.size().y / 2.0),
            galley,
            color,
        );
        left += width;
    }
    response
}

fn figure(figures: &mut Vec<[String; 3]>, name: &str, value: &str, note: &str) {
    figures.push([name.to_string(), value.to_string(), note.to_string()]);
}

fn metrics(ui: &mut Ui, figures: &[[String; 3]]) {
    let gap = theme::space::L;
    let room = ui.available_width();
    let columns = if room >= 4.0 * 150.0 + 3.0 * gap {
        4
    } else if room >= 2.0 * 150.0 + gap {
        2
    } else {
        1
    };
    let width = (room - gap * (columns - 1) as f32) / columns as f32;
    for row in figures.chunks(columns) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for [name, value, note] in row {
                ui.allocate_ui_with_layout(
                    vec2(width, 62.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.set_min_width(width);
                        ui.set_max_width(width);
                        muted(ui, name);
                        ui.add(
                            egui::Label::new(
                                RichText::new(value)
                                    .font(theme::strong(20.0))
                                    .color(palette::text()),
                            )
                            .halign(egui::Align::Min)
                            .truncate(),
                        )
                        .on_hover_text(if note.is_empty() {
                            value
                        } else {
                            note
                        });
                    },
                );
            }
        });
    }
}

fn activity_fill(strength: f32) -> egui::Color32 {
    let base = palette::sunken();
    let ink = palette::text_muted();
    let mix = |a: u8, b: u8| (f32::from(a) + (f32::from(b) - f32::from(a)) * strength * 0.4) as u8;
    egui::Color32::from_rgb(
        mix(base.r(), ink.r()),
        mix(base.g(), ink.g()),
        mix(base.b(), ink.b()),
    )
}

fn cost_text(cost: &CostEstimate) -> String {
    match cost.usd {
        Some(usd) if cost.is_complete() => format!("${usd:.2}"),
        Some(usd) => format!("${usd:.2}+"),
        None if cost.subscription_tokens > 0 && cost.unpriced_tokens == 0 => "subscription".into(),
        None if cost.unpriced_tokens > 0 => "unpriced".into(),
        None => "$0.00".into(),
    }
}

fn cost_note(cost: &CostEstimate) -> String {
    let mut parts = Vec::new();

    if cost.subscription_tokens > 0 {
        parts.push(format!(
            "{} on a subscription",
            count(cost.subscription_tokens)
        ));
    }

    if cost.unpriced_tokens > 0 {
        parts.push(format!("{} without a price", count(cost.unpriced_tokens)));
    }

    if parts.is_empty() {
        "estimated from list prices".to_string()
    } else {
        parts.join(" · ")
    }
}

/// A count of tokens, short: 950, 12.4k, 3.1M.
fn count(tokens: u64) -> String {
    match tokens {
        0..1_000 => tokens.to_string(),
        1_000..1_000_000 => format!("{:.1}k", tokens as f64 / 1_000.0),
        1_000_000..1_000_000_000 => format!("{:.1}M", tokens as f64 / 1_000_000.0),
        _ => format!("{:.2}B", tokens as f64 / 1_000_000_000.0),
    }
}

fn muted(ui: &mut Ui, text: &str) {
    ui.label(
        RichText::new(text)
            .font(theme::small())
            .color(palette::text_muted()),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn models_scroll_after_five_rows_without_growing_the_viewport() {
        for count in [3, 5, 12] {
            let context = egui::Context::default();
            let mut report = MonthReport::default();
            report.summary.models = (0..count)
                .map(|index| cortex_core::ledger::ModelUse {
                    model: format!("model-{index}"),
                    ..Default::default()
                })
                .collect();
            let mut viewport = 0.0;
            let mut content = 0.0;
            let _ = context.run_ui(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, vec2(800.0, 600.0))),
                    ..Default::default()
                },
                |ui| {
                    ui.allocate_ui_with_layout(
                        vec2(224.0, 0.0),
                        egui::Layout::top_down(egui::Align::Min),
                        |ui| {
                            ui.set_width(224.0);
                            let output = model_scroll(ui, &report);
                            viewport = output.inner_rect.height();
                            content = output.content_size.y;
                        },
                    );
                },
            );
            assert!(
                (viewport - count.min(5) as f32 * MODEL_ROW_HEIGHT).abs() < 1.0,
                "{count}: viewport {viewport}"
            );
            assert!(
                (content - count as f32 * MODEL_ROW_HEIGHT).abs() < 1.0,
                "{count}: content {content}"
            );
        }
    }

    #[test]
    fn counts_are_short() {
        assert_eq!(count(950), "950");
        assert_eq!(count(12_400), "12.4k");
        assert_eq!(count(3_100_000), "3.1M");
        assert_eq!(count(2_500_000_000), "2.50B");
    }

    #[test]
    fn a_cost_says_what_it_leaves_out() {
        let priced = CostEstimate {
            usd: Some(1.234),
            priced_tokens: 10,
            ..CostEstimate::default()
        };

        assert_eq!(cost_text(&priced), "$1.23");

        let partly = CostEstimate {
            unpriced_tokens: 5,
            ..priced
        };

        assert_eq!(cost_text(&partly), "$1.23+");

        let subscription = CostEstimate {
            subscription_tokens: 40,
            ..CostEstimate::default()
        };

        assert_eq!(cost_text(&subscription), "subscription");
    }

    #[test]
    fn a_session_is_on_the_days_between_its_first_and_last_call() {
        let day = 86_400_000;

        let session = SessionUse {
            first_ms: 20_000 * day + 3_600_000,
            last_ms: 20_002 * day,
            ..SessionUse::default()
        };

        let date = |days: i64| {
            let (year, month, day) = ledger::civil(days);

            format!("{year:04}-{month:02}-{day:02}")
        };

        assert!(touches(&session, &date(20_000), 0));
        assert!(touches(&session, &date(20_001), 0));
        assert!(!touches(&session, &date(20_003), 0));

        // Two hours behind UTC, the first call was still on the day before.
        assert!(touches(&session, &date(19_999), -120));
    }

    #[test]
    fn months_turn_over_the_year() {
        let mut view = InsightsView {
            year: 2026,
            month: 12,
            ..InsightsView::default()
        };

        let context = egui::Context::default();

        view.turn_month(1, &context, 0.0);
        assert_eq!((view.year, view.month), (2027, 1));

        view.turn_month(-2, &context, 0.0);
        assert_eq!((view.year, view.month), (2026, 11));
    }
}
