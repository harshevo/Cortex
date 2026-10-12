use std::sync::Arc;

use async_trait::async_trait;
use cortex_core::{Access, Tool, ToolContext, ToolError, ToolSpec};
use cortex_index::CodeGraph;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::input;

/// Lists the tests that exercise the given files, by following imports.
pub struct AffectedTests {
    graph: Arc<CodeGraph>,
}

impl AffectedTests {
    pub fn new(graph: Arc<CodeGraph>) -> Self {
        Self { graph }
    }
}

#[derive(Deserialize)]
struct Input {
    paths: Vec<String>,
}

#[async_trait]
impl Tool for AffectedTests {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: "affected_tests".to_string(),

            description: "List the test files that import the given source files, directly or \
                          through other files. Use it after editing to run only the tests that \
                          can be affected by the change instead of the whole suite, and before \
                          editing to find the tests that describe how the code should behave. \
                          A test that reaches the code without importing it is not found."
                .to_string(),

            input_schema: json!({
                "type": "object",
                "properties": {
                    "paths": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "The files that were or will be changed, relative to the project directory.",
                    },
                },
                "required": ["paths"],
            }),
        }
    }

    fn access(&self) -> Access {
        Access::ReadOnly
    }

    async fn run(&self, input: Value, context: &ToolContext) -> Result<String, ToolError> {
        let input: Input = input::parse(input)?;

        let workspace = &context.workspace;

        let changed = input
            .paths
            .iter()
            .map(|path| workspace.resolve(path))
            .collect::<Result<Vec<_>, _>>()?;

        let tests = self.graph.tests_affected_by(workspace.root(), &changed);

        if tests.is_empty() {
            return Ok(
                "No test file imports these files, directly or indirectly. Either nothing \
                 tests them or the tests reach them another way."
                    .to_string(),
            );
        }

        let mut output = format!("Tests that import these files ({}):\n", tests.len());

        for test in &tests {
            output.push_str(&format!("  {}\n", workspace.relative(test)));
        }

        Ok(output)
    }
}
