//! The project map as context for a task.
//!
//! Unlike the router's other sources this one does not depend on the task:
//! the same project gives the same map. It is placed before everything
//! else in the first message, so two tasks in one project start with the
//! same text and a provider's prompt cache can reuse it.

use std::sync::Arc;

use async_trait::async_trait;
use cortex_core::{Briefing, ContextSource, Workspace};
use cortex_index::{CodeIndex, repo_map};
use serde_json::json;

/// Roughly 1,500 tokens.
const MAX_CHARS: usize = 6_000;

pub struct ProjectMap {
    index: Arc<CodeIndex>,
}

impl ProjectMap {
    pub fn new(index: Arc<CodeIndex>) -> Self {
        Self { index }
    }
}

#[async_trait]
impl ContextSource for ProjectMap {
    async fn brief(&self, _instruction: &str, workspace: &Workspace) -> Option<Briefing> {
        let map = repo_map(&self.index, workspace.root(), MAX_CHARS)?;

        let text = format!(
            "## Project map\n\n\
             The project's source files as they are now, each with its number of lines and \
             the names it defines at the top level. Use it to go straight to the files the \
             task is about instead of listing directories.\n\n{}",
            map.text
        );

        Some(Briefing {
            source: "map".to_string(),
            detail: json!({
                "files": map.files,
                "detail": map.detail,
                "chars": map.text.len(),
            }),
            text,
        })
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    #[tokio::test]
    async fn the_map_names_the_files_and_what_they_define() {
        let dir = tempfile::tempdir().unwrap();

        fs::write(
            dir.path().join("fees.py"),
            "def compute_fee(amount):\n    return amount\n",
        )
        .unwrap();

        let workspace = Workspace::new(dir.path()).unwrap();

        let briefing = ProjectMap::new(Arc::new(CodeIndex::new()))
            .brief("anything", &workspace)
            .await
            .unwrap();

        assert!(briefing.text.starts_with("## Project map"));
        assert!(briefing.text.contains("fees.py (2): compute_fee"));
        assert_eq!(briefing.detail["files"], 1);
    }
}
