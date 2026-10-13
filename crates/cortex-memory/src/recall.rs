//! Choosing which memories to show for a task, and writing them out.

use serde_json::{Value, json};

use crate::episode::Episode;
use crate::fact::{Fact, FactKind};
use crate::lesson::StoredLesson;

/// Facts shown at most. They are repeated on every model call of the task,
/// so the list has to stay short.
const MAX_FACTS: usize = 8;

/// An episode is shown only if it is at least this close to the best one.
const RELATIVE_FLOOR: f32 = 0.6;

/// Lessons shown for one task at most.
pub const MAX_LESSONS: usize = 3;

/// Episodes, and lessons, from other projects shown for one task at most.
/// They are about different code, so only the closest earn their place.
pub const MAX_ELSEWHERE: usize = 2;

/// Checked tasks needed before the track record says anything.
const MIN_CHECKED_TASKS: usize = 5;

#[derive(Debug, Clone)]
pub struct ScoredLesson {
    pub lesson: StoredLesson,

    /// Similarity to the task, higher is closer.
    pub score: f32,
}

/// How earlier tasks in the repository went, as numbers the model can
/// weigh the cost of a check against.
///
/// A model decides whether to test before finishing with no idea how
/// often its first answer is right here. Telling it is the idea of
/// "Calibrate-Then-Act" (Ding, Tomlin and Durrett, 2026): give the prior,
/// and let the model do the weighing.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackRecord {
    /// Tasks whose result the verification gate checked.
    pub checked: usize,

    /// Of those, the ones whose first "done" passed.
    pub passed_first_time: usize,

    /// Of those, the ones that never passed.
    pub never_passed: usize,

    /// Median model calls per task.
    pub median_steps: u32,
}

impl TrackRecord {
    pub fn from_episodes(episodes: &[Episode]) -> Option<Self> {
        let checked: Vec<&Episode> = episodes
            .iter()
            .filter(|episode| episode.verified.is_some())
            .collect();

        if checked.len() < MIN_CHECKED_TASKS {
            return None;
        }

        let mut steps: Vec<u32> = checked.iter().map(|episode| episode.steps).collect();

        steps.sort_unstable();

        Some(Self {
            checked: checked.len(),
            passed_first_time: checked
                .iter()
                .filter(|episode| episode.verified == Some(true) && episode.rejections == 0)
                .count(),
            never_passed: checked
                .iter()
                .filter(|episode| episode.verified == Some(false))
                .count(),
            median_steps: steps[steps.len() / 2],
        })
    }

    pub fn first_time_rate(&self) -> f64 {
        self.passed_first_time as f64 / self.checked as f64
    }

    pub fn render(&self) -> String {
        let percent = (self.first_time_rate() * 100.0).round() as u32;

        let advice = if self.first_time_rate() >= 0.8 {
            "A first answer is usually right here, so one focused check before finishing is              enough."
        } else {
            "A first answer is often wrong here. Reproduce the problem before the fix and run              a focused check after it; that costs less than being sent back."
        };

        format!(
            "Track record in this repository: of {} checked tasks, {} ({percent}%) passed the              project's checks the first time the agent said it was done, and {} never passed.              A task took a median of {} model calls. {advice}",
            self.checked, self.passed_first_time, self.never_passed, self.median_steps
        )
    }
}

#[derive(Debug, Clone)]
pub struct ScoredEpisode {
    pub episode: Episode,

    /// Similarity to the task, higher is closer.
    pub score: f32,
}

/// What memory has to say about one task.
#[derive(Debug, Clone, Default)]
pub struct Recall {
    /// Most similar first.
    pub episodes: Vec<ScoredEpisode>,

    pub facts: Vec<Fact>,

    /// Advice taken from earlier runs, closest to the task first.
    pub lessons: Vec<ScoredLesson>,

    /// The closest tasks from other projects, each with the project's
    /// name, when memory is shared between projects.
    pub episodes_elsewhere: Vec<(String, ScoredEpisode)>,

    /// Advice from other projects that is not about any one file.
    pub lessons_elsewhere: Vec<(String, ScoredLesson)>,

    /// How earlier tasks here went, when that is switched on and enough
    /// of them were checked.
    pub track_record: Option<TrackRecord>,

    /// How similarity was measured: `embedding` or `lexical`.
    pub method: &'static str,

    /// Whether a learned reranker reordered the episodes.
    pub reranked: bool,
}

impl Recall {
    pub fn is_empty(&self) -> bool {
        self.episodes.is_empty()
            && self.facts.is_empty()
            && self.lessons.is_empty()
            && self.track_record.is_none()
            && self.episodes_elsewhere.is_empty()
            && self.lessons_elsewhere.is_empty()
    }

    /// The memory as shown to the model. `exists` says whether a path is
    /// still in the project.
    pub fn render(&self, exists: impl Fn(&str) -> bool + Copy) -> String {
        let mut text = String::from(
            "## Memory of earlier work in this repository\n\n\
             These notes were recorded after earlier tasks here. Use them to skip exploring \
             what is already known, but they may be out of date: check a file before relying \
             on what a note says about it.\n",
        );

        let (overview, facts): (Vec<&Fact>, Vec<&Fact>) = self
            .facts
            .iter()
            .partition(|fact| fact.kind == FactKind::Overview);

        if let Some(overview) = overview.first() {
            text.push_str(&format!("\nAbout this project:\n\n{}\n", overview.text));
        }

        if !facts.is_empty() {
            text.push_str("\nKnown about this repository:\n");

            for fact in facts {
                text.push_str(&format!("- {}\n", fact.text));
            }
        }

        if let Some(record) = &self.track_record {
            text.push_str(&format!("\n{}\n", record.render()));
        }

        if !self.lessons.is_empty() {
            text.push_str("\nLessons from earlier tasks that may apply:\n");

            for scored in &self.lessons {
                text.push_str(&format!("- {}\n", scored.lesson.render()));
            }
        }

        if !self.episodes.is_empty() {
            text.push_str("\nEarlier tasks most like this one:\n\n");

            for (position, scored) in self.episodes.iter().enumerate() {
                text.push_str(&format!(
                    "{}. {}\n",
                    position + 1,
                    scored.episode.render(exists)
                ));
            }
        }

        if !self.episodes_elsewhere.is_empty() || !self.lessons_elsewhere.is_empty() {
            text.push_str(
                "\nFrom your other projects. The code there is different: take the approach, \
                 not the paths.\n",
            );

            for (project, scored) in &self.lessons_elsewhere {
                text.push_str(&format!("- In `{project}`: {}\n", scored.lesson.render()));
            }

            for (project, scored) in &self.episodes_elsewhere {
                // Its files are not in this project, so none is named.
                text.push_str(&format!(
                    "- In `{project}`: {}\n",
                    scored.episode.render(|_| false)
                ));
            }
        }

        text
    }

    /// What was shown and why, for the trajectory log.
    pub fn detail(&self) -> Value {
        let episodes: Vec<Value> = self
            .episodes
            .iter()
            .map(|scored| {
                json!({
                    "id": scored.episode.id,
                    "score": scored.score,
                    "instruction": scored.episode.instruction,
                    "verified": scored.episode.verified,
                })
            })
            .collect();

        let facts: Vec<Value> = self
            .facts
            .iter()
            .map(|fact| json!({ "kind": fact.kind.label(), "key": fact.key }))
            .collect();

        let lessons: Vec<Value> = self
            .lessons
            .iter()
            .map(|scored| {
                json!({
                    "id": scored.lesson.id,
                    "score": scored.score,
                    "title": scored.lesson.lesson.title,
                    "from_failure": scored.lesson.lesson.from_failure,
                })
            })
            .collect();

        let track_record = self.track_record.as_ref().map(|record| {
            json!({
                "checked": record.checked,
                "passed_first_time": record.passed_first_time,
                "never_passed": record.never_passed,
            })
        });

        json!({
            "method": self.method,
            "reranked": self.reranked,
            "episodes": episodes,
            "facts": facts,
            "lessons": lessons,
            "elsewhere": self
                .episodes_elsewhere
                .iter()
                .map(|(project, scored)| {
                    json!({
                        "project": project,
                        "instruction": scored.episode.instruction,
                        "score": scored.score,
                    })
                })
                .collect::<Vec<Value>>(),
            "lessons_elsewhere": self
                .lessons_elsewhere
                .iter()
                .map(|(project, scored)| json!({ "project": project, "title": scored.lesson.lesson.title }))
                .collect::<Vec<Value>>(),
            "track_record": track_record,
        })
    }
}

/// Keeps the best `limit` episodes that clear `floor` and are not far
/// behind the best one.
pub fn select_episodes(
    mut scored: Vec<ScoredEpisode>,
    limit: usize,
    floor: f32,
) -> Vec<ScoredEpisode> {
    scored.sort_by(|left, right| right.score.total_cmp(&left.score));

    let Some(best) = scored.first().map(|first| first.score) else {
        return scored;
    };

    scored.retain(|candidate| candidate.score >= floor && candidate.score >= best * RELATIVE_FLOOR);
    scored.truncate(limit);

    scored
}

/// Keeps the best few of what other projects hold: those that clear
/// `floor` and are not far behind the best one.
pub fn select_elsewhere<T>(
    mut found: Vec<(String, T)>,
    score: impl Fn(&T) -> f32,
    floor: f32,
) -> Vec<(String, T)> {
    found.sort_by(|left, right| score(&right.1).total_cmp(&score(&left.1)));

    let Some(best) = found.first().map(|first| score(&first.1)) else {
        return found;
    };

    found.retain(|(_, item)| score(item) >= floor && score(item) >= best * RELATIVE_FLOOR);
    found.truncate(MAX_ELSEWHERE);

    found
}

/// Keeps the best lessons that clear `floor`. A lesson seen in several
/// runs is preferred among equals.
pub fn select_lessons(mut scored: Vec<ScoredLesson>, floor: f32) -> Vec<ScoredLesson> {
    scored.retain(|candidate| candidate.score >= floor);

    scored.sort_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then(right.lesson.support.cmp(&left.lesson.support))
    });

    scored.truncate(MAX_LESSONS);

    scored
}

/// Orders facts by kind, then by how many episodes support them.
pub fn select_facts(mut facts: Vec<Fact>) -> Vec<Fact> {
    facts.sort_by(|left, right| {
        (
            left.kind.priority(),
            std::cmp::Reverse(left.support),
            &left.key,
        )
            .cmp(&(
                right.kind.priority(),
                std::cmp::Reverse(right.support),
                &right.key,
            ))
    });

    facts.truncate(MAX_FACTS);

    facts
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::working::WorkingMemory;

    fn scored(instruction: &str, score: f32) -> ScoredEpisode {
        let memory = WorkingMemory {
            instruction: instruction.to_string(),
            files_changed: vec!["a.ts".to_string()],
            ..WorkingMemory::default()
        };

        ScoredEpisode {
            episode: Episode::from_working(&memory, 0),
            score,
        }
    }

    #[test]
    fn weak_matches_are_left_out() {
        let candidates = vec![
            scored("close", 0.8),
            scored("unrelated", 0.2),
            scored("near", 0.6),
            scored("below the floor", 0.05),
        ];

        let chosen = select_episodes(candidates, 3, 0.1);

        let names: Vec<&str> = chosen
            .iter()
            .map(|scored| scored.episode.instruction.as_str())
            .collect();

        assert_eq!(names, ["close", "near"]);
    }

    #[test]
    fn the_briefing_lists_facts_then_episodes() {
        let recall = Recall {
            episodes: vec![scored("fix the calendar", 0.8)],
            episodes_elsewhere: Vec::new(),
            lessons_elsewhere: Vec::new(),
            facts: vec![Fact {
                kind: FactKind::CheckCommand,
                key: "npm test".to_string(),
                text: "`npm test` works here.".to_string(),
                paths: Vec::new(),
                support: 2,
                updated_ms: 0,
            }],
            lessons: vec![ScoredLesson {
                lesson: StoredLesson::new(
                    cortex_core::Lesson {
                        title: "Week boundaries".to_string(),
                        when: "a day lands in the wrong week".to_string(),
                        advice: "Check the time zone conversion first.".to_string(),
                        paths: Vec::new(),
                        from_failure: false,
                    },
                    0,
                ),
                score: 0.7,
            }],
            track_record: None,
            method: "lexical",
            reranked: false,
        };

        let text = recall.render(|_| true);

        let facts_at = text.find("`npm test` works here.").unwrap();
        let lesson_at = text.find("- **Week boundaries** When a day lands").unwrap();
        let episode_at = text.find("1. \"fix the calendar\"").unwrap();

        assert!(facts_at < lesson_at && lesson_at < episode_at);
        assert_eq!(recall.detail()["lessons"][0]["title"], "Week boundaries");
        assert_eq!(recall.detail()["episodes"][0]["score"], 0.800000011920929);
    }

    #[test]
    fn the_track_record_needs_enough_checked_tasks() {
        let episode = |verified: Option<bool>, rejections: u32, steps: u32| Episode {
            verified,
            rejections,
            steps,
            ..scored("a task", 0.0).episode
        };

        let mut episodes = vec![
            episode(Some(true), 0, 8),
            episode(Some(true), 1, 14),
            episode(Some(false), 2, 30),
            episode(None, 0, 5),
        ];

        assert_eq!(TrackRecord::from_episodes(&episodes), None);

        episodes.push(episode(Some(true), 0, 10));
        episodes.push(episode(Some(true), 0, 9));

        let record = TrackRecord::from_episodes(&episodes).unwrap();

        assert_eq!(
            record,
            TrackRecord {
                checked: 5,
                passed_first_time: 3,
                never_passed: 1,
                median_steps: 10,
            }
        );

        assert!(
            record
                .render()
                .contains("of 5 checked tasks, 3 (60%) passed")
        );
        assert!(record.render().contains("often wrong here"));
    }
}
