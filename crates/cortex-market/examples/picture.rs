//! Reads the candles off a chart picture and prints them.
//!
//! `cargo run -p cortex-market --example picture -- chart.png [highest lowest]`

fn main() {
    let mut args = std::env::args().skip(1);
    let file = args.next().expect("a picture");
    let prices = match (args.next(), args.next()) {
        (Some(high), Some(low)) => Some((
            high.parse().expect("a price"),
            low.parse().expect("a price"),
        )),
        _ => None,
    };

    let bytes = std::fs::read(&file).expect("readable");
    let read = cortex_market::picture::read_chart(&bytes, prices, 300_000, 0).expect("candles");

    println!(
        "{} candles, volumes {}, {:?}",
        read.bars.len(),
        read.volumes,
        read.notes
    );

    for bar in read.bars.iter().rev().take(8).rev() {
        println!(
            "o {:8.2} h {:8.2} l {:8.2} c {:8.2} v {:6.0}",
            bar.open, bar.high, bar.low, bar.close, bar.volume
        );
    }
}
