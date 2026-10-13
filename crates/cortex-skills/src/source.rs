//! Starting a skill by typing `/name`.

use async_trait::async_trait;
use cortex_core::{Briefing, ContextSource, Workspace};
use serde_json::json;

use crate::{Invocation, SkillLibrary};

/// Puts a skill's text in front of a task that begins with `/name`.
///
/// The instruction itself is left as the person typed it, so the session
/// log and the window show `/review 12`, not three pages of procedure. An
/// instruction that names no skill gets nothing, and costs nothing.
#[derive(Debug, Clone, Copy, Default)]
pub struct SkillSource;

#[async_trait]
impl ContextSource for SkillSource {
    async fn brief(&self, instruction: &str, workspace: &Workspace) -> Option<Briefing> {
        let invocation = Invocation::parse(instruction)?;

        // Read when asked for, so a skill edited during a session is the
        // one that runs.
        let library = SkillLibrary::for_project(workspace.root());

        let skill = library.get(invocation.name)?;

        let text = format!(
            "## Skill: {}\n\nThe task below starts this skill. Carry it out as written.\n\n{}\n",
            skill.name,
            skill.render(invocation.arguments)
        );

        Some(Briefing {
            source: "skill".to_string(),
            text,
            detail: json!({
                "skill": skill.name,
                "origin": skill.origin.label(),
                "arguments": invocation.arguments,
            }),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_slash_instruction_brings_the_skill_with_its_arguments() {
        let project = tempfile::tempdir().unwrap();
        let workspace = Workspace::new(project.path()).unwrap();

        let briefing = SkillSource
            .brief("/review 12 --comment", &workspace)
            .await
            .unwrap();

        assert!(briefing.text.starts_with("## Skill: review"));
        assert!(briefing.text.contains("Arguments: 12 --comment"));
        assert_eq!(briefing.detail["skill"], "review");
    }

    #[tokio::test]
    async fn other_instructions_get_nothing() {
        let project = tempfile::tempdir().unwrap();
        let workspace = Workspace::new(project.path()).unwrap();

        assert!(SkillSource.brief("fix the bug", &workspace).await.is_none());
        assert!(SkillSource.brief("/nonesuch", &workspace).await.is_none());
    }
}
