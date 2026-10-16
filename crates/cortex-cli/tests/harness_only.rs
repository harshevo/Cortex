use clap::Parser;
use cortex_cli::args::{Cli, Command};
use cortex_cli::features::{self, SessionLocation};

#[test]
fn removed_features_and_flags_are_rejected() {
    for feature in ["brain", "memory", "hope", "lessons", "priors"] {
        assert!(Cli::try_parse_from(["cortex", "chat", "--with", feature]).is_err());
    }
    for flag in ["--memory-db", "--memory-from", "--hope-url"] {
        assert!(Cli::try_parse_from(["cortex", "chat", flag, "unused"]).is_err());
    }
}

#[tokio::test]
async fn a_plain_harness_has_no_learner_tools_or_persistent_memory() {
    let project = tempfile::tempdir().unwrap();
    let cli = Cli::try_parse_from(["cortex", "chat", "--no-records"]).unwrap();
    let Command::Chat(chat) = cli.command else {
        unreachable!()
    };
    let model = chat.session.model.with_defaults();
    let setup = features::setup(
        &model,
        SessionLocation {
            directory: project.path().join(".cortex"),
            scope: project.path().display().to_string(),
        },
    )
    .await
    .unwrap();
    let specs = setup.tools.specs();
    let names: Vec<_> = specs.iter().map(|spec| spec.name.as_str()).collect();
    assert!(names.contains(&"read_file") && names.contains(&"shell"));
    for removed in [
        "brain_chat",
        "teach_brain",
        "brain_state",
        "remember",
        "recall",
    ] {
        assert!(!names.contains(&removed));
    }
    assert!(setup.advisers.is_empty());
    assert!(!setup.extras.reflect);
    assert!(!project.path().join(".cortex").exists());
}
