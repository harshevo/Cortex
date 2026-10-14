//! The whole analysis, end to end, on made-up series whose answer is
//! known.
//!
//! These run in an unoptimised build, where the networks are some fifty
//! times slower, so the series are short and the networks small. The
//! same two series at full size are run by `examples/analyse.rs`.

use std::cell::{Cell, RefCell};

use crate::analysis::*;
use crate::{Error, synthetic};

/// Small enough to train in about a second unoptimised.
fn small() -> Settings {
    Settings {
        units: 10,
        epochs: 12,
        window: 16,
        ..Settings::default()
    }
}

fn accuracy(analysis: &Analysis, predictor: Predictor) -> f64 {
    analysis.score(predictor).unwrap().accuracy.unwrap()
}

/// With the clock readings set aside, which differ from run to run.
fn untimed(mut analysis: Analysis) -> Analysis {
    analysis.timings = Timings::default();

    analysis
}

#[test]
fn a_series_with_a_pattern_is_found_to_have_one() {
    let bars = synthetic::patterned(700, 1);
    let analysis = analyse(&bars, &small(), &|_| {}).unwrap();

    let combined = accuracy(&analysis, Predictor::Combined);
    let liquid = accuracy(&analysis, Predictor::Liquid);
    let same = accuracy(&analysis, Predictor::Persistence);
    let up = accuracy(&analysis, Predictor::AlwaysUp);

    println!(
        "pattern: combined {combined:.3} liquid {liquid:.3} same {same:.3} up {up:.3}\n{}",
        analysis.verdict.text
    );

    assert!(combined > 0.65, "{combined}");
    assert!(combined > same + 0.1, "{combined} {same}");
    assert!(combined > up + 0.1, "{combined} {up}");
    assert!(liquid > 0.65, "{liquid}");
    assert_ne!(
        analysis.verdict.edge,
        Edge::None,
        "{}",
        analysis.verdict.text
    );

    // It also predicts the size of the return better than "no change".
    let error = |predictor| analysis.score(predictor).unwrap().mean_abs_error;

    assert!(error(Predictor::Combined) < error(Predictor::Zero));
    assert!(error(Predictor::Combined) < error(Predictor::Persistence));

    let combined = analysis.score(Predictor::Combined).unwrap();

    assert!(combined.p_value.unwrap() < 0.001);
    assert!(combined.accuracy_low.unwrap() > 0.5);
}

#[test]
fn a_random_walk_is_found_to_have_none() {
    let bars = synthetic::random_walk(700, 1);
    let analysis = analyse(&bars, &small(), &|_| {}).unwrap();

    println!(
        "walk: combined {:.3} liquid {:.3}\n{}",
        accuracy(&analysis, Predictor::Combined),
        accuracy(&analysis, Predictor::Liquid),
        analysis.verdict.text
    );

    assert_eq!(
        analysis.verdict.edge,
        Edge::None,
        "{}",
        analysis.verdict.text
    );
    assert!(analysis.verdict.text.starts_with("No edge found"));
    assert!(analysis.verdict.text.ends_with("not advice."));
}

#[test]
fn an_analysis_holds_everything_a_screen_needs() {
    let bars = synthetic::patterned(400, 3);
    let settings = Settings {
        horizon: 2,
        ..small()
    };

    let analysis = analyse(&bars, &settings, &|_| {}).unwrap();

    assert_eq!(analysis.bars, 400);
    assert_eq!(analysis.train_bars, 300);
    assert_eq!(analysis.horizon, 2);

    let predictors: Vec<Predictor> = analysis
        .scores
        .iter()
        .map(|score| score.predictor)
        .collect();

    assert_eq!(
        predictors,
        [
            Predictor::Liquid,
            Predictor::Dendritic,
            Predictor::Combined,
            Predictor::AlwaysUp,
            Predictor::Persistence,
            Predictor::Zero,
        ]
    );

    for score in &analysis.scores {
        // The last two held-out bars have no outcome yet.
        assert_eq!(score.bars, 98, "{}", score.name);
        assert!(score.scored <= score.bars && score.right <= score.scored);
        assert!(score.mean_abs_error.is_finite() && score.mean_abs_error > 0.0);

        if let Some(p) = score.p_value {
            assert!((0.0..=1.0).contains(&p));
        }
    }

    assert_eq!(analysis.score(Predictor::Zero).unwrap().accuracy, None);
    assert_eq!(analysis.score(Predictor::AlwaysUp).unwrap().scored, 98);

    let series = &analysis.series;

    assert_eq!(series.first_bar, 300);
    assert_eq!(series.predicted_return.len(), 100);
    assert_eq!(series.probability_up.len(), 100);
    assert_eq!(series.actual_return.len(), 100);
    assert!(series.actual_return[..98].iter().all(Option::is_some));
    assert!(series.actual_return[98..].iter().all(Option::is_none));
    assert!(series.predicted_return.iter().all(|x| x.is_finite()));
    assert!(
        series
            .probability_up
            .iter()
            .all(|p| (0.0..=1.0).contains(p))
    );

    // The actual return is the one the bars say.
    let actual = (bars[302].close / bars[300].close).ln() as f32;

    assert_eq!(series.actual_return[0], Some(actual));

    assert_eq!(analysis.liquid_losses.len(), 12);
    assert_eq!(analysis.combined_losses.len(), 12);
    assert!(
        analysis
            .combined_losses
            .iter()
            .all(|loss| loss.fit.is_finite() && loss.fit > 0.0)
    );
    assert!(
        analysis.combined_losses[11].fit < analysis.combined_losses[0].fit,
        "{:?}",
        analysis.combined_losses
    );

    assert_eq!(analysis.regimes.len(), 400);
    assert_eq!(analysis.regime_names.len(), 9);
    assert!(analysis.regimes.iter().all(|regime| (*regime as usize) < 9));

    assert_eq!(analysis.growth.moments, 400);
    assert!(analysis.growth.neurons > 14 && analysis.growth.synapses > 0);
    assert!(analysis.growth.segments > 0);

    let forecast = &analysis.forecast;

    assert_eq!(forecast.after_time_ms, bars[399].time_ms);
    assert_eq!(forecast.horizon, 2);
    assert!(forecast.expected_return.is_finite());
    assert!((0.5..=1.0).contains(&forecast.confidence));
    assert_eq!(
        forecast.direction == Direction::Up,
        forecast.probability_up > 0.5
    );
    assert_eq!(forecast.dendritic.now.len(), 5);
    assert!(!forecast.dendritic.regime.is_empty());

    assert!(analysis.notes.iter().any(|note| note.contains("overlap")));
    assert!(analysis.notes.last().unwrap().contains("not advice"));

    // And it goes to JSON and back unchanged.
    let saved = serde_json::to_string(&analysis).unwrap();
    let loaded: Analysis = serde_json::from_str(&saved).unwrap();

    assert_eq!(loaded, analysis);
    assert!(!saved.contains("\"buy\"") && !saved.contains("\"sell\"") && !saved.contains("signal"));
}

#[test]
fn the_same_seed_gives_the_same_analysis_and_another_seed_another() {
    let bars = synthetic::patterned(300, 5);

    let run = |seed: u64| {
        let settings = Settings { seed, ..small() };
        let (model, analysis) = Model::train(&bars, &settings, &|_| {}).unwrap();

        (model, untimed(analysis))
    };

    let (model, analysis) = run(7);
    let (model_again, analysis_again) = run(7);

    assert_eq!(analysis, analysis_again);
    assert_eq!(model, model_again);
    assert_eq!(model.to_json(), model_again.to_json());

    let (_, other) = run(8);

    assert_ne!(
        analysis.series.predicted_return,
        other.series.predicted_return
    );
}

#[test]
fn nothing_fitted_depends_on_the_held_out_bars() {
    let bars = synthetic::patterned(300, 6);
    let mut other = bars.clone();

    // Bars from the first held-out one on are replaced with different
    // prices at the same times.
    let first_test = 225;
    let replacement = synthetic::random_walk(300, 99);

    for at in first_test..300 {
        other[at] = crate::Bar {
            time_ms: bars[at].time_ms,
            ..replacement[at]
        };
    }

    let (model, analysis) = Model::train(&bars, &small(), &|_| {}).unwrap();
    let (other_model, other_analysis) = Model::train(&other, &small(), &|_| {}).unwrap();

    assert_eq!(analysis.train_bars, first_test);

    let fitted = |model: &Model| {
        let saved: serde_json::Value = serde_json::from_str(&model.to_json()).unwrap();

        [
            "cell",
            "scaler",
            "outlook_scaler",
            "target_mean",
            "target_spread",
            "gap_ms",
        ]
        .map(|part| saved[part].clone())
    };

    assert_eq!(fitted(&model), fitted(&other_model));
    assert_eq!(model.regime_buckets(), other_model.regime_buckets());
    assert_eq!(analysis.liquid_losses, other_analysis.liquid_losses);
    assert_eq!(analysis.combined_losses, other_analysis.combined_losses);
    assert_eq!(
        analysis.regimes[..first_test],
        other_analysis.regimes[..first_test]
    );

    // The held-out bars did differ, and so did what was made of them.
    assert_ne!(
        analysis.series.predicted_return,
        other_analysis.series.predicted_return
    );
}

#[test]
fn a_saved_model_comes_back_the_same_and_goes_on_the_same() {
    let bars = synthetic::patterned(360, 2);
    let (model, analysis) = Model::train(&bars[..300], &small(), &|_| {}).unwrap();

    assert_eq!(model.forecast(), &analysis.forecast);
    assert_eq!(model.bars_seen(), 300);

    let saved = model.to_json();
    let mut loaded = Model::from_json(&saved).unwrap();

    assert_eq!(loaded, model);
    assert_eq!(loaded.forecast(), model.forecast());
    assert_eq!(loaded.growth(), model.growth());
    assert_eq!(loaded.to_json(), saved);

    // New bars all at once, a few at a time, and after a save and load
    // in the middle, come to the same thing.
    let mut at_once = model.clone();
    let mut in_parts = model.clone();

    assert_eq!(
        at_once.update(&bars[300..]),
        Update {
            added: 60,
            ignored: 0
        }
    );

    in_parts.update(&bars[300..301]);
    in_parts.update(&bars[301..330]);

    let mut in_parts = Model::from_json(&in_parts.to_json()).unwrap();

    in_parts.update(&bars[330..]);
    loaded.update(&bars[300..]);

    assert_eq!(in_parts, at_once);
    assert_eq!(loaded, at_once);
    assert_eq!(in_parts.forecast(), at_once.forecast());

    assert_eq!(at_once.bars_seen(), 360);
    assert_eq!(at_once.forecast().after_time_ms, bars[359].time_ms);
    assert_ne!(at_once.forecast(), model.forecast());

    // Bars already seen, and bars that cannot be used, are left out.
    let before = at_once.clone();
    let mut broken = bars[359];

    broken.time_ms += 86_400_000;
    broken.close = f64::NAN;

    assert_eq!(
        at_once.update(&[bars[10], bars[359], broken]),
        Update {
            added: 0,
            ignored: 3
        }
    );
    assert_eq!(at_once, before);
}

#[test]
fn a_damaged_saved_model_is_refused() {
    let bars = synthetic::patterned(200, 2);
    let (model, _) = Model::train(&bars, &small(), &|_| {}).unwrap();
    let saved = model.to_json();

    assert!(matches!(
        Model::from_json(&saved[..saved.len() / 2]),
        Err(Error::Saved(_))
    ));
    assert!(matches!(Model::from_json("{}"), Err(Error::Saved(_))));

    // A state of the wrong length for the network.
    let mut value: serde_json::Value = serde_json::from_str(&saved).unwrap();

    value["state"] = serde_json::json!([0.0, 0.0]);

    assert!(matches!(
        Model::from_json(&value.to_string()),
        Err(Error::Saved(_))
    ));
}

#[test]
fn progress_only_goes_forward_and_ends_at_one() {
    let bars = synthetic::random_walk(200, 4);

    let told: RefCell<Vec<Progress>> = RefCell::new(Vec::new());
    let calls = Cell::new(0);

    analyse(&bars, &small(), &|progress| {
        calls.set(calls.get() + 1);
        told.borrow_mut().push(progress);
    })
    .unwrap();

    let told = told.into_inner();

    assert!(calls.get() > 20);
    assert!(
        told.windows(2)
            .all(|two| two[0].fraction <= two[1].fraction)
    );
    assert_eq!(told[0].stage, Stage::Features);
    assert_eq!(told[0].fraction, 0.0);
    assert_eq!(
        *told.last().unwrap(),
        Progress {
            stage: Stage::Done,
            fraction: 1.0
        }
    );

    for stage in [
        Stage::Dendritic,
        Stage::Liquid,
        Stage::Combined,
        Stage::Scoring,
    ] {
        assert!(
            told.iter().any(|progress| progress.stage == stage),
            "{stage:?}"
        );
    }
}

#[test]
fn it_runs_on_a_background_thread_and_its_results_can_be_sent() {
    fn sendable<T: Send + 'static>() {}

    sendable::<Model>();
    sendable::<Analysis>();
    sendable::<Settings>();
    sendable::<Error>();

    let bars = synthetic::random_walk(150, 4);
    let (sender, receiver) = std::sync::mpsc::channel();

    let worker = std::thread::spawn(move || {
        let result = Model::train(&bars, &small(), &|progress| {
            sender.send(progress.fraction).ok();
        });

        result.map(|(model, analysis)| (model, analysis.verdict.edge))
    });

    let (model, _) = worker.join().unwrap().unwrap();

    assert_eq!(model.bars_seen(), 150);
    assert_eq!(receiver.try_iter().last(), Some(1.0));
}

#[test]
fn too_few_bars_and_unusable_bars_are_refused_with_the_reason() {
    let bars = synthetic::random_walk(84, 4);

    assert_eq!(
        analyse(&bars, &Settings::default(), &|_| {}).unwrap_err(),
        Error::TooFewBars {
            needed: 85,
            found: 84
        }
    );

    let mut bars = synthetic::random_walk(200, 4);

    bars[17].low = 0.0;

    let error = analyse(&bars, &small(), &|_| {}).unwrap_err();

    assert!(
        matches!(&error, Error::BadBar { index: 17, .. }),
        "{error:?}"
    );
    assert!(error.to_string().contains("bar 17"));
}

#[test]
fn bars_out_of_order_or_repeated_are_put_right_first() {
    let bars = synthetic::patterned(200, 4);

    let mut shuffled = bars.clone();

    shuffled.reverse();
    shuffled.push(bars[50]);

    let straight = untimed(analyse(&bars, &small(), &|_| {}).unwrap());
    let mut put_right = untimed(analyse(&shuffled, &small(), &|_| {}).unwrap());

    assert!(put_right.notes[0].contains("1 bars had the same time"));

    put_right.notes.remove(0);

    assert_eq!(put_right, straight);
}

#[test]
fn a_csv_file_goes_all_the_way_through() {
    let bars = synthetic::patterned(200, 4);

    let mut text = String::from("timestamp;open;high;low;close;volume\n");

    for bar in &bars {
        text.push_str(&format!(
            "{};{};{};{};{};{}\n",
            bar.time_ms, bar.open, bar.high, bar.low, bar.close, bar.volume
        ));
    }

    let parsed = crate::parse_csv(&text).unwrap();

    assert_eq!(parsed.bars, bars);

    let from_file = untimed(analyse(&parsed.bars, &small(), &|_| {}).unwrap());
    let direct = untimed(analyse(&bars, &small(), &|_| {}).unwrap());

    assert_eq!(from_file, direct);
}
