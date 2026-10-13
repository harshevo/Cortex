//! `cortex skills`

use std::path::Path;
use std::process::ExitCode;

use anyhow::{Result, bail};
use cortex_skills::SkillLibrary;

use crate::args::SkillsArgs;

pub fn execute(args: SkillsArgs) -> Result<ExitCode> {
    let library = SkillLibrary::for_project(&args.dir);

    let Some(name) = &args.name else {
        print!("{}", list(&args.dir));

        return Ok(ExitCode::SUCCESS);
    };

    let name = name.trim_start_matches('/');

    let Some(skill) = library.get(name) else {
        bail!(
            "there is no skill named `{name}`. The skills are: {}",
            library.names().join(", ")
        );
    };

    println!("{}", skill.body);

    Ok(ExitCode::SUCCESS)
}

/// One line per skill that can be started in `project`.
pub fn list(project: &Path) -> String {
    let library = SkillLibrary::for_project(project);

    let mut text = String::new();

    for skill in library.skills() {
        let hint = skill
            .argument_hint
            .as_deref()
            .map(|hint| format!(" {hint}"))
            .unwrap_or_default();

        text.push_str(&format!(
            "/{}{hint}  ({})\n    {}\n",
            skill.name,
            skill.origin.label(),
            skill.description
        ));
    }

    text
}
