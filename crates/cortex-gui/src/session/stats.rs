//! The numbers under the composer.

use cortex_core::Usage;

/// How many recent model calls the speed sparkline shows.
pub const SPARKLINE_POINTS: usize = 24;

/// One square of the loop strip: something the agent did in this run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cell {
    /// The model is working on a response.
    Thinking,

    /// The model responded.
    Model,

    /// A tool ran and its output went back to the model.
    Observation,

    /// A tool failed; the error went back to the model.
    FailedObservation,

    /// The project's checks ran.
    Check { passed: bool },
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Stats {
    /// Output tokens per second of each model call, oldest first.
    pub speeds: Vec<f32>,

    /// Tokens used since the session began.
    pub usage: Usage,

    /// The loop of the current run.
    pub cells: Vec<Cell>,

    /// Model calls made in the current run.
    pub step: u32,

    /// Repeated lookups answered with one line because nothing had
    /// changed, since the session began.
    pub repeats_folded: u32,

    /// Characters of superseded tool output taken out of the conversation
    /// since the session began.
    pub chars_pruned: u64,

    /// Characters of old tool output masked since the session began.
    pub chars_masked: u64,

    /// Times a run was carried on after a provider failure.
    pub resumes: u32,

    /// Tokens of the current run already added to `usage`, one model
    /// call at a time.
    run_counted: Usage,
}

impl Stats {
    pub fn run_started(&mut self) {
        self.cells.clear();
        self.step = 0;
        self.run_counted = Usage::default();
    }

    pub fn model_started(&mut self, step: u32) {
        self.step = step;
        self.cells.push(Cell::Thinking);
    }

    pub fn model_responded(&mut self, step: u32, usage: Usage, duration_ms: u64) {
        self.step = step;
        self.usage += usage;
        self.run_counted += usage;

        if let Some(speed) = tokens_per_second(usage.output_tokens, duration_ms) {
            self.speeds.push(speed);
        }

        // The response replaces the "thinking" square that announced it.
        // Logs from before that event existed have none to replace.
        match self.cells.last_mut() {
            Some(cell @ Cell::Thinking) => *cell = Cell::Model,
            _ => self.cells.push(Cell::Model),
        }
    }

    pub fn repeat_folded(&mut self) {
        self.repeats_folded += 1;
    }

    pub fn pruned(&mut self, chars: usize) {
        self.chars_pruned += chars as u64;
    }

    pub fn masked(&mut self, chars: usize) {
        self.chars_masked += chars as u64;
    }

    pub fn resumed(&mut self) {
        self.resumes += 1;
    }

    /// What the engine has spared the model so far, in words. `None` when
    /// there is nothing to report.
    pub fn savings(&self) -> Option<String> {
        let mut parts = Vec::new();

        if self.repeats_folded > 0 {
            parts.push(format!(
                "{} repeated lookups answered in one line",
                self.repeats_folded
            ));
        }

        if self.chars_pruned > 0 {
            parts.push(format!(
                "{} characters of superseded output removed",
                self.chars_pruned
            ));
        }

        if self.chars_masked > 0 {
            parts.push(format!(
                "{} characters of old output masked",
                self.chars_masked
            ));
        }

        if self.resumes > 0 {
            parts.push(format!(
                "carried on {} times after a provider failure",
                self.resumes
            ));
        }

        (!parts.is_empty()).then(|| parts.join("; "))
    }

    pub fn tool_finished(&mut self, is_error: bool) {
        self.cells.push(if is_error {
            Cell::FailedObservation
        } else {
            Cell::Observation
        });
    }

    pub fn verification_ran(&mut self, passed: bool) {
        self.cells.push(Cell::Check { passed });
    }

    /// A run that ended in an error leaves its last request unanswered.
    pub fn run_ended(&mut self) {
        self.cells.retain(|cell| *cell != Cell::Thinking);
    }

    /// A run reported what it used in all. That can be more than its
    /// steps added up to: a review, the lessons, and attempts that were
    /// not kept are model calls too. The difference is added here.
    pub fn run_used(&mut self, total: Usage) {
        let counted = self.run_counted;

        self.usage += Usage {
            input_tokens: total.input_tokens.saturating_sub(counted.input_tokens),
            output_tokens: total.output_tokens.saturating_sub(counted.output_tokens),
            cache_read_tokens: total
                .cache_read_tokens
                .saturating_sub(counted.cache_read_tokens),
            cache_write_tokens: total
                .cache_write_tokens
                .saturating_sub(counted.cache_write_tokens),
        };

        self.run_counted = total;
    }

    /// Speed of the most recent model call.
    pub fn speed(&self) -> Option<f32> {
        self.speeds.last().copied()
    }

    pub fn recent_speeds(&self) -> &[f32] {
        let start = self.speeds.len().saturating_sub(SPARKLINE_POINTS);

        &self.speeds[start..]
    }

    /// Share of input tokens the provider served from its cache, from 0 to
    /// 1. `None` before any input has been sent.
    pub fn cache_hit_ratio(&self) -> Option<f32> {
        let total = self.usage.total_input();

        if total == 0 {
            return None;
        }

        Some(self.usage.cache_read_tokens as f32 / total as f32)
    }

    pub fn observations(&self) -> usize {
        self.cells
            .iter()
            .filter(|cell| matches!(cell, Cell::Observation | Cell::FailedObservation))
            .count()
    }
}

pub fn tokens_per_second(output_tokens: u64, duration_ms: u64) -> Option<f32> {
    if output_tokens == 0 || duration_ms == 0 {
        return None;
    }

    Some(output_tokens as f32 * 1000.0 / duration_ms as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(input: u64, cached: u64, output: u64) -> Usage {
        Usage {
            input_tokens: input,
            output_tokens: output,
            cache_read_tokens: cached,
            cache_write_tokens: 0,
        }
    }

    #[test]
    fn speed_is_output_tokens_over_call_time() {
        let mut stats = Stats::default();

        stats.model_started(1);
        stats.model_responded(1, usage(100, 0, 90), 2_000);

        assert_eq!(stats.speed(), Some(45.0));
    }

    #[test]
    fn a_call_without_timing_has_no_speed() {
        assert_eq!(tokens_per_second(90, 0), None);
        assert_eq!(tokens_per_second(0, 500), None);
    }

    #[test]
    fn cache_ratio_counts_cached_input_against_all_input() {
        let mut stats = Stats::default();

        assert_eq!(stats.cache_hit_ratio(), None);

        stats.model_responded(1, usage(1_000, 0, 10), 100);
        assert_eq!(stats.cache_hit_ratio(), Some(0.0));

        stats.model_responded(2, usage(250, 1_750, 10), 100);
        assert_eq!(stats.cache_hit_ratio(), Some(1_750.0 / 3_000.0));
    }

    #[test]
    fn savings_are_reported_only_when_there_are_some() {
        let mut stats = Stats::default();

        assert_eq!(stats.savings(), None);

        stats.repeat_folded();
        stats.pruned(4_200);

        assert_eq!(
            stats.savings().unwrap(),
            "1 repeated lookups answered in one line; \
             4200 characters of superseded output removed"
        );
    }

    #[test]
    fn the_loop_strip_follows_the_run() {
        let mut stats = Stats::default();

        stats.run_started();
        stats.model_started(1);
        assert_eq!(stats.cells, [Cell::Thinking]);

        stats.model_responded(1, usage(10, 0, 5), 100);
        stats.tool_finished(false);
        stats.tool_finished(true);
        stats.model_started(2);
        stats.run_ended();

        assert_eq!(
            stats.cells,
            [Cell::Model, Cell::Observation, Cell::FailedObservation]
        );

        assert_eq!(stats.observations(), 2);
    }
}
