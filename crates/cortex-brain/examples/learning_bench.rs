//! Frozen evaluations. `cargo run -p cortex-brain --example learning_bench -- <experiment directory>`
use cortex_brain::{
    Brain,
    brain::{arithmetic::evaluate, knowledge::question_key},
};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::BufRead;
use std::path::Path;
use std::time::Instant;

fn rows(file: &Path) -> Vec<Value> {
    std::io::BufReader::new(std::fs::File::open(file).unwrap())
        .lines()
        .map(|line| serde_json::from_str(&line.unwrap()).unwrap())
        .collect()
}

fn evaluate_rows(brain: &Brain, rows: &[Value]) -> Value {
    let mut scores: BTreeMap<String, (u64, u64, u64)> = BTreeMap::new();
    let mut examples = Vec::new();
    for row in rows {
        let mut frozen = brain.clone();
        let answer = frozen.chat("evaluation", row["question"].as_str().unwrap());
        let method = frozen
            .snapshot()
            .cognition
            .traces
            .last()
            .unwrap()
            .method
            .clone();
        let correct = if row["domain"] == "solution_equation_execution" {
            let expected = evaluate(row["reference"].as_str().unwrap());
            let actual = answer
                .text
                .split_once(" = ")
                .and_then(|(_, rhs)| evaluate(rhs.trim_end_matches('.')).ok());
            matches!((actual,expected),(Some(a),Ok(b)) if a.value==b.value)
                && method == "built_in_arithmetic"
        } else {
            question_key(&answer.text) == question_key(row["expected"].as_str().unwrap())
        };
        let score = scores
            .entry(row["domain"].as_str().unwrap().into())
            .or_default();
        score.0 += 1;
        score.1 += u64::from(correct);
        score.2 += u64::from(method == "unknown");
        if examples.len() < 4 || (!correct && examples.len() < 16) {
            examples.push(json!({"id":row["id"],"domain":row["domain"],"question":row["question"],"answer":answer.text,"expected":row["expected"],"method":method,"correct":correct}));
        }
    }
    json!({"domains":scores.into_iter().map(|(domain,(total,correct,abstained))| (domain,json!({"total":total,"correct":correct,"accuracy":correct as f64/total.max(1) as f64,"abstained":abstained}))).collect::<BTreeMap<_,_>>(),"examples":examples})
}

fn sequence_experiment() -> Value {
    let mut results = Vec::new();
    // Seeds change presentation order and names; fixed hyperparameters.
    for seed in 0..10 {
        let stories = [
            format!("<story> the fox{seed} sat on the mat ."),
            format!("<story> the dog{seed} ran to the park ."),
            format!("<story> the bird{seed} sang in the tree ."),
        ];
        let test = [
            format!("<story> the fox{seed} sat on the mat . the dog{seed} ran to the park ."),
            format!("<story> the bird{seed} sang in the tree . the fox{seed} sat on the mat ."),
        ];
        let test_refs: Vec<&str> = test.iter().map(String::as_str).collect();
        let mut once = Brain::new();
        let mut rehearsed = Brain::new();
        let mut fixed = Brain::new();
        let mut passes = 0;
        let mut seconds = [0.0; 3];
        for at in 0..stories.len() {
            let story = &stories[(at + seed) % stories.len()];
            let started = Instant::now();
            once.learn(story, "development sequences");
            seconds[0] += started.elapsed().as_secs_f64();
            let started = Instant::now();
            passes += rehearsed.study(story, "development sequences", 4).passes;
            seconds[1] += started.elapsed().as_secs_f64();
            let started = Instant::now();
            for _ in 0..4 {
                fixed.learn(story, "development sequences");
            }
            seconds[2] += started.elapsed().as_secs_f64();
        }
        let once_score = once.assess(&test_refs);
        let study_score = rehearsed.assess(&test_refs);
        let fixed_score = fixed.assess(&test_refs);
        // Replay is a separate, additional update budget, not a free gain.
        let before_steps = rehearsed.cortex.totals.steps;
        let before_scored = rehearsed.stats().scored;
        for _ in 0..12 {
            rehearsed.sleep();
        }
        let replay_score = rehearsed.assess(&test_refs);
        assert_eq!(before_scored, rehearsed.stats().scored);
        results.push(json!({"seed":seed,"one_exposure":once_score,"adaptive_study":study_score,"fixed_four_passes":fixed_score,"study_passes":passes,"fixed_passes":12,"training_seconds":{"one_exposure":seconds[0],"adaptive_including_assessment":seconds[1],"fixed_four_passes":seconds[2]},"after_additional_replay":replay_score,"replay_steps":rehearsed.cortex.totals.steps-before_steps}));
    }
    let mean = |key: &str| {
        results
            .iter()
            .map(|r| r[key]["accuracy"].as_f64().unwrap())
            .sum::<f64>()
            / results.len() as f64
    };
    json!({"mean_one_exposure":mean("one_exposure"),"mean_adaptive_study":mean("adaptive_study"),"mean_fixed_four_passes":mean("fixed_four_passes"),"mean_after_additional_replay":mean("after_additional_replay"),"trials":results,"scope":"Synthetic held-out concatenations of taught sequences. Seeds vary presentation order and names; not a population-level confidence interval."})
}

fn main() {
    let directory = std::env::args()
        .nth(1)
        .expect("Give the prepared experiment directory");
    let directory = Path::new(&directory);
    let train = rows(&directory.join("train.jsonl"));
    let test = rows(&directory.join("test.jsonl"));
    let started = Instant::now();
    let mut brain = Brain::new();
    let baseline = evaluate_rows(&brain, &test);
    let foundations = brain.foundations();
    let mut rehearsal_passes = 0;
    for row in &train {
        let text = format!(
            "<user> {} <brain> {}",
            row["question"].as_str().unwrap(),
            row["expected"].as_str().unwrap()
        );
        rehearsal_passes += brain
            .study(&text, row["source"].as_str().unwrap(), 4)
            .passes;
    }
    let training_recall = evaluate_rows(&brain, &train);
    let held_out = evaluate_rows(&brain, &test);
    let generalization = sequence_experiment();
    brain.save(&directory.join("brain.bin")).unwrap();
    let report = json!({"baseline":baseline,"training_recall":training_recall,"held_out":held_out,"sequence_learning":generalization,"training_examples":train.len(),"foundations":foundations,"rehearsal_passes":rehearsal_passes,"seconds":started.elapsed().as_secs_f64(),"checkpoint":directory.join("brain.bin"),"limits":["Arithmetic is built in; formula substitution uses explicit learned rules, not neural discovery.","Exact-answer evaluation is strict; arbitrary English word problems and advanced science remain unsupported.","The evaluation clone receives only the question. Expected answers are used outside the brain for scoring."]});
    std::fs::write(
        directory.join("report.json"),
        serde_json::to_vec_pretty(&report).unwrap(),
    )
    .unwrap();
    println!("{}",serde_json::to_string_pretty(&json!({"held_out":report["held_out"]["domains"],"training_recall":report["training_recall"]["domains"],"sequence_one_exposure":report["sequence_learning"]["mean_one_exposure"],"sequence_study":report["sequence_learning"]["mean_adaptive_study"],"sequence_fixed":report["sequence_learning"]["mean_fixed_four_passes"],"sequence_after_replay":report["sequence_learning"]["mean_after_additional_replay"],"seconds":report["seconds"]})).unwrap());
}
