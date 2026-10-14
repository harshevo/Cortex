//! Price bars read with the liquid and the dendritic network: the chart,
//! what the networks made of it, and how their forecasts did on bars
//! they were not trained on.
//!
//! Nothing here is advice. The view says what the forecast is, how such
//! forecasts did on held-out bars, and whether that is any better than
//! guessing, which on most series it is not.

use std::path::PathBuf;
use std::sync::Arc;

use cortex_liquid::{
    Analysis, Bar, Candle, Desk, Direction, Edge, EpochLoss, Predictor, Score, Settings, Stage,
    Status,
};
use eframe::egui::{
    self, Align2, Color32, Pos2, Rect, RichText, ScrollArea, Sense, Stroke, Ui, Vec2, pos2, vec2,
};

use crate::theme::{self, palette};

const PANEL_WIDTH: f32 = 340.0;

/// Screen points one candle takes at least. Bars are drawn several to a
/// candle when there are more of them than fit.
const CANDLE: f32 = 5.0;

/// Fewest and most bars the chart shows at once.
const FEWEST_SHOWN: f32 = 20.0;

/// Height of the strips under the chart.
const STRIP: f32 = 8.0;

const HORIZONS: [usize; 4] = [1, 3, 5, 10];

pub struct TradingView {
    desk: Desk,

    /// Why the last file could not be read.
    problem: Option<String>,

    horizon: usize,

    /// The bars on screen: the first, and how many, as the chart is
    /// dragged and zoomed. `None` shows all of them.
    window: Option<(f32, f32)>,

    /// What the desk was at when the chart was last fitted to it.
    fitted: u64,
}

impl Default for TradingView {
    fn default() -> Self {
        Self {
            desk: Desk::new(),
            problem: None,
            horizon: 1,
            window: None,
            fitted: 0,
        }
    }
}

impl TradingView {
    /// Reads `path` as the bars to look at.
    pub fn open(&mut self, path: &std::path::Path) {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default();

        let read = std::fs::read_to_string(path)
            .map_err(|error| format!("Cannot read {name}: {error}"))
            .and_then(|text| {
                self.desk
                    .load_csv(&name, &text)
                    .map_err(|error| format!("{name}: {error}"))
            });

        self.problem = read.err();
        self.window = None;
    }

    fn analyse(&mut self, context: &egui::Context) {
        let context = context.clone();

        let settings = Settings {
            horizon: self.horizon,
            ..Settings::default()
        };

        self.desk.analyse(settings, move || context.request_repaint());
    }
}

pub fn show(ui: &mut Ui, view: &mut TradingView) {
    // A file dropped on the window is opened.
    let dropped: Option<PathBuf> = ui.input(|input| {
        input
            .raw
            .dropped_files
            .iter()
            .map(|file| file.path().to_path_buf())
            .next()
    });

    if let Some(path) = dropped {
        view.open(&path);
    }

    let status = view.desk.status();
    let bars = view.desk.bars();
    let analysis = view.desk.analysis();

    if view.fitted != view.desk.revision() && status != Status::Empty {
        view.fitted = view.desk.revision();

        if view.window.is_none() {
            view.window = Some((0.0, bars.len() as f32));
        }
    }

    egui::Panel::right(ui.id().with("trading panel"))
        .exact_size(PANEL_WIDTH)
        .show_separator_line(false)
        .frame(
            egui::Frame::new()
                .fill(palette::sidebar())
                .inner_margin(egui::Margin::symmetric(16, 12)),
        )
        .show(ui, |ui| {
            ScrollArea::vertical()
                .auto_shrink(false)
                .show(ui, |ui| panel(ui, view, &status, analysis.as_deref()));
        });

    if bars.is_empty() {
        empty(ui, view);

        return;
    }

    chart(ui, view, &bars, analysis.as_deref());
}

/// Nothing is loaded: say what to do.
fn empty(ui: &mut Ui, view: &mut TradingView) {
    ui.add_space(ui.available_height() * 0.3);

    ui.vertical_centered(|ui| {
        ui.label(
            RichText::new("Open a file of price bars")
                .font(theme::voice(24.0))
                .color(palette::text()),
        );

        ui.add_space(6.0);

        ui.label(
            RichText::new(
                "A CSV with a time and a close column; open, high, low and volume if it has \
                 them. Drop it here or choose it.",
            )
            .color(palette::text_secondary()),
        );

        ui.add_space(14.0);

        if ui.button("Choose a file…").clicked() {
            choose(view);
        }

        if let Some(problem) = &view.problem {
            ui.add_space(12.0);
            ui.label(RichText::new(problem).color(palette::bad()));
        }
    });
}

fn choose(view: &mut TradingView) {
    let chosen = rfd::FileDialog::new()
        .add_filter("Price bars", &["csv", "tsv", "txt"])
        .pick_file();

    if let Some(path) = chosen {
        view.open(&path);
    }
}

fn panel(ui: &mut Ui, view: &mut TradingView, status: &Status, analysis: Option<&Analysis>) {
    ui.label(
        RichText::new("Trading")
            .font(theme::voice(22.0))
            .color(palette::text()),
    );

    let bars = view.desk.bars();

    if bars.is_empty() {
        muted(
            ui,
            "A liquid network and the dendritic network read the bars together. They are \
             trained on the first three quarters and scored on the last, which they never saw.",
        );

        return;
    }

    muted(
        ui,
        &format!(
            "{} · {} bars · {} to {}",
            view.desk.name(),
            bars.len(),
            day(bars[0].time_ms),
            day(bars[bars.len() - 1].time_ms)
        ),
    );

    for note in view.desk.notes() {
        muted(ui, &note);
    }

    if let Some(problem) = &view.problem {
        ui.label(RichText::new(problem).color(palette::bad()));
    }

    ui.add_space(8.0);

    let working = matches!(status, Status::Working { .. });

    ui.horizontal(|ui| {
        ui.label(RichText::new("Bars ahead").color(palette::text_secondary()));

        for horizon in HORIZONS {
            if ui
                .selectable_label(view.horizon == horizon, horizon.to_string())
                .clicked()
            {
                view.horizon = horizon;
            }
        }
    });

    ui.add_space(4.0);

    ui.horizontal(|ui| {
        let label = if analysis.is_some() {
            "Analyse again"
        } else {
            "Analyse"
        };

        if ui.add_enabled(!working, egui::Button::new(label)).clicked() {
            view.analyse(ui.ctx());
        }

        if ui
            .add_enabled(!working, egui::Button::new("Another file…"))
            .clicked()
        {
            choose(view);
        }
    });

    match status {
        Status::Working { stage, fraction } => {
            ui.add_space(8.0);
            ui.add(egui::ProgressBar::new(*fraction).text(stage_name(*stage)));
        }

        Status::Failed(reason) => {
            ui.add_space(8.0);
            ui.label(RichText::new(reason).color(palette::bad()));
        }

        _ => {}
    }

    let Some(analysis) = analysis else {
        return;
    };

    verdict(ui, analysis);
    forecast(ui, analysis);
    scores(ui, analysis);
    network(ui, analysis);
    training(ui, analysis);

    heading(ui, "To know");

    for note in &analysis.notes {
        muted(ui, note);
    }

    muted(
        ui,
        &format!(
            "Analysed in {:.0} ms on the processor: the dendritic network {:.0} ms, training the \
             liquid networks {:.0} ms.",
            analysis.timings.total_ms, analysis.timings.dendritic_ms, analysis.timings.training_ms
        ),
    );
}

fn verdict(ui: &mut Ui, analysis: &Analysis) {
    let (title, colour) = match analysis.verdict.edge {
        Edge::None => ("No edge found", palette::text_secondary()),
        Edge::Weak => ("A weak edge, perhaps", palette::warning()),
        Edge::Present => ("An edge on the held-out bars", palette::good()),
    };

    ui.add_space(10.0);

    theme::card().show(ui, |ui| {
        ui.set_width(ui.available_width());

        ui.label(RichText::new(title).font(theme::strong(15.0)).color(colour));
        ui.add_space(2.0);
        ui.label(RichText::new(&analysis.verdict.text).color(palette::text_secondary()));
    });
}

fn forecast(ui: &mut Ui, analysis: &Analysis) {
    let forecast = &analysis.forecast;

    heading(
        ui,
        &format!(
            "Forecast for the next {}",
            if forecast.horizon == 1 {
                "bar".to_string()
            } else {
                format!("{} bars", forecast.horizon)
            }
        ),
    );

    let (direction, colour) = match forecast.direction {
        Direction::Up => ("Up", palette::good()),
        Direction::Down => ("Down", palette::bad()),
        Direction::Flat => ("Flat", palette::text_secondary()),
    };

    ui.horizontal(|ui| {
        ui.label(
            RichText::new(direction)
                .font(theme::strong(18.0))
                .color(colour),
        );

        ui.label(
            RichText::new(format!(
                "{:+.2}% expected · {:.0}% sure of the direction",
                forecast.expected_return * 100.0,
                forecast.confidence * 100.0
            ))
            .color(palette::text_secondary()),
        );
    });

    if analysis.verdict.edge == Edge::None {
        muted(
            ui,
            "With no edge found, this is no better than a coin: it is shown so that you can \
             see what the networks say, not so that you act on it.",
        );
    }

    let account = &forecast.dendritic;

    row(ui, "Now", &account.now.join(", "));
    row(ui, "Regime", &account.regime);

    for expectation in &account.expectations {
        row(
            ui,
            "Expects",
            &format!(
                "{} · {:.0}%",
                expectation.label,
                expectation.confidence * 100.0
            ),
        );
    }

    for (step, imagined) in account.imagined.iter().enumerate() {
        row(
            ui,
            &format!("Imagines +{}", step + 1),
            &format!(
                "{} · {:.0}%",
                imagined.labels.join(", "),
                imagined.confidence * 100.0
            ),
        );
    }
}

/// How each predictor did on the held-out bars: a bar from 30% to 70%
/// right, its interval, and a mark at one half.
fn scores(ui: &mut Ui, analysis: &Analysis) {
    heading(ui, "Direction called right, on held-out bars");

    for score in &analysis.scores {
        score_row(ui, score, analysis.verdict.best_baseline);
    }

    muted(
        ui,
        "The line is 50%. The lighter band is the range the true share is likely in; where \
         it crosses the line, the predictor is not told apart from a coin.",
    );
}

fn score_row(ui: &mut Ui, score: &Score, best_baseline: Predictor) {
    let Some(accuracy) = score.accuracy else {
        row(ui, &score.name, "called no direction");

        return;
    };

    let learned = matches!(
        score.predictor,
        Predictor::Liquid | Predictor::Dendritic | Predictor::Combined
    );

    let colour = if learned {
        palette::accent()
    } else {
        palette::text_muted()
    };

    ui.add_space(4.0);

    ui.horizontal(|ui| {
        ui.label(RichText::new(&score.name).color(palette::text()));

        if score.predictor == best_baseline {
            ui.label(RichText::new("to beat").font(theme::small()).color(palette::warning()));
        }

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                RichText::new(format!("{:.1}% of {}", accuracy * 100.0, score.scored))
                    .font(theme::mono())
                    .color(palette::text_secondary()),
            );
        });
    });

    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 8.0), Sense::hover());
    let painter = ui.painter_at(rect);

    // From 30% to 70%: where every honest result on prices lies.
    let at = |share: f64| {
        let along = ((share - 0.3) / 0.4).clamp(0.0, 1.0) as f32;

        rect.left() + along * rect.width()
    };

    painter.rect_filled(rect, 3.0, palette::sunken());

    if let (Some(low), Some(high)) = (score.accuracy_low, score.accuracy_high) {
        painter.rect_filled(
            Rect::from_x_y_ranges(at(low)..=at(high), rect.y_range()),
            3.0,
            colour.gamma_multiply(0.35),
        );
    }

    painter.circle_filled(pos2(at(accuracy), rect.center().y), 4.0, colour);

    painter.line_segment(
        [pos2(at(0.5), rect.top() - 2.0), pos2(at(0.5), rect.bottom() + 2.0)],
        Stroke::new(1.0, palette::text_muted()),
    );
}

fn network(ui: &mut Ui, analysis: &Analysis) {
    let growth = &analysis.growth;

    heading(ui, "The dendritic network");

    row(
        ui,
        "Grew to",
        &format!(
            "{} neurons, {} branches, {} synapses",
            growth.neurons, growth.segments, growth.synapses
        ),
    );

    row(
        ui,
        "Expected",
        &format!(
            "{:.0}% of each bar (guessing the commonest: {:.0}%)",
            growth.expected_right * 100.0,
            growth.baseline_right * 100.0
        ),
    );
}

fn training(ui: &mut Ui, analysis: &Analysis) {
    heading(ui, "Training");

    curve(ui, "Liquid alone", &analysis.liquid_losses);
    curve(ui, "Liquid + dendritic", &analysis.combined_losses);

    muted(
        ui,
        "Solid: the loss on what it was trained on. Faint: on bars held back from training. \
         When the faint one rises while the solid one falls, it is memorising noise, and the \
         weights from before that are the ones kept.",
    );
}

fn curve(ui: &mut Ui, name: &str, losses: &[EpochLoss]) {
    if losses.len() < 2 {
        return;
    }

    ui.label(RichText::new(name).font(theme::small()).color(palette::text_secondary()));

    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 44.0), Sense::hover());
    let painter = ui.painter_at(rect);

    painter.rect_filled(rect, 4.0, palette::sunken());

    let values = losses.iter().flat_map(|loss| [loss.fit, loss.check]);
    let (lowest, highest) = values.fold((f32::MAX, f32::MIN), |(low, high), value| {
        (low.min(value), high.max(value))
    });

    let span = (highest - lowest).max(1e-6);

    let point = |epoch: usize, value: f32| {
        pos2(
            rect.left() + 4.0 + epoch as f32 / (losses.len() - 1) as f32 * (rect.width() - 8.0),
            rect.bottom() - 4.0 - (value - lowest) / span * (rect.height() - 8.0),
        )
    };

    let line = |pick: fn(&EpochLoss) -> f32, colour: Color32| {
        let points: Vec<Pos2> = losses
            .iter()
            .enumerate()
            .map(|(epoch, loss)| point(epoch, pick(loss)))
            .collect();

        painter.add(egui::Shape::line(points, Stroke::new(1.5, colour)));
    };

    line(|loss| loss.check, palette::accent().gamma_multiply(0.4));
    line(|loss| loss.fit, palette::accent());
}

fn chart(ui: &mut Ui, view: &mut TradingView, bars: &Arc<Vec<Bar>>, analysis: Option<&Analysis>) {
    let (response, painter) = ui.allocate_painter(ui.available_size(), Sense::click_and_drag());

    let whole = response.rect.shrink2(vec2(16.0, 14.0));
    let strips = if analysis.is_some() { 2.0 * (STRIP + 4.0) } else { 0.0 };

    let plot = Rect::from_min_max(
        whole.min,
        pos2(whole.right() - 64.0, whole.bottom() - 22.0 - strips),
    );

    if plot.width() < 40.0 || plot.height() < 40.0 {
        return;
    }

    let total = bars.len() as f32;
    let (mut first, mut shown) = view.window.unwrap_or((0.0, total));

    // Dragging moves along the series; scrolling zooms about the pointer.
    if response.dragged() {
        first -= response.drag_delta().x / plot.width() * shown;
    }

    if let Some(pointer) = response.hover_pos() {
        let scroll = ui.input(|input| input.smooth_scroll_delta.y);

        if scroll != 0.0 {
            let about = ((pointer.x - plot.left()) / plot.width()).clamp(0.0, 1.0);
            let zoomed = (shown * (1.0 - scroll * 0.004)).clamp(FEWEST_SHOWN.min(total), total);

            first += (shown - zoomed) * about;
            shown = zoomed;
        }
    }

    shown = shown.clamp(FEWEST_SHOWN.min(total), total);
    first = first.clamp(0.0, total - shown);

    view.window = Some((first, shown));

    let most = (plot.width() / CANDLE) as usize;
    let drawn = view.desk.candles(first as usize, shown.ceil() as usize, most);

    if drawn.is_empty() {
        return;
    }

    let highest = drawn.iter().map(|candle| candle.high).fold(f64::MIN, f64::max);
    let lowest = drawn.iter().map(|candle| candle.low).fold(f64::MAX, f64::min);
    let span = (highest - lowest).max(highest.abs() * 1e-6).max(1e-12);

    let x = |bar: f32| plot.left() + (bar - first) / shown * plot.width();

    let y = |price: f64| {
        plot.bottom() - ((price - lowest) / span) as f32 * (plot.height() - 12.0) - 6.0
    };

    // The bars the networks never saw in training are on a lighter ground.
    if let Some(analysis) = analysis {
        let held_out = x(analysis.train_bars as f32).clamp(plot.left(), plot.right());

        painter.rect_filled(
            Rect::from_min_max(pos2(held_out, plot.top()), plot.max),
            0.0,
            palette::surface().gamma_multiply(0.6),
        );

        if held_out > plot.left() + 2.0 && held_out < plot.right() - 60.0 {
            painter.text(
                pos2(held_out + 6.0, plot.top() + 4.0),
                Align2::LEFT_TOP,
                "held out",
                theme::small(),
                palette::text_muted(),
            );
        }
    }

    // Price lines.
    for step in 0..=4 {
        let price = lowest + span * f64::from(step) / 4.0;
        let at = y(price);

        painter.line_segment(
            [pos2(plot.left(), at), pos2(plot.right(), at)],
            Stroke::new(1.0, palette::border().gamma_multiply(0.5)),
        );

        painter.text(
            pos2(plot.right() + 8.0, at),
            Align2::LEFT_CENTER,
            price_text(price),
            theme::small(),
            palette::text_muted(),
        );
    }

    let width = (plot.width() / drawn.len() as f32 * 0.7).clamp(1.0, 14.0);

    for candle in &drawn {
        let middle = x(candle.first as f32 + candle.count as f32 / 2.0);

        let colour = if candle.close >= candle.open {
            palette::good()
        } else {
            palette::bad()
        };

        painter.line_segment(
            [pos2(middle, y(candle.high)), pos2(middle, y(candle.low))],
            Stroke::new(1.0, colour),
        );

        let (top, bottom) = (y(candle.open.max(candle.close)), y(candle.open.min(candle.close)));

        painter.rect_filled(
            Rect::from_min_max(
                pos2(middle - width / 2.0, top),
                pos2(middle + width / 2.0, bottom.max(top + 1.0)),
            ),
            1.0,
            colour,
        );
    }

    // Dates along the bottom.
    for step in 0..=4 {
        let bar = (first + shown * step as f32 / 4.0).min(total - 1.0) as usize;

        painter.text(
            pos2(x(bar as f32), plot.bottom() + 4.0),
            if step == 4 { Align2::RIGHT_TOP } else { Align2::LEFT_TOP },
            day(bars[bar].time_ms),
            theme::small(),
            palette::text_muted(),
        );
    }

    if analysis.is_some() {
        let top = plot.bottom() + 22.0;

        strip(
            &painter,
            Rect::from_min_size(pos2(plot.left(), top), vec2(plot.width(), STRIP)),
            &drawn,
            |candle| {
                candle
                    .right
                    .map(|share| mix(palette::bad(), palette::good(), share))
            },
        );

        strip(
            &painter,
            Rect::from_min_size(
                pos2(plot.left(), top + STRIP + 4.0),
                vec2(plot.width(), STRIP),
            ),
            &drawn,
            |candle| candle.regime.map(regime_colour),
        );

        painter.text(
            pos2(plot.right() + 8.0, top + STRIP / 2.0),
            Align2::LEFT_CENTER,
            "right",
            theme::small(),
            palette::text_muted(),
        );

        painter.text(
            pos2(plot.right() + 8.0, top + STRIP * 1.5 + 4.0),
            Align2::LEFT_CENTER,
            "regime",
            theme::small(),
            palette::text_muted(),
        );
    }

    pointed_at(ui, &response, &painter, plot, bars, analysis, first, shown);
}

/// One coloured cell under each candle, where there is something to say.
fn strip(
    painter: &egui::Painter,
    rect: Rect,
    drawn: &[Candle],
    colour: impl Fn(&Candle) -> Option<Color32>,
) {
    painter.rect_filled(rect, 2.0, palette::sunken());

    let each = rect.width() / drawn.len() as f32;

    for (at, candle) in drawn.iter().enumerate() {
        if let Some(colour) = colour(candle) {
            painter.rect_filled(
                Rect::from_min_size(
                    pos2(rect.left() + at as f32 * each, rect.top()),
                    vec2(each.max(1.0), rect.height()),
                ),
                0.0,
                colour,
            );
        }
    }
}

fn regime_colour(regime: u8) -> Color32 {
    let colours = [
        palette::model(),
        palette::accent(),
        palette::warning(),
        palette::good(),
        palette::bad(),
        palette::text_muted(),
    ];

    colours[usize::from(regime) % colours.len()].gamma_multiply(0.8)
}

fn mix(from: Color32, to: Color32, share: f32) -> Color32 {
    let part = |from: u8, to: u8| (f32::from(from) + (f32::from(to) - f32::from(from)) * share) as u8;

    Color32::from_rgb(
        part(from.r(), to.r()),
        part(from.g(), to.g()),
        part(from.b(), to.b()),
    )
}

/// The bar under the pointer, spelled out.
#[allow(clippy::too_many_arguments)]
fn pointed_at(
    ui: &Ui,
    response: &egui::Response,
    painter: &egui::Painter,
    plot: Rect,
    bars: &[Bar],
    analysis: Option<&Analysis>,
    first: f32,
    shown: f32,
) {
    let Some(pointer) = response.hover_pos().filter(|pointer| plot.contains(*pointer)) else {
        return;
    };

    let at = (first + (pointer.x - plot.left()) / plot.width() * shown) as usize;

    let Some(bar) = bars.get(at) else {
        return;
    };

    painter.line_segment(
        [pos2(pointer.x, plot.top()), pos2(pointer.x, plot.bottom())],
        Stroke::new(1.0, palette::text_muted().gamma_multiply(0.6)),
    );

    let mut lines = vec![
        day(bar.time_ms),
        format!(
            "open {}  high {}  low {}  close {}",
            price_text(bar.open),
            price_text(bar.high),
            price_text(bar.low),
            price_text(bar.close)
        ),
    ];

    if let Some(analysis) = analysis {
        if let Some(name) = analysis
            .regimes
            .get(at)
            .and_then(|regime| analysis.regime_names.get(usize::from(*regime)))
        {
            lines.push(format!("regime: {name}"));
        }

        let series = &analysis.series;

        if let Some(up) = at
            .checked_sub(series.first_bar)
            .and_then(|at| series.probability_up.get(at))
        {
            lines.push(format!("the model put {:.0}% on up from here", up * 100.0));
        }
    }

    egui::Tooltip::always_open(
        ui.ctx().clone(),
        ui.layer_id(),
        ui.id().with("trading bar"),
        egui::PopupAnchor::Pointer,
    )
    .show(|ui| {
        for line in &lines {
            ui.label(RichText::new(line).font(theme::small()));
        }
    });
}

fn stage_name(stage: Stage) -> &'static str {
    match stage {
        Stage::Features => "Describing the bars",
        Stage::Dendritic => "The dendritic network is reading them",
        Stage::Liquid => "Training the liquid network",
        Stage::Combined => "Training the two together",
        Stage::Scoring => "Scoring on held-out bars",
        Stage::Done => "Done",
    }
}

/// A price with as many decimals as its size calls for.
fn price_text(price: f64) -> String {
    let size = price.abs();

    if size >= 1_000.0 {
        format!("{price:.0}")
    } else if size >= 10.0 {
        format!("{price:.2}")
    } else if size >= 0.1 {
        format!("{price:.4}")
    } else {
        format!("{price:.6}")
    }
}

/// The day a time falls on, in UTC, as `2024-03-09`.
fn day(time_ms: i64) -> String {
    let days = time_ms.div_euclid(86_400_000);

    // Days since 1970 to a date in the Gregorian calendar, by eras of
    // 400 years (Howard Hinnant's method).
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 { shifted_month + 3 } else { shifted_month - 9 };
    let year = year_of_era + era * 400 + i64::from(month <= 2);

    format!("{year:04}-{month:02}-{day:02}")
}

fn heading(ui: &mut Ui, text: &str) {
    ui.add_space(14.0);
    ui.label(
        RichText::new(text.to_uppercase())
            .font(theme::small())
            .color(palette::text_muted()),
    );
    ui.add_space(2.0);
}

fn muted(ui: &mut Ui, text: &str) {
    ui.label(
        RichText::new(text)
            .font(theme::small())
            .color(palette::text_muted()),
    );
}

fn row(ui: &mut Ui, name: &str, value: &str) {
    ui.horizontal_top(|ui| {
        ui.allocate_ui_with_layout(
            Vec2::new(92.0, 0.0),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                ui.label(RichText::new(name).color(palette::text_secondary()));
            },
        );

        ui.add(egui::Label::new(RichText::new(value).color(palette::text())).wrap());
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_time_is_shown_as_its_day() {
        assert_eq!(day(0), "1970-01-01");
        assert_eq!(day(951_782_400_000), "2000-02-29");
        assert_eq!(day(1_709_942_400_000), "2024-03-09");
        assert_eq!(day(-86_400_000), "1969-12-31");
    }

    #[test]
    fn prices_keep_the_decimals_their_size_needs() {
        assert_eq!(price_text(43_210.4), "43210");
        assert_eq!(price_text(182.356), "182.36");
        assert_eq!(price_text(1.08234), "1.0823");
        assert_eq!(price_text(0.00012345), "0.000123");
    }

    #[test]
    fn a_file_that_cannot_be_read_says_why_and_keeps_what_was_loaded() {
        let folder = tempfile::tempdir().unwrap();
        let good = folder.path().join("good.csv");
        let bad = folder.path().join("bad.csv");

        std::fs::write(&good, "date,close\n2024-01-01,10\n2024-01-02,11\n").unwrap();
        std::fs::write(&bad, "name,colour\nx,red\n").unwrap();

        let mut view = TradingView::default();

        view.open(&good);

        assert_eq!(view.problem, None);
        assert_eq!(view.desk.bars().len(), 2);

        view.open(&bad);

        assert!(view.problem.as_deref().unwrap().starts_with("bad.csv"));
        assert_eq!(view.desk.bars().len(), 2);
    }
}
