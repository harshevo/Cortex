//! The `skill` tool: the model's own way to a skill.

use async_trait::async_trait;
use cortex_core::{Access, Tool, ToolContext, ToolError, ToolSpec};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::SkillLibrary;

/// Returns the text of a skill the model asks for by name.
#[derive(Debug, Clone, Copy, Default)]
pub struct UseSkill;

#[derive(Deserialize)]
struct Input {
    name: String,

    #[serde(default)]
    arguments: String,
}

#[async_trait]
impl Tool for UseSkill {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "skill".to_string(),

            description: "Load a skill: the written procedure for a kind of task, to follow \
                          for the rest of the task. Call it before starting work that one of \
                          the listed skills covers."
                .to_string(),

            input_schema: json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "The skill's name, as listed.",
                    },
                    "arguments": {
                        "type": "string",
                        "description": "What the skill should act on, such as a path or a \
                                        pull request number.",
                    },
                },
                "required": ["name"],
            }),
        }
    }

    fn access(&self) -> Access {
        Access::ReadOnly
    }

    async fn run(&self, input: Value, context: &ToolContext) -> Result<String, ToolError> {
        let input: Input = serde_json::from_value(input)
            .map_err(|error| ToolError::InvalidInput(error.to_string()))?;

        let library = SkillLibrary::for_project(context.workspace.root());

        let name = input.name.trim().trim_start_matches('/');

        let Some(skill) = library.get(name) else {
            return Err(ToolError::InvalidInput(format!(
                "there is no skill named `{name}`. The skills are: {}",
                library.names().join(", ")
            )));
        };

        Ok(skill.render(&input.arguments))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use cortex_core::{AutoApprove, Workspace};

    use super::*;

    fn context(root: &std::path::Path) -> ToolContext {
        ToolContext {
            workspace: Workspace::new(root).unwrap(),
            approver: Arc::new(AutoApprove),
        }
    }

    #[tokio::test]
    async fn a_named_skill_is_returned_with_its_arguments() {
        let project = tempfile::tempdir().unwrap();

        let text = UseSkill
            .run(
                json!({"name": "/security-review", "arguments": "src/auth"}),
                &context(project.path()),
            )
            .await
            .unwrap();

        assert!(text.contains("Arguments: src/auth"));
        assert!(text.contains("Trace"));
    }

    #[tokio::test]
    async fn an_unknown_name_lists_the_skills_there_are() {
        let project = tempfile::tempdir().unwrap();

        let error = UseSkill
            .run(json!({"name": "nonesuch"}), &context(project.path()))
            .await
            .unwrap_err();

        assert!(error.to_string().contains("code-review"));
    }
}
