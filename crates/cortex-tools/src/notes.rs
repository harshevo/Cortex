use async_trait::async_trait;
use cortex_core::notes::{Kind, Notes};
use cortex_core::{Access, Tool, ToolContext, ToolError, ToolSpec};
use serde::Deserialize;
use serde_json::{Value, json};

use crate::input;

/// Records a fact about the project for later sessions. See
/// [`cortex_core::notes`].
pub struct SaveNote;

/// Retires a note that turned out to be wrong.
pub struct DropNote;

#[derive(Deserialize)]
struct SaveInput {
    text: String,

    #[serde(default)]
    kind: Kind,

    #[serde(default)]
    files: Vec<String>,

    #[serde(default)]
    replaces: Option<u32>,
}

#[derive(Deserialize)]
struct DropInput {
    id: u32,
}

#[async_trait]
impl Tool for SaveNote {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "save_note".to_string(),

            description: "Record one durable fact about this project for later sessions: a \
                          convention, how parts fit together, a decision and its reason, a \
                          procedure that works, or a pitfall. Not for the progress of the \
                          current task."
                .to_string(),

            input_schema: json!({
                "type": "object",
                "properties": {
                    "text": { "type": "string", "description": "The fact, in a sentence or two." },
                    "kind": {
                        "type": "string",
                        "enum": ["convention", "architecture", "decision", "procedure", "pitfall"],
                    },
                    "files": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Files the fact is about, relative to the project. When they change, the note is shown as possibly out of date.",
                    },
                    "replaces": {
                        "type": "integer",
                        "description": "The id of a note this one corrects.",
                    },
                },
                "required": ["text", "kind"],
            }),
        }
    }

    fn access(&self) -> Access {
        Access::ReadOnly
    }

    fn describe(&self, input: &Value) -> String {
        let text = input.get("text").and_then(Value::as_str).unwrap_or("?");

        format!("note: {text}")
    }

    async fn run(&self, input: Value, context: &ToolContext) -> Result<String, ToolError> {
        let input: SaveInput = input::parse(input)?;

        if input.text.trim().is_empty() {
            return Err(ToolError::InvalidInput("the note is empty".to_string()));
        }

        // Only files inside the project can be followed.
        let mut files = Vec::new();

        for file in &input.files {
            let path = context.workspace.resolve(file)?;

            files.push(context.workspace.relative(&path));
        }

        let notes = Notes::for_project(context.workspace.root());

        let id = notes
            .save(input.kind, &input.text, &files, "", input.replaces)
            .map_err(|error| ToolError::Failed(format!("could not save the note: {error}")))?;

        Ok(format!("saved as note {id}"))
    }
}

#[async_trait]
impl Tool for DropNote {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "drop_note".to_string(),

            description: "Remove a project note that is wrong or no longer true, by its id."
                .to_string(),

            input_schema: json!({
                "type": "object",
                "properties": {
                    "id": { "type": "integer" },
                },
                "required": ["id"],
            }),
        }
    }

    fn access(&self) -> Access {
        Access::ReadOnly
    }

    async fn run(&self, input: Value, context: &ToolContext) -> Result<String, ToolError> {
        let input: DropInput = input::parse(input)?;

        let notes = Notes::for_project(context.workspace.root());

        match notes.drop(input.id) {
            Ok(true) => Ok(format!("removed note {}", input.id)),
            Ok(false) => Err(ToolError::InvalidInput(format!(
                "there is no note {}",
                input.id
            ))),
            Err(error) => Err(ToolError::Failed(format!(
                "could not remove the note: {error}"
            ))),
        }
    }
}
