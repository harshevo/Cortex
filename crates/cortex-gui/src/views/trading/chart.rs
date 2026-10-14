//! The candle chart: closed candles, the one forming, the next ten as
//! forecast with their 80% band, VWAP and two moving averages, support
//! and resistance, volume, and RSI beneath. Dragged to move along,
//! scrolled to zoom; it follows the latest candle until moved away.

use cortex_liquid::Bar;
use cortex_market::{Snapshot, clock};
use eframe::egui::{
    self, Align2, Color32, CornerRadius, Pos2, Rect, Sense, Shape, Stroke, Ui, pos2, vec2,
};

use super::format;
use crate::theme::{self, palette};

const FEWEST_SHOWN: f32 = 20.0;
const DEFAULT_SHOWN: f32 = 110.0;
const AXIS: f32 = 74.0;
const RSI_HEIGHT: f32 = 70.0;
const TIME_HEIGHT: f32 = 20.0;

pub struct ChartState {
    /// The first slot shown, and how many.
    window: Option<(f32, f32)>,

    /// Keeps the latest candle in view as new ones come.
    pinned: bool,
}

impl Default for ChartState {
    fn default() -> Self {
        Self {
            window: None,
            pinned: true,
        }
    }
}

/// What occupies one slot along the chart.
#[derive(Clone, Copy)]
enum Slot {
    Closed(Bar),
    Forming(Bar),
    Ahead,
}

pub fn show(ui: &mut Ui, state: &mut ChartState, snapshot: &Snapshot) {
    let (response, painter) = ui.allocate_painter(ui.available_size(), Sense::click_and_drag());
    let whole = response.rect.shrink2(vec2(12.0, 10.0));

    let closed = snapshot.bars.as_slice();

    if closed.is_empty() {
        painter.text(
            whole.center(),
            Align2::CENTER_CENTER,
            "Waiting for candles",
            theme::ui(),
            palette::text_muted(),
        );

        return;
    }

    let forecast = &snapshot.rows;

    // Slots: every closed candle, then the forecast; the first forecast
    // candle is the one now forming.
    let total = (closed.len() + forecast.len().max(usize::from(snapshot.forming.is_some()))) as f32;

    let slot = |at: usize| -> Slot {
        if at < closed.len() {
            Slot::Closed(closed[at])
        } else if at == closed.len()
            && let Some(forming) = snapshot.forming
        {
            Slot::Forming(forming)
        } else {
            Slot::Ahead
        }
    };

    let price_area = Rect::from_min_max(
        whole.min,
        pos2(
            whole.right() - AXIS,
            whole.bottom() - RSI_HEIGHT - TIME_HEIGHT - 8.0,
        ),
    );
    let rsi_area = Rect::from_min_max(
        pos2(whole.left(), price_area.bottom() + 8.0),
        pos2(price_area.right(), price_area.bottom() + 8.0 + RSI_HEIGHT),
    );

    if price_area.width() < 80.0 || price_area.height() < 80.0 {
        return;
    }

    let (mut first, mut shown) = state
        .window
        .unwrap_or((total - DEFAULT_SHOWN.min(total), DEFAULT_SHOWN.min(total)));

    if state.pinned {
        first = total - shown;
    }

    if response.dragged() {
        first -= response.drag_delta().x / price_area.width() * shown;
    }

    if let Some(pointer) = response.hover_pos() {
        let (scroll, zoom) = ui.input(|input| (input.smooth_scroll_delta, input.zoom_delta()));
        let factor = if zoom != 1.0 {
            1.0 / zoom
        } else {
            1.0 - scroll.y * 0.003
        };

        if factor != 1.0 {
            let about = ((pointer.x - price_area.left()) / price_area.width()).clamp(0.0, 1.0);
            let zoomed = (shown * factor).clamp(FEWEST_SHOWN.min(total), total);

            first += (shown - zoomed) * about;
            shown = zoomed;
        }

        if scroll.x != 0.0 {
            first -= scroll.x / price_area.width() * shown;
        }
    }

    shown = shown.clamp(FEWEST_SHOWN.min(total), total);
    first = first.clamp(0.0, (total - shown).max(0.0));
    state.pinned = first + shown >= total - 0.5;
    state.window = Some((first, shown));

    let begin = first.floor() as usize;
    let end = ((first + shown).ceil() as usize).min(total as usize);

    let x = |at: f32| price_area.left() + (at - first + 0.5) / shown * price_area.width();
    let step = price_area.width() / shown;

    // The price range of what is in view.
    let mut highest = f64::MIN;
    let mut lowest = f64::MAX;

    for at in begin..end {
        match slot(at) {
            Slot::Closed(bar) | Slot::Forming(bar) => {
                highest = highest.max(bar.high);
                lowest = lowest.min(bar.low);
            }
            Slot::Ahead => {}
        }

        if let Some(row) = at
            .checked_sub(closed.len())
            .and_then(|ahead| forecast.get(ahead))
        {
            highest = highest.max(row.high).max(row.close_high);
            lowest = lowest.min(row.low).min(row.close_low);
        }
    }

    if highest <= lowest {
        highest = lowest + 1.0;
    }

    let pad = (highest - lowest) * 0.06;

    highest += pad;
    lowest -= pad;

    let span = highest - lowest;
    let y =
        |price: f64| price_area.bottom() - ((price - lowest) / span) as f32 * price_area.height();

    // The ground, and price lines.
    painter.rect_filled(
        price_area,
        CornerRadius::same(theme::radius::M),
        palette::sunken().gamma_multiply(0.35),
    );

    // Prices marked on the axis by name: the levels and the latest. Plain
    // price lines give way to them where they would overlap.
    let mut marked: Vec<f32> = Vec::new();

    if let Some(plan) = &snapshot.plan {
        marked.extend([y(plan.support), y(plan.resistance)]);
    }

    if let Some(bar) = snapshot.forming.or(closed.last().copied()) {
        marked.push(y(bar.close));
    }

    let tick = nice_step(span / 5.0);
    let mut level = (lowest / tick).ceil() * tick;

    while level < highest {
        let at = y(level);

        painter.line_segment(
            [pos2(price_area.left(), at), pos2(price_area.right(), at)],
            Stroke::new(1.0, palette::border().gamma_multiply(0.45)),
        );

        if marked.iter().all(|mark| (mark - at).abs() > 14.0) {
            painter.text(
                pos2(price_area.right() + 8.0, at),
                Align2::LEFT_CENTER,
                format::price(level),
                theme::small(),
                palette::text_muted(),
            );
        }

        level += tick;
    }

    // The forecast's side.
    let ahead_left = x(closed.len() as f32 - 0.5).max(price_area.left());

    if ahead_left < price_area.right() {
        painter.rect_filled(
            Rect::from_min_max(
                pos2(ahead_left, price_area.top()),
                pos2(price_area.right(), rsi_area.bottom()),
            ),
            0.0,
            palette::accent().gamma_multiply(0.05),
        );

        painter.text(
            pos2(ahead_left + 6.0, price_area.top() + 6.0),
            Align2::LEFT_TOP,
            format!("{} forecast", snapshot.brain.short()),
            theme::small(),
            palette::accent(),
        );
    }

    // Support and resistance.
    let plan_levels = snapshot.plan.as_ref().map(|plan| {
        [
            (plan.support, palette::good()),
            (plan.resistance, palette::bad()),
        ]
    });

    if let Some(levels) = plan_levels {
        for (level, colour) in levels {
            if level <= lowest || level >= highest {
                continue;
            }

            let at = y(level);

            painter.add(Shape::dashed_line(
                &[pos2(price_area.left(), at), pos2(price_area.right(), at)],
                Stroke::new(1.0, colour.gamma_multiply(0.7)),
                6.0,
                5.0,
            ));

            label(
                &painter,
                pos2(price_area.right() + 4.0, at),
                &format::price(level),
                colour.gamma_multiply(0.85),
            );
        }
    }

    // Volume, in the lowest fifth of the price area.
    let volume_top = price_area.bottom() - price_area.height() * 0.18;
    let loudest = (begin..end)
        .filter_map(|at| match slot(at) {
            Slot::Closed(bar) | Slot::Forming(bar) => Some(bar.volume),
            Slot::Ahead => None,
        })
        .chain(forecast.iter().map(|row| row.volume))
        .fold(0.0f64, f64::max);

    let body_width = (step * 0.68).clamp(1.0, 18.0);

    if loudest > 0.0 {
        for at in begin..end {
            let (volume, colour) = match slot(at) {
                Slot::Closed(bar) | Slot::Forming(bar) => (
                    bar.volume,
                    if bar.close >= bar.open {
                        palette::good()
                    } else {
                        palette::bad()
                    },
                ),
                Slot::Ahead => match forecast.get(at - closed.len()) {
                    Some(row) => (row.volume, palette::accent()),
                    None => continue,
                },
            };

            let height = (volume / loudest) as f32 * (price_area.bottom() - volume_top);
            let middle = x(at as f32);

            painter.rect_filled(
                Rect::from_min_max(
                    pos2(middle - body_width / 2.0, price_area.bottom() - height),
                    pos2(middle + body_width / 2.0, price_area.bottom()),
                ),
                0.0,
                colour.gamma_multiply(0.22),
            );
        }
    }

    // The forecast's 80% band, as trapezoids between candles.
    for ahead in 0..forecast.len().saturating_sub(1) {
        let (one, two) = (&forecast[ahead], &forecast[ahead + 1]);
        let (left, right) = (
            x((closed.len() + ahead) as f32),
            x((closed.len() + ahead + 1) as f32),
        );

        painter.add(Shape::convex_polygon(
            vec![
                pos2(left, y(one.close_high)),
                pos2(right, y(two.close_high)),
                pos2(right, y(two.close_low)),
                pos2(left, y(one.close_low)),
            ],
            palette::accent().gamma_multiply(0.10),
            Stroke::NONE,
        ));
    }

    // Indicator lines over the closed candles.
    let line = |values: &[Option<f64>], colour: Color32| {
        let points: Vec<Pos2> = (begin..end.min(closed.len()))
            .filter_map(|at| {
                values
                    .get(at)
                    .copied()
                    .flatten()
                    .map(|value| pos2(x(at as f32), y(value)))
            })
            .collect();

        if points.len() > 1 {
            painter.add(Shape::line(points, Stroke::new(1.3, colour)));
        }
    };

    let indicators = &snapshot.indicators;

    line(&indicators.ema_slow, palette::model().gamma_multiply(0.75));
    line(
        &indicators.ema_fast,
        palette::text_secondary().gamma_multiply(0.75),
    );
    line(&indicators.vwap, palette::warning());

    // Candles.
    for at in begin..end {
        let middle = x(at as f32);

        match slot(at) {
            Slot::Closed(bar) => candle(&painter, middle, body_width, &y, bar, false),
            Slot::Forming(bar) => candle(&painter, middle, body_width, &y, bar, true),
            Slot::Ahead => {}
        }

        if let Some(row) = at
            .checked_sub(closed.len())
            .and_then(|ahead| forecast.get(ahead))
        {
            let colour = palette::accent();
            let stroke = Stroke::new(1.2, colour);

            painter.line_segment(
                [pos2(middle, y(row.high)), pos2(middle, y(row.low))],
                stroke,
            );

            let (top, bottom) = (y(row.open.max(row.close)), y(row.open.min(row.close)));

            painter.rect(
                Rect::from_min_max(
                    pos2(middle - body_width / 2.0, top),
                    pos2(middle + body_width / 2.0, bottom.max(top + 1.0)),
                ),
                CornerRadius::same(1),
                if row.close >= row.open {
                    colour.gamma_multiply(0.18)
                } else {
                    colour.gamma_multiply(0.55)
                },
                stroke,
                egui::StrokeKind::Middle,
            );
        }
    }

    // The forecast closes joined up, from the last close.
    if let Some(last) = closed.last()
        && !forecast.is_empty()
    {
        let mut points = vec![pos2(x(closed.len() as f32 - 1.0), y(last.close))];

        points.extend(
            forecast
                .iter()
                .enumerate()
                .map(|(ahead, row)| pos2(x((closed.len() + ahead) as f32), y(row.close))),
        );

        painter.add(Shape::dashed_line(
            &points,
            Stroke::new(1.2, palette::accent()),
            4.0,
            3.0,
        ));
    }

    // The latest price, on the axis.
    let latest = snapshot.forming.or(closed.last().copied());

    if let Some(bar) = latest
        && bar.close > lowest
        && bar.close < highest
    {
        let colour = if bar.close >= bar.open {
            palette::good()
        } else {
            palette::bad()
        };
        let at = y(bar.close);

        painter.add(Shape::dashed_line(
            &[pos2(price_area.left(), at), pos2(price_area.right(), at)],
            Stroke::new(1.0, colour.gamma_multiply(0.5)),
            2.0,
            3.0,
        ));

        pill(
            &painter,
            pos2(price_area.right() + 4.0, at),
            &format::price(bar.close),
            colour,
        );
    }

    legend(&painter, price_area);

    // RSI.
    painter.rect_filled(
        rsi_area,
        CornerRadius::same(theme::radius::M),
        palette::sunken().gamma_multiply(0.35),
    );

    let rsi_y = |value: f64| rsi_area.bottom() - (value / 100.0) as f32 * rsi_area.height();

    for (level, text) in [(70.0, "70"), (30.0, "30")] {
        painter.add(Shape::dashed_line(
            &[
                pos2(rsi_area.left(), rsi_y(level)),
                pos2(rsi_area.right(), rsi_y(level)),
            ],
            Stroke::new(1.0, palette::border()),
            4.0,
            4.0,
        ));

        painter.text(
            pos2(rsi_area.right() + 8.0, rsi_y(level)),
            Align2::LEFT_CENTER,
            text,
            theme::small(),
            palette::text_muted(),
        );
    }

    painter.text(
        rsi_area.left_top() + vec2(6.0, 4.0),
        Align2::LEFT_TOP,
        "RSI 14",
        theme::small(),
        palette::text_muted(),
    );

    let rsi_points: Vec<Pos2> = (begin..end.min(closed.len()))
        .filter_map(|at| {
            indicators
                .rsi
                .get(at)
                .copied()
                .flatten()
                .map(|value| pos2(x(at as f32), rsi_y(value)))
        })
        .collect();

    if rsi_points.len() > 1 {
        painter.add(Shape::line(
            rsi_points.clone(),
            Stroke::new(1.3, palette::model()),
        ));
    }

    let ahead_rsi: Vec<Pos2> = rsi_points
        .last()
        .copied()
        .into_iter()
        .chain(forecast.iter().enumerate().filter_map(|(ahead, row)| {
            let at = closed.len() + ahead;

            (at >= begin && at < end).then_some(())?;

            row.rsi.map(|value| pos2(x(at as f32), rsi_y(value)))
        }))
        .collect();

    if ahead_rsi.len() > 1 {
        painter.add(Shape::dashed_line(
            &ahead_rsi,
            Stroke::new(1.2, palette::accent()),
            4.0,
            3.0,
        ));
    }

    // Times along the bottom.
    let intraday = snapshot.interval.is_intraday();
    let time_of = |at: usize| -> Option<i64> {
        match slot(at) {
            Slot::Closed(bar) | Slot::Forming(bar) => Some(bar.time_ms),
            Slot::Ahead => forecast.get(at - closed.len()).map(|row| row.time_ms),
        }
    };

    let labels = ((price_area.width() / 140.0) as usize).max(2);
    let mut right_of_last = f32::MIN;

    for index in 0..labels {
        let at = (first + shown * (index as f32 + 0.5) / labels as f32).min(total - 1.0) as usize;

        if let Some(time) = time_of(at) {
            let text = if intraday {
                clock::moment_text(time)
            } else {
                clock::day_text(time)
            };

            let galley = painter.layout_no_wrap(text, theme::small(), palette::text_muted());
            let left = (x(at as f32) - galley.size().x / 2.0)
                .clamp(price_area.left(), price_area.right() - galley.size().x);

            if left < right_of_last + 12.0 {
                continue;
            }

            right_of_last = left + galley.size().x;
            painter.galley(
                pos2(left, rsi_area.bottom() + 4.0),
                galley,
                palette::text_muted(),
            );
        }
    }

    // The candle under the pointer.
    let Some(pointer) = response.hover_pos().filter(|pointer| {
        pointer.x >= price_area.left()
            && pointer.x <= price_area.right()
            && pointer.y <= rsi_area.bottom()
    }) else {
        return;
    };

    let at =
        (first + (pointer.x - price_area.left()) / price_area.width() * shown).floor() as usize;

    if at >= total as usize {
        return;
    }

    painter.line_segment(
        [
            pos2(x(at as f32), price_area.top()),
            pos2(x(at as f32), rsi_area.bottom()),
        ],
        Stroke::new(1.0, palette::text_muted().gamma_multiply(0.5)),
    );

    if pointer.y < price_area.bottom() {
        let price =
            lowest + f64::from((price_area.bottom() - pointer.y) / price_area.height()) * span;

        painter.line_segment(
            [
                pos2(price_area.left(), pointer.y),
                pos2(price_area.right(), pointer.y),
            ],
            Stroke::new(1.0, palette::text_muted().gamma_multiply(0.35)),
        );

        pill(
            &painter,
            pos2(price_area.right() + 4.0, pointer.y),
            &format::price(price),
            palette::text_muted(),
        );
    }

    let mut lines: Vec<(String, Color32)> = Vec::new();

    let mut describe = |bar: &Bar, title: String| {
        lines.push((title, palette::text()));
        lines.push((
            format!(
                "O {}  H {}  L {}  C {}",
                format::price(bar.open),
                format::price(bar.high),
                format::price(bar.low),
                format::price(bar.close)
            ),
            palette::text_secondary(),
        ));

        if bar.volume > 0.0 {
            lines.push((
                format!("Volume {}", format::quantity(bar.volume)),
                palette::text_secondary(),
            ));
        }
    };

    match slot(at) {
        Slot::Closed(bar) => {
            describe(&bar, moment(bar.time_ms, intraday));

            let rsi = indicators.rsi.get(at).copied().flatten();
            let vwap = indicators.vwap.get(at).copied().flatten();

            lines.push((
                format!(
                    "RSI {}  VWAP {}",
                    format::maybe(rsi, |value| format!("{value:.1}")),
                    format::maybe(vwap, format::price)
                ),
                palette::text_muted(),
            ));
        }
        Slot::Forming(bar) => {
            describe(&bar, format!("{} · forming", moment(bar.time_ms, intraday)))
        }
        Slot::Ahead => {}
    }

    if let Some(row) = at
        .checked_sub(closed.len())
        .and_then(|ahead| forecast.get(ahead))
    {
        lines.push((
            format!(
                "{} forecast +{} · {}",
                snapshot.brain.short(),
                row.step,
                moment(row.time_ms, intraday)
            ),
            palette::accent(),
        ));
        lines.push((
            format!(
                "O {}  H {}  L {}  C {}",
                format::price(row.open),
                format::price(row.high),
                format::price(row.low),
                format::price(row.close)
            ),
            palette::text_secondary(),
        ));
        lines.push((
            format!(
                "80% {} – {}  ·  {}",
                format::price(row.close_low),
                format::price(row.close_high),
                format::percent(row.change_pct)
            ),
            palette::text_muted(),
        ));
    }

    egui::Tooltip::always_open(
        ui.ctx().clone(),
        ui.layer_id(),
        ui.id().with("trading candle"),
        egui::PopupAnchor::Pointer,
    )
    .show(|ui| {
        for (line, colour) in &lines {
            ui.label(
                egui::RichText::new(line)
                    .font(theme::small())
                    .color(*colour),
            );
        }
    });
}

fn moment(time_ms: i64, intraday: bool) -> String {
    if intraday {
        clock::moment_text(time_ms)
    } else {
        format!(
            "{} {}",
            clock::day_text(time_ms),
            &clock::date_text(time_ms)[..4]
        )
    }
}

fn candle(
    painter: &egui::Painter,
    middle: f32,
    width: f32,
    y: &impl Fn(f64) -> f32,
    bar: Bar,
    forming: bool,
) {
    let colour = if bar.close >= bar.open {
        palette::good()
    } else {
        palette::bad()
    };
    let colour = if forming {
        colour.gamma_multiply(0.75)
    } else {
        colour
    };

    painter.line_segment(
        [pos2(middle, y(bar.high)), pos2(middle, y(bar.low))],
        Stroke::new(1.0, colour),
    );

    let (top, bottom) = (y(bar.open.max(bar.close)), y(bar.open.min(bar.close)));
    let body = Rect::from_min_max(
        pos2(middle - width / 2.0, top),
        pos2(middle + width / 2.0, bottom.max(top + 1.0)),
    );

    if forming {
        painter.rect(
            body,
            CornerRadius::same(1),
            colour.gamma_multiply(0.35),
            Stroke::new(1.0, colour),
            egui::StrokeKind::Middle,
        );
    } else {
        painter.rect_filled(body, CornerRadius::same(1), colour);
    }
}

/// A price on the axis, on a coloured ground.
fn pill(painter: &egui::Painter, left_centre: Pos2, text: &str, colour: Color32) {
    let galley = painter.layout_no_wrap(text.to_string(), theme::small(), palette::background());
    let rect = Rect::from_min_size(
        pos2(left_centre.x, left_centre.y - galley.size().y / 2.0 - 2.0),
        vec2(galley.size().x + 10.0, galley.size().y + 4.0),
    );

    painter.rect_filled(rect, CornerRadius::same(4), colour);
    painter.galley(rect.min + vec2(5.0, 2.0), galley, palette::background());
}

fn label(painter: &egui::Painter, left_centre: Pos2, text: &str, colour: Color32) {
    painter.text(
        left_centre + vec2(4.0, 0.0),
        Align2::LEFT_CENTER,
        text,
        theme::small(),
        colour,
    );
}

fn legend(painter: &egui::Painter, area: Rect) {
    let mut left = area.left() + 10.0;
    let top = area.top() + 10.0;

    for (name, colour) in [
        ("VWAP", palette::warning()),
        ("EMA 9", palette::text_secondary()),
        ("EMA 21", palette::model()),
    ] {
        painter.line_segment(
            [pos2(left, top + 6.0), pos2(left + 12.0, top + 6.0)],
            Stroke::new(2.0, colour),
        );

        let rect = painter.text(
            pos2(left + 16.0, top),
            Align2::LEFT_TOP,
            name,
            theme::small(),
            palette::text_muted(),
        );

        left = rect.right() + 12.0;
    }
}

/// A round step for price lines: 1, 2 or 5 times a power of ten.
fn nice_step(rough: f64) -> f64 {
    if rough <= 0.0 || !rough.is_finite() {
        return 1.0;
    }

    let power = 10f64.powf(rough.log10().floor());
    let fraction = rough / power;

    let nice = if fraction < 1.5 {
        1.0
    } else if fraction < 3.5 {
        2.0
    } else if fraction < 7.5 {
        5.0
    } else {
        10.0
    };

    nice * power
}

#[cfg(test)]
mod tests {
    use super::nice_step;

    #[test]
    fn price_lines_fall_on_round_numbers() {
        assert_eq!(nice_step(3.7), 5.0);
        assert_eq!(nice_step(0.012), 0.01);
        assert_eq!(nice_step(260.0), 200.0);
    }
}
