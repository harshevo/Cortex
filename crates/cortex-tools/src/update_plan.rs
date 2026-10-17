use async_trait::async_trait;
use cortex_core::plan::{self, Plan, Status};
use cortex_core::{Access, Tool, ToolContext, ToolError, ToolSpec};
use serde_json::{Value, json};

/// Records the run's plan. See [`cortex_core::plan`].
pub struct UpdatePlan;

#[async_trait]
impl Tool for UpdatePlan {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: plan::TOOL.to_string(),

            description: "Record the plan for the task: its steps with their status, and \
                          the acceptance conditions that must be shown to hold for the task \
                          to be done. Each call replaces the whole plan. The person sees it \
                          as a task list. Steps can depend on other steps with `depends_on`; \
                          a step cannot start before those are completed, and circular or \
                          unknown dependencies are refused."
                .to_string(),

            input_schema: json!({
                "type": "object",
                "properties": {
                    "goal": {
                        "type": "string",
                        "description": "What the task is for, in one line.",
                    },
                    "steps": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "step": { "type": "string", "description": "A short imperative phrase." },
                                "status": {
                                    "type": "string",
                                    "enum": ["pending", "in_progress", "completed", "blocked"],
                                },
                                "id": {
                                    "type": "string",
                                    "description": "Optional short name other steps can depend on. Not a number.",
                                },
                                "depends_on": {
                                    "type": "array",
                                    "items": { "type": "string" },
                                    "description": "Steps that must be completed first, by id or by step number (from 1).",
                                },
                                "job": {
                                    "type": "string",
                                    "description": "The background job carrying this step out, if one does.",
                                },
                            },
                            "required": ["step", "status"],
                        },
                    },
                    "acceptance": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Checkable conditions for the task to be done.",
                    },
                },
                "required": ["steps"],
            }),
        }
    }

    fn access(&self) -> Access {
        Access::ReadOnly
    }

    fn describe(&self, input: &Value) -> String {
        match Plan::from_input(input) {
            Some(plan) => format!("plan: {} of {} done", plan.completed(), plan.steps.len()),
            None => "plan".to_string(),
        }
    }

    async fn run(&self, input: Value, _context: &ToolContext) -> Result<String, ToolError> {
        let plan = Plan::from_input(&input).ok_or_else(|| {
            ToolError::InvalidInput(
                "expected `steps`: a list of { step, status } with status one of pending, \
                 in_progress, completed, blocked"
                    .to_string(),
            )
        })?;

        if plan.steps.is_empty() {
            return Err(ToolError::InvalidInput(
                "a plan needs at least one step".to_string(),
            ));
        }

        plan.validate().map_err(ToolError::InvalidInput)?;

        let working: Vec<_> = plan
            .steps
            .iter()
            .filter(|step| step.status == Status::InProgress)
            .collect();

        let mut text = plan.render();

        // Steps in progress together are independent once the plan is
        // valid; doing them at once only works when jobs carry them out.
        if working.len() > 1 && working.iter().any(|step| step.job.is_none()) {
            text.push_str(
                "\nNote: keep one step in progress at a time, unless the others are \
                 carried out by background jobs.",
            );
        }

        Ok(text)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use cortex_core::{AutoApprove, Workspace};

    use super::*;

    #[tokio::test]
    async fn a_plan_with_a_circular_dependency_is_refused() {
        let folder = tempfile::tempdir().unwrap();
        let context = ToolContext {
            workspace: Workspace::new(folder.path()).unwrap(),
            approver: Arc::new(AutoApprove),
        };

        let refused = UpdatePlan
            .run(
                json!({ "steps": [
                    { "step": "A", "status": "pending", "id": "a", "depends_on": ["b"] },
                    { "step": "B", "status": "pending", "id": "b", "depends_on": ["a"] },
                ]}),
                &context,
            )
            .await;

        assert!(matches!(refused, Err(ToolError::InvalidInput(_))));

        let accepted = UpdatePlan
            .run(
                json!({ "steps": [
                    { "step": "A", "status": "completed", "id": "a" },
                    { "step": "B", "status": "in_progress", "depends_on": ["a"] },
                ]}),
                &context,
            )
            .await
            .unwrap();

        assert!(accepted.contains("(after 1)"));
    }
}
