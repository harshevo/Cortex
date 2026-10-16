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
                    ui.label(RichText::new("Reading the records…").color(palette::text_muted()));

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

fn usage(ui: &mut Ui, view: &mut InsightsView, report: &MonthReport, streak: u32, now: f64) {
    let summary = &report.summary;

    ui.horizontal(|ui| {
        if ui.button("‹").clicked() {
            view.turn_month(-1, ui.ctx(), now);
        }

        ui.label(
            RichText::new(format!(
                "{} {}",
                MONTHS[(view.month - 1) as usize],
                view.year
            ))
            .font(theme::strong(16.0))
            .color(palette::text()),
        );

        if ui.button("›").clicked() {
            view.turn_month(1, ui.ctx(), now);
        }

        ui.add_space(12.0);

        if view.reading.is_some() {
            ui.label(
                RichText::new("reading…")
                    .font(theme::small())
                    .color(palette::text_muted()),
            );
        }
    });

    ui.add_space(10.0);

    // The figures that answer "how much".
    let mut tiles: Vec<[String; 3]> = Vec::new();

    {
        let figures = &mut tiles;

        figure(
            figures,
            "Tokens",
            &count(summary.totals.tokens()),
            &format!(
                "{} in · {} out",
                count(summary.totals.all_input()),
                count(summary.totals.output_tokens)
            ),
        );

        figure(
            figures,
            "Cost",
            &cost_text(&summary.cost),
            &cost_note(&summary.cost),
        );

        figure(
            figures,
            "Sessions",
            &summary.session_count.to_string(),
            &format!("{} model calls", summary.totals.calls),
        );

        figure(
            figures,
            "Streak",
            &format!("{streak} {}", if streak == 1 { "day" } else { "days" }),
            "",
        );

        figure(
            figures,
            "Favourite model",
            summary.favourite_model.as_deref().unwrap_or("—"),
            "",
        );

        figure(
            figures,
            "Peak day",
            &summary
                .peak_day
                .as_ref()
                .map_or("—".to_string(), |peak| peak.date.clone()),
            &summary.peak_day.as_ref().map_or(String::new(), |peak| {
                format!("{} tokens", count(peak.tokens))
            }),
        );

        figure(
            figures,
            "Peak hour",
            &summary
                .peak_hour
                .map_or("—".to_string(), |hour| format!("{:02}:00", hour.hour)),
            "",
        );

        figure(
            figures,
            "From the cache",
            &summary
                .cache_hit_share
                .map_or("—".to_string(), |share| format!("{:.0}%", share * 100.0)),
            "of input",
        );
    }

    tiles_in_rows(ui, &tiles);

    section(ui, "By day");
    calendar(ui, view, report);

    if let Some(per_call) = summary.per_call {
        muted(
            ui,
            &format!(
                "An average call sends {} tokens and gets {} back.",
                count(per_call.input_tokens as u64),
                count(per_call.output_tokens as u64)
            ),
        );
    }

    section(ui, "By hour of the day");
    hours(
        ui,
        &summary
            .hours
            .iter()
            .map(|hour| hour.tokens)
            .collect::<Vec<_>>(),
    );

    section(ui, "By model");

    if summary.models.is_empty() {
        muted(ui, "Nothing used this month.");
    }

    for model in &summary.models {
        share_row(
            ui,
            &format!("{} · {}", model.model, model.provider),
            model.share,
            &format!(
                "{} tokens · {}",
                count(model.totals.tokens()),
                cost_text(&model.cost)
            ),
        );
    }

    let picked = view.day.clone();

    section(
        ui,
        &match &picked {
            Some(day) => format!("Sessions on {day}"),
            None => "Sessions".to_string(),
        },
    );

    if picked.is_some() && ui.small_button("Show the whole month").clicked() {
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

    for session in shown {
        session_row(ui, session);
    }

    ui.add_space(8.0);

    ui.label(
        RichText::new("Prices from ~/.cortex/prices.json · records in ~/.cortex/usage")
            .font(theme::small())
            .color(palette::text_muted()),
    )
    .on_hover_text(
        "A model without a known price is counted but not costed, and ChatGPT sign-in \
         sessions are covered by the subscription. Every model call of every session on this \
         machine is recorded.",
    );
}

/// The month as a grid of days, each as dark as the tokens used on it.
fn calendar(ui: &mut Ui, view: &mut InsightsView, report: &MonthReport) {
    let gap = 6.0;
    let cell = ((ui.available_width() - gap * 6.0) / 7.0).clamp(36.0, 96.0);
    let height = cell * 0.62;

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
            palette::accent().gamma_multiply(0.18 + 0.82 * strength)
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

        let text = if strength > 0.55 {
            palette::background()
        } else {
            palette::text_secondary()
        };

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
        muted(ui, "Nothing used this month.");

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

fn session_row(ui: &mut Ui, session: &SessionUse) {
    ui.horizontal(|ui| {
        let title = session.title.clone().unwrap_or_else(|| session.id.clone());

        ui.add_sized(
            vec2(ui.available_width() * 0.5, 18.0),
            egui::Label::new(RichText::new(title).color(palette::text())).truncate(),
        );

        ui.label(
            RichText::new(&session.model)
                .font(theme::small())
                .color(palette::text_muted()),
        );

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                RichText::new(format!(
                    "{} · {}",
                    count(session.totals.tokens()),
                    cost_text(&session.cost)
                ))
                .font(theme::mono())
                .color(palette::text_secondary()),
            );
        });
    });
}

fn figure(figures: &mut Vec<[String; 3]>, name: &str, value: &str, note: &str) {
    figures.push([name.to_string(), value.to_string(), note.to_string()]);
}

/// The figures as cards in whole rows of up to four, as many to a row as
/// the page has room for.
fn tiles_in_rows(ui: &mut Ui, figures: &[[String; 3]]) {
    let gap = theme::space::M;
    let room = ui.available_width();
    let columns = ((room + gap) / (180.0 + gap)).floor().clamp(1.0, 4.0);

    // Less the card's own margins and outline.
    let width = (room - gap * (columns - 1.0)) / columns - 2.0 * theme::space::M - 2.0;

    ui.spacing_mut().item_spacing.y = gap;

    for row in figures.chunks(columns as usize) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;

            for [name, value, note] in row {
                tile(ui, width, name, value, note);
            }
        });
    }
}

fn tile(ui: &mut Ui, width: f32, name: &str, value: &str, note: &str) {
    theme::card().show(ui, |ui| {
        ui.set_width(width);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = theme::space::XS;

            ui.label(
                RichText::new(name.to_uppercase())
                    .font(theme::small())
                    .color(palette::text_muted()),
            );
            ui.add(
                egui::Label::new(
                    RichText::new(value)
                        .font(theme::strong(18.0))
                        .color(palette::text()),
                )
                .truncate(),
            );
            // An empty note still takes its line, so the tiles stay level.
            ui.label(
                RichText::new(if note.is_empty() { " " } else { note })
                    .font(theme::small())
                    .color(palette::text_secondary()),
            );
        });
    });
}

fn share_row(ui: &mut Ui, name: &str, share: f64, note: &str) {
    ui.horizontal(|ui| {
        ui.add_sized(
            vec2(220.0, 18.0),
            egui::Label::new(RichText::new(name).color(palette::text())).truncate(),
        );

        let (rect, _) = ui.allocate_exact_size(
            vec2((ui.available_width() - 220.0).max(60.0), 10.0),
            Sense::hover(),
        );

        let painter = ui.painter_at(rect);

        painter.rect_filled(rect, 3.0, palette::sunken());
        painter.rect_filled(
            Rect::from_min_size(rect.min, vec2(rect.width() * share as f32, rect.height())),
            3.0,
            palette::accent(),
        );

        ui.label(
            RichText::new(note)
                .font(theme::small())
                .color(palette::text_secondary()),
        );
    });
}

fn cost_text(cost: &CostEstimate) -> String {
    match cost.usd {
        Some(usd) if cost.is_complete() => format!("${usd:.2}"),
        Some(usd) => format!("${usd:.2}+"),
        None if cost.subscription_tokens > 0 && cost.unpriced_tokens == 0 => "subscription".into(),
        None if cost.unpriced_tokens > 0 => "no price known".into(),
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

fn section(ui: &mut Ui, text: &str) {
    ui.add_space(theme::space::XL);
    ui.label(
        RichText::new(text.to_uppercase())
            .font(theme::small())
            .color(palette::text_muted()),
    );
    ui.add_space(theme::space::S);
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
