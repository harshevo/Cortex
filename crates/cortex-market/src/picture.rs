//! Candles read off a picture of a candlestick chart, such as a
//! screenshot from a broker's app.
//!
//! Green and red pixels are found and joined into shapes. A shape is a
//! candle: its full height is the wick, the run of colour in its edge
//! columns is the body, green opens at the bottom of the body and red at
//! the top. Shapes that stand on one common line near the bottom are the
//! volume bars, and their heights are the volumes.
//!
//! What it cannot do. It does not read the price axis, so prices are on
//! a made-up scale unless the highest and lowest prices on the chart are
//! given. Hollow candles, candles in other colours, and charts with lines
//! drawn over the candles in green or red are read poorly or not at all.
//! The time of each candle is not known either; they are taken as evenly
//! spaced.

use cortex_liquid::Bar;
use image::RgbImage;
use serde::{Deserialize, Serialize};

/// The longest side a picture is worked on at.
const LONGEST: u32 = 2_400;

/// Fewest candles worth calling a chart.
const FEWEST: usize = 8;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChartRead {
    pub bars: Vec<Bar>,

    /// Whether the prices are real: the highest and lowest were given.
    pub priced: bool,

    /// Whether volume bars were found.
    pub volumes: bool,

    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Colour {
    Green,
    Red,
}

fn colour(pixel: [u8; 3]) -> Option<Colour> {
    let [r, g, b] = pixel.map(i32::from);

    if g > r + 40 && g + 25 > b && g > 80 {
        Some(Colour::Green)
    } else if r > g + 45 && r > b + 30 && r > 100 {
        Some(Colour::Red)
    } else {
        None
    }
}

/// One joined shape of one colour.
#[derive(Debug, Clone)]
struct Shape {
    colour: Colour,
    left: u32,
    right: u32,
    top: u32,
    bottom: u32,
    pixels: u32,

    /// For each column from `left`, the topmost and bottommost pixel.
    columns: Vec<(u32, u32)>,
}

/// Reads the candles in `bytes` (PNG, JPEG or WebP). With `prices`, the
/// highest high and the lowest low on the chart, prices come out real;
/// without, the chart is put on a scale from 100 to 110. Candles are
/// `interval_ms` apart, the last ending at `last_ms`.
pub fn read_chart(
    bytes: &[u8],
    prices: Option<(f64, f64)>,
    interval_ms: i64,
    last_ms: i64,
) -> Result<ChartRead, String> {
    let picture = image::load_from_memory(bytes)
        .map_err(|error| format!("the picture cannot be opened: {error}"))?;

    let picture = if picture.width().max(picture.height()) > LONGEST {
        picture.thumbnail(LONGEST, LONGEST)
    } else {
        picture
    };

    read_rgb(&picture.to_rgb8(), prices, interval_ms, last_ms)
}

fn read_rgb(
    picture: &RgbImage,
    prices: Option<(f64, f64)>,
    interval_ms: i64,
    last_ms: i64,
) -> Result<ChartRead, String> {
    let (width, height) = picture.dimensions();

    let mut shapes = shapes(picture);

    // Specks, and the dashes of horizontal lines: a candle always has some
    // height, if only its wick.
    shapes.retain(|shape| {
        let (width, height) = (shape.right - shape.left + 1, shape.bottom - shape.top + 1);

        shape.pixels >= 4 && height >= 3 && !(height <= 4 && width > height * 2)
    });

    // Letters and digits in green or red: short shapes that stand side by
    // side on one line, tops and bottoms level.
    let text: Vec<bool> = shapes
        .iter()
        .map(|shape| {
            let height = shape.bottom - shape.top + 1;
            let reach = (shape.right - shape.left + 1) * 3 + 6;

            height <= 24
                && shapes
                    .iter()
                    .filter(|other| {
                        !std::ptr::eq(*other, shape)
                            && other.top.abs_diff(shape.top) <= 1
                            && other.bottom.abs_diff(shape.bottom) <= 2
                            && (other.left + other.right).abs_diff(shape.left + shape.right) / 2
                                <= reach
                    })
                    .count()
                    >= 2
        })
        .collect();

    let mut at = 0;

    shapes.retain(|_| {
        at += 1;
        !text[at - 1]
    });

    if shapes.len() < FEWEST {
        return Err(format!(
            "found {} candles; a chart with green and red candles is needed",
            shapes.len()
        ));
    }

    // Wider than any candle could be: a banner or a button.
    let mut widths: Vec<u32> = shapes
        .iter()
        .map(|shape| shape.right - shape.left + 1)
        .collect();

    widths.sort_unstable();

    let usual_width = widths[widths.len() / 2];

    shapes.retain(|shape| shape.right - shape.left + 1 <= usual_width * 4 + 2);

    // Volume bars stand on one line: the commonest bottom, if many share
    // it and it is in the lower part of the picture.
    let mut bottoms: std::collections::HashMap<u32, usize> = std::collections::HashMap::new();

    for shape in &shapes {
        for near in shape.bottom.saturating_sub(1)..=shape.bottom + 1 {
            *bottoms.entry(near).or_default() += 1;
        }
    }

    let floor = bottoms
        .into_iter()
        .max_by_key(|(bottom, count)| (*count, *bottom))
        .filter(|(bottom, count)| *count * 4 >= shapes.len() && *bottom > height / 2)
        .map(|(bottom, _)| bottom);

    let (volume_shapes, mut candles): (Vec<Shape>, Vec<Shape>) = match floor {
        Some(floor) => shapes.into_iter().partition(|shape| {
            shape.bottom.abs_diff(floor) <= 1
                && shape
                    .columns
                    .iter()
                    .all(|(_, bottom)| bottom.abs_diff(floor) <= 2)
        }),
        None => (Vec::new(), shapes),
    };

    // The price candles are above the volume bars.
    if let Some(floor) = floor {
        let volume_top = volume_shapes
            .iter()
            .map(|shape| shape.top)
            .min()
            .unwrap_or(floor);

        candles.retain(|shape| shape.bottom < volume_top + 2);
    }

    candles.sort_by_key(|shape| shape.left);

    // Two shapes over the same columns: keep the taller.
    let mut kept: Vec<Shape> = Vec::new();

    for shape in candles {
        match kept.last_mut() {
            Some(last) if shape.left <= last.right => {
                if shape.bottom - shape.top > last.bottom - last.top {
                    *last = shape;
                }
            }
            _ => kept.push(shape),
        }
    }

    let kept = candle_run(kept);

    if kept.len() < FEWEST {
        return Err(format!(
            "found {} candles; a chart with green and red candles is needed",
            kept.len()
        ));
    }

    let highest = kept.iter().map(|shape| shape.top).min().unwrap_or(0) as f64;
    let lowest = kept
        .iter()
        .map(|shape| shape.bottom)
        .max()
        .unwrap_or(height) as f64;
    let span = (lowest - highest).max(1.0);

    let (top_price, bottom_price) = prices.unwrap_or((110.0, 100.0));
    let price = |y: f64| bottom_price + (lowest - y) / span * (top_price - bottom_price);

    let volume_of = |shape: &Shape| -> f64 {
        let middle = (shape.left + shape.right) / 2;

        volume_shapes
            .iter()
            .filter(|bar| bar.left <= middle + 1 && bar.right + 1 >= middle)
            .map(|bar| f64::from(bar.bottom - bar.top + 1))
            .fold(0.0, f64::max)
    };

    let count = kept.len() as i64;

    let bars: Vec<Bar> = kept
        .iter()
        .enumerate()
        .map(|(at, shape)| {
            let (body_top, body_bottom) = body(shape);

            let (open_y, close_y) = match shape.colour {
                Colour::Green => (body_bottom, body_top),
                Colour::Red => (body_top, body_bottom),
            };

            let open = price(f64::from(open_y));
            let close = price(f64::from(close_y));

            Bar {
                time_ms: last_ms - (count - 1 - at as i64) * interval_ms,
                open,
                high: price(f64::from(shape.top)).max(open.max(close)),
                low: price(f64::from(shape.bottom)).min(open.min(close)),
                close,
                volume: volume_of(shape) * 1_000.0,
            }
        })
        .collect();

    let mut notes = Vec::new();

    if prices.is_none() {
        notes.push("No price scale given: prices are relative, from 100 to 110.".to_string());
    }

    if volume_shapes.is_empty() {
        notes.push("No volume bars found.".to_string());
    }

    let _ = width;

    Ok(ChartRead {
        volumes: !volume_shapes.is_empty(),
        priced: prices.is_some(),
        bars,
        notes,
    })
}

/// The candles among the shapes: the longest run of shapes, left to
/// right, whose gaps stay near the usual gap. Words and numbers drawn in
/// green or red elsewhere on the screen form short runs of their own, or
/// lie far from the chart.
fn candle_run(shapes: Vec<Shape>) -> Vec<Shape> {
    if shapes.len() < 3 {
        return shapes;
    }

    let centre = |shape: &Shape| f64::from(shape.left + shape.right) / 2.0;

    let mut gaps: Vec<f64> = shapes
        .windows(2)
        .map(|two| centre(&two[1]) - centre(&two[0]))
        .collect();

    gaps.sort_by(f64::total_cmp);

    let usual = gaps[gaps.len() / 2].max(1.0);

    let mut best = (0, 0);
    let mut start = 0;

    for at in 1..=shapes.len() {
        let breaks =
            at == shapes.len() || centre(&shapes[at]) - centre(&shapes[at - 1]) > usual * 3.0;

        if breaks {
            if at - start > best.1 - best.0 {
                best = (start, at);
            }

            start = at;
        }
    }

    shapes
        .into_iter()
        .skip(best.0)
        .take(best.1 - best.0)
        .collect()
}

/// The body of a candle: the run of colour in its edge columns, or its
/// whole height when it is too thin to have edges apart from the wick.
fn body(shape: &Shape) -> (u32, u32) {
    let columns = &shape.columns;

    if columns.len() < 3 {
        return (shape.top, shape.bottom);
    }

    let mut edges = vec![columns[0], columns[columns.len() - 1]];

    if columns.len() >= 5 {
        edges.push(columns[1]);
        edges.push(columns[columns.len() - 2]);
    }

    let top = edges.iter().map(|(top, _)| *top).max().unwrap_or(shape.top);
    let bottom = edges
        .iter()
        .map(|(_, bottom)| *bottom)
        .min()
        .unwrap_or(shape.bottom);

    if bottom < top {
        (shape.top, shape.bottom)
    } else {
        (top, bottom)
    }
}

/// Joins pixels of the same colour that touch, side or corner.
fn shapes(picture: &RgbImage) -> Vec<Shape> {
    let (width, height) = picture.dimensions();
    let index = |x: u32, y: u32| (y * width + x) as usize;

    let colours: Vec<Option<Colour>> = picture.pixels().map(|pixel| colour(pixel.0)).collect();
    let mut seen = vec![false; colours.len()];
    let mut found = Vec::new();
    let mut stack = Vec::new();

    for y in 0..height {
        for x in 0..width {
            let at = index(x, y);

            let Some(colour) = colours[at] else {
                continue;
            };

            if seen[at] {
                continue;
            }

            seen[at] = true;
            stack.push((x, y));

            let mut pixels = Vec::new();

            while let Some((x, y)) = stack.pop() {
                pixels.push((x, y));

                for dy in -1i32..=1 {
                    for dx in -1i32..=1 {
                        let (nx, ny) = (x as i32 + dx, y as i32 + dy);

                        if nx < 0 || ny < 0 || nx >= width as i32 || ny >= height as i32 {
                            continue;
                        }

                        let near = index(nx as u32, ny as u32);

                        if !seen[near] && colours[near] == Some(colour) {
                            seen[near] = true;
                            stack.push((nx as u32, ny as u32));
                        }
                    }
                }
            }

            let left = pixels.iter().map(|(x, _)| *x).min().unwrap_or(0);
            let right = pixels.iter().map(|(x, _)| *x).max().unwrap_or(0);
            let mut columns = vec![(u32::MAX, 0u32); (right - left + 1) as usize];

            for (x, y) in &pixels {
                let column = &mut columns[(x - left) as usize];

                column.0 = column.0.min(*y);
                column.1 = column.1.max(*y);
            }

            found.push(Shape {
                colour,
                left,
                right,
                top: pixels.iter().map(|(_, y)| *y).min().unwrap_or(0),
                bottom: pixels.iter().map(|(_, y)| *y).max().unwrap_or(0),
                pixels: pixels.len() as u32,
                columns,
            });
        }
    }

    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    const GREEN: Rgb<u8> = Rgb([38, 166, 154]);
    const RED: Rgb<u8> = Rgb([239, 83, 80]);
    const GROUND: Rgb<u8> = Rgb([255, 255, 255]);

    /// Draws candles with given wick and body rows, and volume bars.
    fn chart(candles: &[(u32, u32, u32, u32, bool)], volumes: bool) -> RgbImage {
        let mut picture = RgbImage::from_pixel(400, 300, GROUND);

        for (at, (high, open, close, low, green)) in candles.iter().enumerate() {
            let left = 10 + at as u32 * 14;
            let fill = if *green { GREEN } else { RED };

            for y in *high..=*low {
                picture.put_pixel(left + 4, y, fill);
            }

            for y in (*open).min(*close)..=(*open).max(*close) {
                for x in left..left + 9 {
                    picture.put_pixel(x, y, fill);
                }
            }

            if volumes {
                let size = 10 + (at as u32 * 7) % 30;

                for y in 290 - size..=290 {
                    for x in left..left + 9 {
                        picture.put_pixel(x, y, fill);
                    }
                }
            }
        }

        picture
    }

    fn sample() -> Vec<(u32, u32, u32, u32, bool)> {
        // (high, open, close, low, green): rows, lower row is lower price.
        (0..12)
            .map(|at| {
                let base = 150 - at * 6;

                if at % 3 == 2 {
                    (base - 25, base - 18, base - 6, base, false)
                } else {
                    (base - 30, base - 5, base - 20, base, true)
                }
            })
            .collect()
    }

    #[test]
    fn candles_are_read_in_order_with_their_colours() {
        let read = read_rgb(
            &chart(&sample(), false),
            Some((110.0, 100.0)),
            60_000,
            600_000,
        )
        .unwrap();

        assert_eq!(read.bars.len(), 12);
        assert!(read.priced);
        assert!(!read.volumes);

        for (bar, (_, _, _, _, green)) in read.bars.iter().zip(sample()) {
            assert_eq!(bar.close > bar.open, green, "{bar:?}");
            assert!(bar.high >= bar.open.max(bar.close));
            assert!(bar.low <= bar.open.min(bar.close));
        }

        // Rising candles: the last close is above the first.
        assert!(read.bars[11].close > read.bars[0].close);

        // The highest high and lowest low are the prices given.
        let high = read
            .bars
            .iter()
            .map(|bar| bar.high)
            .fold(f64::MIN, f64::max);
        let low = read.bars.iter().map(|bar| bar.low).fold(f64::MAX, f64::min);

        assert!((high - 110.0).abs() < 1e-9 && (low - 100.0).abs() < 1e-9);
        assert_eq!(read.bars[11].time_ms, 600_000);
        assert_eq!(read.bars[10].time_ms, 540_000);
    }

    #[test]
    fn volume_bars_are_told_apart_and_read_as_volume() {
        let read = read_rgb(&chart(&sample(), true), None, 60_000, 600_000).unwrap();

        assert_eq!(read.bars.len(), 12);
        assert!(read.volumes);
        assert!(read.bars.iter().all(|bar| bar.volume > 0.0));
        assert!(read.bars[1].volume > read.bars[0].volume);
    }

    #[test]
    fn a_picture_without_candles_is_refused() {
        let blank = RgbImage::from_pixel(100, 100, GROUND);

        assert!(read_rgb(&blank, None, 60_000, 0).is_err());
    }
}
