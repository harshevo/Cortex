//! The task plan a run keeps for itself.
//!
//! The model writes its plan with the `update_plan` tool: the steps, how
//! far each has got, and what has to be true for the task to count as
//! done. Each call replaces the whole plan. Because it is an ordinary
//! tool call, it is in the session's log, so the plan is still there when
//! a session is taken up again after a restart, and the interfaces can
//! show the latest one beside the conversation.
//!
//! Steps may depend on one another. A step names the steps it waits for
//! in `depends_on`, by their `id` or by their number counted from 1. A
//! plan whose dependencies name no step, go round in a circle, or have a
//! step started before what it waits for is finished, is refused, so the
//! order the model writes down is one that can be followed. Steps whose
//! dependencies are all done are ready, and ready steps that do not
//! depend on each other can be carried out at the same time, for
//! instance as background jobs.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The name of the tool the plan is written with.
pub const TOOL: &str = "update_plan";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    #[default]
    Pending,
    InProgress,
    Completed,
    Blocked,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Step {
    pub step: String,

    #[serde(default)]
    pub status: Status,

    /// A short name other steps can depend on. Without one, the step is
    /// known by its number, counted from 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,

    /// The steps that must be completed before this one starts.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<String>,

    /// The background job carrying the step out, when one does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub job: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plan {
    /// What the task is for, in a line.
    #[serde(default)]
    pub goal: String,

    pub steps: Vec<Step>,

    /// What has to be true, and shown, for the task to be done.
    #[serde(default)]
    pub acceptance: Vec<String>,
}

impl Plan {
    /// The plan in a call to [`TOOL`], if the input is one.
    pub fn from_input(input: &Value) -> Option<Self> {
        serde_json::from_value(input.clone()).ok()
    }

    pub fn completed(&self) -> usize {
        self.steps
            .iter()
            .filter(|step| step.status == Status::Completed)
            .count()
    }

    pub fn current(&self) -> Option<&Step> {
        self.steps
            .iter()
            .find(|step| step.status == Status::InProgress)
    }

    pub fn is_finished(&self) -> bool {
        !self.steps.is_empty() && self.completed() == self.steps.len()
    }

    /// The name the step at `index` is known by: its id, or its number.
    pub fn key(&self, index: usize) -> String {
        match self.steps.get(index).and_then(|step| step.id.as_deref()) {
            Some(id) if !id.trim().is_empty() => id.trim().to_string(),
            _ => (index + 1).to_string(),
        }
    }

    /// The step a dependency names: by id first, then by number.
    pub fn resolve(&self, name: &str) -> Option<usize> {
        let name = name.trim();

        let by_id = self
            .steps
            .iter()
            .position(|step| step.id.as_deref().map(str::trim) == Some(name));

        by_id.or_else(|| {
            let number: usize = name.trim_start_matches('#').parse().ok()?;

            (1..=self.steps.len()).contains(&number).then(|| number - 1)
        })
    }

    /// The steps the step at `index` waits for, as indices. Names that
    /// match no step are left out; [`Self::validate`] reports them.
    pub fn dependencies(&self, index: usize) -> Vec<usize> {
        let mut found: Vec<usize> = self.steps[index]
            .depends_on
            .iter()
            .filter_map(|name| self.resolve(name))
            .collect();

        found.sort_unstable();
        found.dedup();

        found
    }

    /// The dependencies of the step at `index` that are not completed.
    pub fn waiting_on(&self, index: usize) -> Vec<usize> {
        self.dependencies(index)
            .into_iter()
            .filter(|&dependency| self.steps[dependency].status != Status::Completed)
            .collect()
    }

    /// Pending steps whose dependencies are all completed, in order.
    pub fn ready(&self) -> Vec<usize> {
        (0..self.steps.len())
            .filter(|&index| {
                self.steps[index].status == Status::Pending && self.waiting_on(index).is_empty()
            })
            .collect()
    }

    /// Whether the dependencies form a plan that can be followed: every
    /// name matches one step, ids are not given twice, no step waits on
    /// itself through any chain, and no step has started before what it
    /// waits for is completed.
    pub fn validate(&self) -> Result<(), String> {
        let mut seen = std::collections::BTreeSet::new();

        for (index, step) in self.steps.iter().enumerate() {
            if let Some(id) = step.id.as_deref().map(str::trim).filter(|id| !id.is_empty()) {
                if !seen.insert(id.to_string()) {
                    return Err(format!("the id `{id}` is given to more than one step"));
                }

                // A numeric id would make a number mean two steps.
                if id.trim_start_matches('#').parse::<usize>().is_ok() {
                    return Err(format!(
                        "step {}: an id cannot be a number; numbers already name steps",
                        index + 1
                    ));
                }
            }
        }

        for (index, step) in self.steps.iter().enumerate() {
            for name in &step.depends_on {
                match self.resolve(name) {
                    None => {
                        return Err(format!(
                            "step {} depends on `{}`, which is not a step's id or number",
                            index + 1,
                            name.trim()
                        ));
                    }

                    Some(dependency) if dependency == index => {
                        return Err(format!("step {} depends on itself", index + 1));
                    }

                    Some(_) => {}
                }
            }
        }

        if let Some(cycle) = self.cycle() {
            let names: Vec<String> = cycle.iter().map(|&index| self.key(index)).collect();

            return Err(format!(
                "the dependencies go round in a circle: {} -> {}",
                names.join(" -> "),
                names[0]
            ));
        }

        for (index, step) in self.steps.iter().enumerate() {
            if !matches!(step.status, Status::InProgress | Status::Completed) {
                continue;
            }

            if let Some(&first) = self.waiting_on(index).first() {
                let verb = match step.status {
                    Status::Completed => "is completed",
                    _ => "is in progress",
                };

                return Err(format!(
                    "step {} {verb} but depends on step {}, which is not completed",
                    index + 1,
                    first + 1
                ));
            }
        }

        Ok(())
    }

    /// One circle in the dependencies, as the steps on it in order.
    fn cycle(&self) -> Option<Vec<usize>> {
        #[derive(Clone, Copy, PartialEq)]
        enum Mark {
            New,
            Open,
            Done,
        }

        fn visit(plan: &Plan, index: usize, marks: &mut [Mark], path: &mut Vec<usize>) -> bool {
            marks[index] = Mark::Open;
            path.push(index);

            for dependency in plan.dependencies(index) {
                let mark = marks[dependency];

                if mark == Mark::Open {
                    let start = path.iter().position(|&on| on == dependency).unwrap_or(0);

                    path.drain(..start);

                    return true;
                }

                if mark == Mark::New && visit(plan, dependency, marks, path) {
                    return true;
                }
            }

            path.pop();
            marks[index] = Mark::Done;

            false
        }

        let mut marks = vec![Mark::New; self.steps.len()];

        for index in 0..self.steps.len() {
            let mut path = Vec::new();

            if marks[index] == Mark::New && visit(self, index, &mut marks, &mut path) {
                return Some(path);
            }
        }

        None
    }

    /// The plan as the model is shown it back.
    pub fn render(&self) -> String {
        let mut text = String::new();

        if !self.goal.trim().is_empty() {
            text.push_str(&format!("Goal: {}\n", self.goal.trim()));
        }

        for (number, step) in self.steps.iter().enumerate() {
            let mark = match step.status {
                Status::Pending => "[ ]",
                Status::InProgress => "[>]",
                Status::Completed => "[x]",
                Status::Blocked => "[!]",
            };

            text.push_str(&format!("{mark} {}. {}", number + 1, step.step.trim()));

            if let Some(id) = step.id.as_deref().filter(|id| !id.trim().is_empty()) {
                text.push_str(&format!(" [{}]", id.trim()));
            }

            let after: Vec<String> = self
                .dependencies(number)
                .into_iter()
                .map(|dependency| (dependency + 1).to_string())
                .collect();

            if !after.is_empty() {
                text.push_str(&format!(" (after {})", after.join(", ")));
            }

            if let Some(job) = step.job.as_deref().filter(|job| !job.trim().is_empty()) {
                text.push_str(&format!(" (job {})", job.trim()));
            }

            text.push('\n');
        }

        let ready: Vec<String> = self
            .ready()
            .into_iter()
            .map(|index| (index + 1).to_string())
            .collect();

        if !ready.is_empty() && self.steps.iter().any(|step| !step.depends_on.is_empty()) {
            text.push_str(&format!("Ready to start: {}\n", ready.join(", ")));
        }

        if !self.acceptance.is_empty() {
            text.push_str("Done when:\n");

            for criterion in &self.acceptance {
                text.push_str(&format!("- {}\n", criterion.trim()));
            }
        }

        text.push_str(&format!(
            "{} of {} steps complete.",
            self.completed(),
            self.steps.len()
        ));

        text
    }
}

/// What the model is told about keeping a plan.
pub fn note() -> String {
    "Planning. For a task with more than two steps, call `update_plan` before you start: the \
     steps in order, and under `acceptance` the conditions that must hold, and be shown by \
     something you ran, for the task to be done. Keep exactly one step `in_progress`; call \
     it again whenever a step finishes, is blocked, or the plan changes, sending the whole \
     plan each time. When a step needs others done first, list them in its `depends_on` \
     (by `id` or step number); steps that are ready and independent can run in parallel \
     as background jobs (`spawn_job`), with the job id kept in the step's `job`. Before you report that you are done, check each acceptance condition \
     against evidence (a test, a build, the program's output, the diff) and say plainly \
     which ones you could not verify."
        .to_string()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_call_reads_back_as_a_plan() {
        let plan = Plan::from_input(&json!({
            "goal": "Fix login",
            "steps": [
                { "step": "Reproduce", "status": "completed" },
                { "step": "Fix", "status": "in_progress" },
                { "step": "Test" },
            ],
            "acceptance": ["login test passes"],
        }))
        .unwrap();

        assert_eq!(plan.completed(), 1);
        assert_eq!(plan.current().unwrap().step, "Fix");
        assert!(!plan.is_finished());

        let text = plan.render();

        assert!(text.contains("[x] 1. Reproduce"));
        assert!(text.contains("[>] 2. Fix"));
        assert!(text.contains("- login test passes"));
    }

    fn plan(steps: Value) -> Plan {
        Plan::from_input(&json!({ "steps": steps })).unwrap()
    }

    #[test]
    fn dependencies_resolve_by_id_or_number_and_decide_what_is_ready() {
        let plan = plan(json!([
            { "step": "Parse", "status": "completed", "id": "parse" },
            { "step": "Lex", "status": "pending" },
            { "step": "Check", "status": "pending", "id": "check", "depends_on": ["parse"] },
            { "step": "Emit", "status": "pending", "depends_on": ["2", "check"] },
        ]));

        assert_eq!(plan.validate(), Ok(()));
        assert_eq!(plan.dependencies(3), vec![1, 2]);
        assert_eq!(plan.ready(), vec![1, 2]);
        assert_eq!(plan.waiting_on(3), vec![1, 2]);

        let text = plan.render();

        assert!(text.contains("4. Emit (after 2, 3)"), "{text}");
        assert!(text.contains("Ready to start: 2, 3"), "{text}");
    }

    #[test]
    fn unknown_circular_and_premature_dependencies_are_refused() {
        let unknown = plan(json!([{ "step": "A", "status": "pending", "depends_on": ["x"] }]));
        assert!(unknown.validate().unwrap_err().contains("`x`"));

        let own = plan(json!([{ "step": "A", "status": "pending", "depends_on": ["1"] }]));
        assert!(own.validate().unwrap_err().contains("itself"));

        let circle = plan(json!([
            { "step": "A", "status": "pending", "id": "a", "depends_on": ["c"] },
            { "step": "B", "status": "pending", "id": "b", "depends_on": ["a"] },
            { "step": "C", "status": "pending", "id": "c", "depends_on": ["b"] },
        ]));
        assert!(circle.validate().unwrap_err().contains("circle"));

        let early = plan(json!([
            { "step": "A", "status": "pending" },
            { "step": "B", "status": "in_progress", "depends_on": ["1"] },
        ]));
        assert!(early.validate().unwrap_err().contains("step 2 is in progress"));

        let twice = plan(json!([
            { "step": "A", "status": "pending", "id": "a" },
            { "step": "B", "status": "pending", "id": "a" },
        ]));
        assert!(twice.validate().is_err());

        let numeric = plan(json!([{ "step": "A", "status": "pending", "id": "2" }]));
        assert!(numeric.validate().is_err());
    }

    #[test]
    fn a_plan_without_dependencies_reads_as_before() {
        let plain = plan(json!([{ "step": "A", "status": "pending" }]));

        assert_eq!(plain.validate(), Ok(()));
        assert!(!plain.render().contains("Ready"));
        assert_eq!(
            serde_json::to_value(&plain.steps[0]).unwrap(),
            json!({ "step": "A", "status": "pending" })
        );
    }
}
