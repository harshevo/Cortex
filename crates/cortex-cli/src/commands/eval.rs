//! `cortex eval run | validate | compare`

use std::process::ExitCode;

use anyhow::{Result, bail};
use cortex_core::MaskConfig;
use cortex_core::trajectory::unix_millis;
use cortex_eval::locate::{self, Summary};
use cortex_eval::preserve;
use cortex_eval::report::{self, REPORT_FILE};
use cortex_eval::runner;
use cortex_eval::{Comparison, RunOptions, Suite, SuiteReport, Verdict};

use crate::args::{
    EvalCommand, EvalCompareArgs, EvalLocateArgs, EvalPreserveArgs, EvalRunArgs, EvalValidateArgs,
};
use crate::features::{self, MemoryLocation};
use crate::session;

pub async fn execute(command: EvalCommand) -> Result<ExitCode> {
    match command {
        EvalCommand::Run(args) => run(*args).await,
        EvalCommand::Validate(args) => validate(args).await,
        EvalCommand::Compare(args) => compare(args),
        EvalCommand::Locate(args) => locate(args).await,
        EvalCommand::Preserve(args) => preserve(args).await,
    }
}

async fn run(args: EvalRunArgs) -> Result<ExitCode> {
    if args.repeats == 0 {
        bail!("--repeats must be at least 1");
    }

    let suite = Suite::load(&args.suite)?.filtered(args.filter.as_deref());

    if suite.tasks.is_empty() {
        bail!("no task matches the filter");
    }

    let provider = session::build_provider(&args.model)?;

    // Every task of a run shares one memory, so later tasks can draw on
    // earlier ones. A fresh file per run keeps runs independent unless
    // `--memory-db` deliberately carries one over.
    let memory_location = MemoryLocation {
        path: args.model.memory_db.clone().unwrap_or_else(|| {
            args.out
                .join("memory")
                .join(format!("{}-{}.db", args.label, unix_millis() / 1000))
        }),
        scope: suite.name.clone(),
        skip_identical: true,
    };

    let mut setup = features::setup(&args.model, memory_location).await?;

    // An evaluation compares a capability switched on with it switched
    // off. Holding an adviser out of some runs would water down the side
    // that has it.
    setup.extras.experiment = None;

    let options = RunOptions {
        label: args.label,
        results_dir: args.out,
        max_steps: args.model.max_steps,
        sandbox: args.model.sandbox(),
        tools: setup.tools,
        prune: setup.prune,
        repeats: args.repeats,
        context_sources: setup.context_sources,
        sinks: setup.sinks,
        verifier: setup.verifier,
        advisers: setup.advisers,
        max_verification_rounds: args.model.max_check_rounds,
        prompt_notes: setup.prompt_notes,
        fold_repeats: setup.fold_repeats,
        extras: setup.extras,
        max_resumes: runner::DEFAULT_MAX_RESUMES,
        resume_pause: runner::DEFAULT_RESUME_PAUSE,
    };

    println!(
        "Running {} tasks x {} attempts on {}\n",
        suite.tasks.len(),
        options.repeats,
        provider.model(),
    );

    let (suite_report, run_dir) = cortex_eval::run_suite(&suite, provider, &options, |result| {
        println!(
            "  {:<28} attempt {}  {:<22} {:>3} steps  {}",
            result.task_id,
            result.attempt,
            report::describe_result(result),
            result.steps,
            report::format_cost(result.cost_usd),
        );

        if let Some(error) = &result.error {
            println!("      {error}");
        }
    })
    .await?;

    println!("\n{}", suite_report.to_markdown());
    println!("Report saved to {}", run_dir.join(REPORT_FILE).display());

    Ok(ExitCode::SUCCESS)
}

async fn validate(args: EvalValidateArgs) -> Result<ExitCode> {
    let suite = Suite::load(&args.suite)?;

    let validations = cortex_eval::validate_suite(&suite).await?;

    let mut all_sound = true;

    for validation in &validations {
        let untouched = if validation.fails_untouched {
            "fails untouched (good)"
        } else {
            "PASSES UNTOUCHED (broken)"
        };

        let solution = match validation.passes_with_solution {
            Some(true) => "passes with solution (good)",
            Some(false) => "FAILS WITH SOLUTION (broken)",
            None => "NO SOLUTION TO CHECK",
        };

        println!("  {:<28} {untouched}; {solution}", validation.task_id);

        for part in &validation.parts_passing_untouched {
            println!("      part `{part}` PASSES UNTOUCHED (broken)");
        }

        for part in &validation.parts_failing_with_solution {
            println!("      part `{part}` FAILS WITH SOLUTION (broken)");
        }

        all_sound &= validation.is_sound();
    }

    if all_sound {
        println!("\nAll {} tasks are sound.", validations.len());

        return Ok(ExitCode::SUCCESS);
    }

    println!("\nSome tasks are not sound; fix them before trusting any results.");

    Ok(ExitCode::FAILURE)
}

fn compare(args: EvalCompareArgs) -> Result<ExitCode> {
    let baseline = SuiteReport::load(&args.baseline)?;
    let candidate = SuiteReport::load(&args.candidate)?;

    if baseline.suite != candidate.suite {
        bail!(
            "the reports are for different suites (`{}` and `{}`)",
            baseline.suite,
            candidate.suite
        );
    }

    let comparison = Comparison::between(&baseline, &candidate);

    println!("{}", comparison.render());

    // A regression exits non-zero so the comparison can gate a script.
    let code = match comparison.verdict {
        Verdict::Regressed => ExitCode::from(1),
        Verdict::Improved | Verdict::Unchanged => ExitCode::SUCCESS,
    };

    Ok(code)
}

async fn locate(args: EvalLocateArgs) -> Result<ExitCode> {
    let suite = Suite::load(&args.suite)?.filtered(args.filter.as_deref());

    if suite.tasks.is_empty() {
        bail!("no task matches the filter");
    }

    // A memory that does not exist yet: what is measured is what the
    // sources find in the code, not what earlier runs left behind.
    let scratch = tempfile::tempdir()?;

    let memory_location = MemoryLocation {
        path: args
            .model
            .memory_db
            .clone()
            .unwrap_or_else(|| scratch.path().join("memory.db")),
        scope: suite.name.clone(),
        skip_identical: true,
    };

    let setup = features::setup(&args.model, memory_location).await?;

    if setup.context_sources.is_empty() {
        bail!("no context source is on; try `--with likely`, `--with router` or `--with map`");
    }

    let located = locate::locate_suite(&suite, &setup.context_sources).await?;

    for task in &located {
        let rank = match task.rank {
            Some(rank) => format!("{rank} of {}", task.mentioned),
            None => format!("not among {}", task.mentioned),
        };

        println!(
            "  {:<28} {:<16} {:>6} chars  {:>5} ms",
            task.task_id, rank, task.chars, task.duration_ms
        );
    }

    let summary = Summary::of(&located);

    println!(
        "\nThe right file was named first for {} of {} tasks, in the first three for {}, in \
         the first five for {}.\nMean briefing: {} characters, gathered in {} ms.",
        summary.first,
        summary.tasks,
        summary.in_three,
        summary.in_five,
        summary.mean_chars,
        summary.mean_ms
    );

    Ok(ExitCode::SUCCESS)
}

async fn preserve(args: EvalPreserveArgs) -> Result<ExitCode> {
    let trajectories = preserve::trajectories_under(&args.runs);

    if trajectories.is_empty() {
        bail!(
            "no trajectory logs (`.jsonl`) under {}",
            args.runs.display()
        );
    }

    let provider = session::build_provider(&args.model)?;

    let mask = MaskConfig {
        keep_steps: args.keep_steps,
        ..MaskConfig::default()
    };

    let tools = cortex_tools::default_registry().specs();

    println!(
        "Asking {} at up to {} steps of each of {} saved runs, twice each\n",
        provider.model(),
        args.samples,
        trajectories.len()
    );

    let preserved = preserve::measure(
        provider,
        &tools,
        &trajectories,
        &mask,
        args.samples,
        |so_far| {
            println!(
                "  {:>3} steps: same step when shortened {}, full agrees with the run {}",
                so_far.samples, so_far.same_when_shortened, so_far.full_same_as_run
            );
        },
    )
    .await?;

    if preserved.samples == 0 {
        println!(
            "No step of these runs is long enough for masking to remove anything (it keeps the \
             last {} steps). Try a lower `--keep-steps`, or longer runs.",
            args.keep_steps
        );

        return Ok(ExitCode::SUCCESS);
    }

    let share = |count: usize| 100.0 * count as f64 / preserved.samples as f64;

    println!(
        "\nOver {} steps, masking removed {:.0}% of the conversation.\n\
         - Same next step with the shortened conversation as with the full one: {:.0}%\n\
         - Full conversation, same step as the run took (how often the model agrees with itself): {:.0}%\n\
         - Shortened conversation, same step as the run took: {:.0}%",
        preserved.samples,
        preserved.share_removed() * 100.0,
        share(preserved.same_when_shortened),
        share(preserved.full_same_as_run),
        share(preserved.shortened_same_as_run),
    );

    Ok(ExitCode::SUCCESS)
}
