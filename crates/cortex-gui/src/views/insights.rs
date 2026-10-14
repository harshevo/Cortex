//! What the agent has cost and whether its learning parts help: tokens
//! by day, session and model on a calendar, and the experiment that
//! compares runs in which each adviser spoke with runs it was held out
//! of.
//!
//! Both read the records every session keeps under `~/.cortex`. They are
//! read on a thread of their own, since a long history is many lines.

use std::sync::mpsc::{Receiver, channel};
use std::time::{SystemTime, UNIX_EPOCH};

use cortex_core::effect::{
    EffectLedger, EffectReport, Filter, MetricEffect, SourceEffect, Verdict,
};
use cortex_core::ledger::{
    self, CostEstimate, DayUse, Ledger, MonthReport, PriceTable, SessionUse,
};
use eframe::egui::{
    self, Align2, Color32, Rect, RichText, ScrollArea, Sense, Stroke, Ui, pos2, vec2,
};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Usage,
    Effect,
}

/// Everything read from the records at once.
struct Read {
    month: MonthReport,
    streak: u32,
    effect: EffectReport,
}

pub struct InsightsView {
    pub tab: Tab,

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

    /// The adviser whose weeks are drawn.
    adviser: usize,
}

impl Default for InsightsView {
    fn default() -> Self {
        let offset = cortex_cli::commands::usage::utc_offset_minutes().unwrap_or(0);
        let (year, month) = ledger::local_month(now_ms(), offset);

        Self {
            tab: Tab::Usage,
            year,
            month,
            offset,
            read: None,
            reading: None,
            read_at: f64::NEG_INFINITY,
            day: None,
            adviser: 0,
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

    let effect = match EffectLedger::at_home() {
        Some(ledger) => ledger.report(0, u64::MAX, &Filter::default(), offset),
        None => cortex_core::effect::analyse(&[], offset),
    };

    Read {
        month,
        streak,
        effect,
    }
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
        ui.add_space(18.0);

        let width = ui.available_width().min(980.0);
        let margin = ((ui.available_width() - width) / 2.0).max(16.0);

        ui.horizontal(|ui| {
            ui.add_space(margin);

            ui.vertical(|ui| {
                ui.set_width(width - 32.0);

                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Insights")
                            .font(theme::voice(26.0))
                            .color(palette::text()),
                    );

                    ui.add_space(16.0);

                    ui.selectable_value(&mut view.tab, Tab::Usage, "Tokens");
                    ui.selectable_value(&mut view.tab, Tab::Effect, "Does it help?");
                });

                ui.add_space(10.0);

                let Some(read) = &view.read else {
                    ui.label(RichText::new("Reading the records…").color(palette::text_muted()));

                    return;
                };

                match view.tab {
                    Tab::Usage => {
                        let month = read.month.clone();
                        let streak = read.streak;

                        usage(ui, view, &month, streak, now);
                    }

                    Tab::Effect => {
                        let effect = read.effect.clone();

                        effect_tab(ui, view, &effect);
                    }
                }

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
    ui.horizontal_wrapped(|ui| {
        tile(
            ui,
            "Tokens",
            &count(summary.totals.tokens()),
            &format!(
                "{} in · {} out",
                count(summary.totals.all_input()),
                count(summary.totals.output_tokens)
            ),
        );

        tile(
            ui,
            "Cost",
            &cost_text(&summary.cost),
            &cost_note(&summary.cost),
        );

        tile(
            ui,
            "Sessions",
            &summary.session_count.to_string(),
            &format!("{} model calls", summary.totals.calls),
        );

        tile(
            ui,
            "Streak",
            &format!("{streak} {}", if streak == 1 { "day" } else { "days" }),
            "days in a row with use",
        );

        tile(
            ui,
            "Favourite model",
            summary.favourite_model.as_deref().unwrap_or("—"),
            "most tokens went to it",
        );

        tile(
            ui,
            "Peak day",
            &summary
                .peak_day
                .as_ref()
                .map_or("—".to_string(), |peak| peak.date.clone()),
            &summary.peak_day.as_ref().map_or(String::new(), |peak| {
                format!("{} tokens", count(peak.tokens))
            }),
        );

        tile(
            ui,
            "Peak hour",
            &summary
                .peak_hour
                .map_or("—".to_string(), |hour| format!("{:02}:00", hour.hour)),
            "of the day, local",
        );

        tile(
            ui,
            "From the cache",
            &summary
                .cache_hit_share
                .map_or("—".to_string(), |share| format!("{:.0}%", share * 100.0)),
            "of input, billed cheaper",
        );
    });

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

    muted(
        ui,
        "Costs use the prices in ~/.cortex/prices.json and the few built in; a model without a \
         known price is counted but not costed, and ChatGPT sign-in sessions are covered by the \
         subscription. Every model call of every session on this machine is recorded in \
         ~/.cortex/usage.",
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

fn effect_tab(ui: &mut Ui, view: &mut InsightsView, report: &EffectReport) {
    muted(
        ui,
        &format!(
            "In about a quarter of runs each adviser is held out at random: it still watches \
             and learns, but the model is not shown what it says. Runs it spoke in are compared \
             with runs it was held out of. {} runs recorded, {} of them part of the experiment. \
             A verdict needs {} runs on each side.",
            report.runs, report.runs_in_experiment, report.min_per_arm
        ),
    );

    if report.comparisons > 0 {
        muted(
            ui,
            &format!(
                "{} comparisons have enough runs. At 95% about one in twenty shows a difference \
                 by chance alone, so one isolated \"helps\" or \"hurts\" proves little.",
                report.comparisons
            ),
        );
    }

    section(ui, "All advisers together");
    source(ui, &report.overall);

    for adviser in &report.advisers {
        section(ui, &adviser.source);
        source(ui, adviser);
    }

    for briefing in &report.briefings {
        section(ui, &format!("Briefing: {}", briefing.source));
        source(ui, briefing);
    }

    if report.advisers.is_empty() {
        ui.add_space(8.0);
        muted(
            ui,
            "No adviser has spoken in a recorded run yet. Turn on Intuition, Brain (the dendritic \
             network), the decision controller or the mind under Settings → Capabilities, and \
             use the agent: the comparison fills as runs finish.",
        );

        return;
    }

    let names: Vec<&str> = report
        .advisers
        .iter()
        .map(|adviser| adviser.source.as_str())
        .collect();

    view.adviser = view.adviser.min(names.len() - 1);

    section(ui, "By week");

    ui.horizontal_wrapped(|ui| {
        for (at, name) in names.iter().enumerate() {
            ui.selectable_value(&mut view.adviser, at, *name);
        }
    });

    weeks(ui, &report.advisers[view.adviser]);
}

/// One adviser: how often it spoke, then each metric as a row with the
/// difference and its interval drawn about zero.
fn source(ui: &mut Ui, of: &SourceEffect) {
    muted(
        ui,
        &format!(
            "Spoke in {} runs, held out of {} in which it had something to say, nothing to say \
             in {}.",
            of.runs_spoke, of.runs_held_out, of.runs_nothing_to_say
        ),
    );

    for metric in &of.metrics {
        metric_row(ui, metric);
    }

    if of.comparable_observational > 0 {
        muted(
            ui,
            &format!(
                "Not causal: of {} pieces of advice that named something checkable, the model \
                 did it next {} times ({:.0}%).",
                of.comparable_observational,
                of.followed_observational,
                of.followed_observational as f64 / of.comparable_observational as f64 * 100.0
            ),
        );
    }
}

fn metric_row(ui: &mut Ui, metric: &MetricEffect) {
    let rate = metric.metric.is_rate();

    let value = |mean: Option<f64>| match mean {
        None => "—".to_string(),
        Some(mean) if rate => format!("{:.0}%", mean * 100.0),
        Some(mean) => count(mean.round() as u64),
    };

    let (verdict, colour) = match metric.verdict {
        Verdict::Helps => ("helps", palette::good()),
        Verdict::Hurts => ("hurts", palette::bad()),
        Verdict::NoDifferenceSeen => ("no difference seen", palette::text_secondary()),
        Verdict::TooFewRuns => ("too few runs", palette::text_muted()),
    };

    ui.horizontal(|ui| {
        ui.add_sized(
            vec2(120.0, 18.0),
            egui::Label::new(RichText::new(metric.metric.label()).color(palette::text())),
        );

        ui.add_sized(
            vec2(170.0, 18.0),
            egui::Label::new(
                RichText::new(format!(
                    "{} with ({}) · {} without ({})",
                    value(metric.spoke.mean),
                    metric.spoke.n,
                    value(metric.held_out.mean),
                    metric.held_out.n
                ))
                .font(theme::small())
                .color(palette::text_secondary()),
            ),
        );

        let width = (ui.available_width() - 130.0).max(80.0);
        let (rect, _) = ui.allocate_exact_size(vec2(width, 14.0), Sense::hover());

        interval(ui, rect, metric, colour);

        ui.label(RichText::new(verdict).font(theme::small()).color(colour));
    });
}

/// The difference and its interval on an axis centred on no difference.
/// Better is drawn to the right whichever way the metric runs.
fn interval(ui: &Ui, rect: Rect, metric: &MetricEffect, colour: Color32) {
    let painter = ui.painter_at(rect);

    painter.rect_filled(rect, 3.0, palette::sunken());

    let middle = rect.center().x;

    painter.line_segment(
        [pos2(middle, rect.top()), pos2(middle, rect.bottom())],
        Stroke::new(1.0, palette::text_muted()),
    );

    let (Some(difference), Some([low, high])) = (metric.difference, metric.interval) else {
        return;
    };

    let flip = if metric.higher_is_better { 1.0 } else { -1.0 };

    // A rate's difference is at most one; a count's is scaled to the
    // widest end of its own interval.
    let reach = if metric.metric.is_rate() {
        1.0
    } else {
        low.abs().max(high.abs()).max(1e-9)
    };

    let at = |value: f64| {
        let along = (value * flip / reach).clamp(-1.0, 1.0) as f32;

        middle + along * (rect.width() / 2.0 - 4.0)
    };

    let (left, right) = (at(low).min(at(high)), at(low).max(at(high)));

    painter.rect_filled(
        Rect::from_x_y_ranges(left..=right, rect.y_range().shrink(3.0)),
        2.0,
        colour.gamma_multiply(0.45),
    );

    painter.circle_filled(pos2(at(difference), rect.center().y), 4.0, colour);
}

/// The checks pass rate by week, with the adviser speaking and without.
fn weeks(ui: &mut Ui, of: &SourceEffect) {
    use cortex_core::effect::Metric;

    let points: Vec<_> = of
        .weekly
        .iter()
        .filter(|point| point.metric == Metric::ChecksPassed)
        .collect();

    if points.len() < 2 {
        muted(
            ui,
            "Fewer than two weeks with checked runs: nothing to draw yet.",
        );

        return;
    }

    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 120.0), Sense::hover());
    let painter = ui.painter_at(rect);

    painter.rect_filled(rect, 6.0, palette::sunken());

    let x = |at: usize| {
        rect.left() + 10.0 + at as f32 / (points.len() - 1) as f32 * (rect.width() - 20.0)
    };
    let y = |share: f64| rect.bottom() - 18.0 - share as f32 * (rect.height() - 30.0);

    let line = |pick: &dyn Fn(usize) -> Option<f64>, colour: Color32| {
        let drawn: Vec<_> = (0..points.len())
            .filter_map(|at| Some(pos2(x(at), y(pick(at)?))))
            .collect();

        if drawn.len() > 1 {
            painter.add(egui::Shape::line(drawn.clone(), Stroke::new(2.0, colour)));
        }

        for point in drawn {
            painter.circle_filled(point, 3.0, colour);
        }
    };

    line(&|at| points[at].spoke.mean, palette::accent());
    line(&|at| points[at].held_out.mean, palette::text_muted());

    painter.text(
        pos2(rect.left() + 10.0, rect.top() + 6.0),
        Align2::LEFT_TOP,
        "checks passed · with it",
        theme::small(),
        palette::accent(),
    );

    painter.text(
        pos2(rect.right() - 10.0, rect.top() + 6.0),
        Align2::RIGHT_TOP,
        "without it",
        theme::small(),
        palette::text_muted(),
    );

    painter.text(
        pos2(x(0), rect.bottom() - 4.0),
        Align2::LEFT_BOTTOM,
        &points[0].week,
        theme::small(),
        palette::text_muted(),
    );

    painter.text(
        pos2(x(points.len() - 1), rect.bottom() - 4.0),
        Align2::RIGHT_BOTTOM,
        &points[points.len() - 1].week,
        theme::small(),
        palette::text_muted(),
    );
}

fn tile(ui: &mut Ui, name: &str, value: &str, note: &str) {
    theme::card().show(ui, |ui| {
        ui.set_width(200.0);

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
        ui.label(
            RichText::new(note)
                .font(theme::small())
                .color(palette::text_secondary()),
        );
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
    ui.add_space(16.0);
    ui.label(
        RichText::new(text.to_uppercase())
            .font(theme::small())
            .color(palette::text_muted()),
    );
    ui.add_space(4.0);
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
