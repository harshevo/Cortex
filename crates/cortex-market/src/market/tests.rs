use std::sync::mpsc;
use std::time::{Duration, Instant};

use super::*;
use crate::standin;

/// Waits until `ready` holds of the snapshot, or panics after a minute.
fn wait(
    market: &Market,
    told: &mpsc::Receiver<()>,
    what: &str,
    ready: impl Fn(&Snapshot) -> bool,
) -> Snapshot {
    let began = Instant::now();

    loop {
        let snapshot = market.snapshot();

        if ready(&snapshot) {
            return snapshot;
        }

        assert!(
            began.elapsed() < Duration::from_secs(90),
            "never {what}: work {:?}, feed {:?}, connection {:?}, problem {:?}",
            snapshot.work,
            snapshot.feed,
            snapshot.connection,
            snapshot.problem
        );

        let _ = told.recv_timeout(Duration::from_millis(200));
    }
}

fn market(folder: &Path) -> (Market, mpsc::Receiver<()>) {
    let (tell, told) = mpsc::channel();
    let tell = Mutex::new(tell);

    let market = Market::new(folder, move || {
        let _ = tell.lock().unwrap().send(());
    });

    (market, told)
}

#[test]
fn a_live_instrument_is_fetched_trained_on_followed_and_forecasts_recorded() {
    let addresses = standin::start();

    // Read when the credentials are made; this is the only test that sets them.
    unsafe {
        std::env::set_var("CORTEX_DHAN_API", &addresses.api);
        std::env::set_var("CORTEX_DHAN_FEED", &addresses.feed);
        std::env::set_var("CORTEX_MARKET_ALWAYS_OPEN", "1");
    }

    let folder = tempfile::tempdir().unwrap();
    let (market, told) = market(folder.path());

    market.connect("1000000001", "wrong");

    let refused = wait(&market, &told, "refused", |s| {
        matches!(s.connection, Connection::Failed(_))
    });

    match refused.connection {
        Connection::Failed(reason) => assert!(reason.contains("expired"), "{reason}"),
        other => panic!("{other:?}"),
    }

    market.set_interval(Interval::M1);
    market.connect("1000000001", "stand-in");

    let snapshot = wait(&market, &told, "trained", |s| {
        s.report.is_some() && !s.rows.is_empty()
    });

    assert!(matches!(snapshot.connection, Connection::Connected(_)));
    assert!(matches!(snapshot.source, Some(Source::Dhan { .. })));
    assert!(snapshot.bars.len() > 500, "{}", snapshot.bars.len());
    assert_eq!(snapshot.rows.len(), 10);
    assert!(snapshot.plan.is_some());
    assert!(snapshot.now.rsi.is_some() && snapshot.now.vwap.is_some());

    // Every network's forecast at the latest bar is written down.
    let sources: std::collections::HashSet<String> =
        snapshot.outcomes.iter().map(|p| p.source.clone()).collect();

    for short in ["LNN", "DANN", "SNN", "ENS"] {
        assert!(sources.contains(short), "{sources:?}");
    }

    // The feed's ticks build the forming candle.
    let live = wait(&market, &told, "live", |s| {
        s.feed == FeedState::Live && s.now.tick_ms.is_some()
    });

    assert!(live.forming.is_some());
    assert!(live.now.spread.is_some() && live.now.buy_share.is_some());

    // Choosing another brain changes the forecast shown.
    let before = live.rows.clone();

    market.set_brain(Brain::Spiking);

    let spiking = market.snapshot();

    assert_eq!(spiking.brain, Brain::Spiking);
    assert_ne!(spiking.rows, before);

    // Kept for next time: the instrument, the interval and the brain.
    drop(market);

    let kept: Kept = read_json(&folder.path().join("market.json")).unwrap();

    assert_eq!(kept.interval, Some(Interval::M1));
    assert_eq!(kept.brain, Some(Brain::Spiking));
    assert!(kept.instrument.is_some());
    assert!(folder.path().join("dhan.json").exists());
}

#[test]
fn a_file_is_trained_on_and_too_short_a_picture_is_read_by_the_last_networks() {
    let folder = tempfile::tempdir().unwrap();
    let (market, told) = market(folder.path());

    let bars = cortex_liquid::synthetic::patterned(900, 2);
    let mut text = String::from("time,open,high,low,close,volume\n");

    for bar in &bars {
        text.push_str(&format!(
            "{},{},{},{},{},{}\n",
            bar.time_ms / 1_000,
            bar.open,
            bar.high,
            bar.low,
            bar.close,
            bar.volume
        ));
    }

    assert_eq!(market.load_csv("made.csv", &text).unwrap(), 900);

    let trained = wait(&market, &told, "trained", |s| s.report.is_some());

    assert_eq!(trained.interval, Interval::Day);
    assert_eq!(trained.rows.len(), 10);
    assert!(
        trained.outcomes.is_empty(),
        "a file's forecasts are not recorded"
    );

    // A short series read off a picture: too few to train on.
    let picture = cortex_liquid::synthetic::random_walk(40, 3);
    let mut image = image::RgbImage::from_pixel(600, 300, image::Rgb([255, 255, 255]));
    let high = picture.iter().map(|b| b.high).fold(f64::MIN, f64::max);
    let low = picture.iter().map(|b| b.low).fold(f64::MAX, f64::min);
    let y = |price: f64| (20.0 + (high - price) / (high - low) * 250.0) as u32;

    for (at, bar) in picture.iter().enumerate() {
        let left = 10 + at as u32 * 14;
        let colour = if bar.close >= bar.open {
            image::Rgb([38, 166, 154])
        } else {
            image::Rgb([239, 83, 80])
        };

        for row in y(bar.high)..=y(bar.low) {
            image.put_pixel(left + 4, row, colour);
        }

        for row in y(bar.open.max(bar.close))
            ..=y(bar.open.min(bar.close)).max(y(bar.open.max(bar.close)) + 1)
        {
            for x in left..left + 9 {
                image.put_pixel(x, row, colour);
            }
        }
    }

    let mut png = Vec::new();

    image
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .unwrap();

    let read = market.load_picture("chart.png", png, None).unwrap();

    assert!(read >= 30, "{read}");

    let pictured = market.snapshot();

    assert!(matches!(pictured.source, Some(Source::Picture { .. })));
    assert_eq!(pictured.rows.len(), 10, "{:?}", pictured.work);
    assert!(
        pictured
            .notes
            .iter()
            .any(|note| note.contains("trained on made.csv")),
        "{:?}",
        pictured.notes
    );
}

/// Answers every prompt with ten candles a little above the last close.
struct Scripted;

impl Oracle for Scripted {
    fn name(&self) -> String {
        "scripted".to_string()
    }

    fn ask(&self, prompt: String, _picture: Option<Picture>) -> crate::llm::Answer {
        Box::pin(async move {
            assert!(prompt.contains("Recent candles"), "{prompt}");

            let candles: Vec<String> = (1..=10)
                .map(|step| {
                    let close = 100.0 + f64::from(step);

                    format!(
                        "{{\"o\":{},\"h\":{},\"l\":{},\"c\":{close},\"v\":10}}",
                        close - 1.0,
                        close + 0.5,
                        close - 1.5
                    )
                })
                .collect();

            Ok(format!(
                "{{\"bias\":\"up\",\"reason\":\"Higher lows.\",\"candles\":[{}]}}",
                candles.join(",")
            ))
        })
    }
}

#[test]
fn the_language_model_forecast_fills_the_table_when_chosen() {
    let folder = tempfile::tempdir().unwrap();
    let (market, told) = market(folder.path());

    let bars = cortex_liquid::synthetic::random_walk(300, 4);
    let mut text = String::from("time,open,high,low,close,volume\n");

    for bar in &bars {
        text.push_str(&format!(
            "{},{},{},{},{},{}\n",
            bar.time_ms / 1_000,
            bar.open,
            bar.high,
            bar.low,
            bar.close,
            bar.volume
        ));
    }

    market.load_csv("walk.csv", &text).unwrap();
    market.set_oracle(Some(Arc::new(Scripted)));
    market.set_brain(Brain::Llm);

    let answered = wait(&market, &told, "answered", |s| {
        s.llm.calls == 1 && s.llm.asking == Asking::Idle
    });

    assert_eq!(answered.llm.model.as_deref(), Some("scripted"));
    assert_eq!(answered.llm.reason, "Higher lows.");
    assert_eq!(answered.rows.len(), 10);
    assert_eq!(answered.rows[9].close, 110.0);
    assert!(
        answered
            .plan
            .unwrap()
            .scenarios
            .iter()
            .any(|s| s.case.contains("LLM path"))
    );
}
