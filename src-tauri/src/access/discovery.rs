//! Evidence for Skills hosts, separate from invocation adapters and installation state.
use super::{installed_skills, skills_rel, AccessTarget};
use crate::agent;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub(super) fn targets(
    home: Option<&Path>,
    commands: &HashMap<String, PathBuf>,
    applications: &[PathBuf],
    dsh_home: Option<&Path>,
) -> Vec<AccessTarget> {
    let mut targets: Vec<_> = agent::ids().filter_map(|id| {
        let relative = skills_rel(id)?;
        let dir = home.map(|root| root.join(relative));
        Some(host(id, &agent::agent_name(id), commands.get(id), None, dir))
    }).collect();
    for (id, name, app_name, relative) in [
        ("chatgpt-desktop", "ChatGPT 桌面端", Some("ChatGPT.app"), ".codex"),
        ("workbuddy", "WorkBuddy 桌面端", Some("WorkBuddy AI.app"), ".workbuddy-ai"),
        ("deepseek-harness", "DeepSeek Harness", None, ".dsh"),
    ] {
        let root = if id == "deepseek-harness" {
            dsh_home.map(Path::to_path_buf).or_else(|| home.map(|path| path.join(relative)))
        } else { home.map(|path| path.join(relative)) };
        let app = app_name.and_then(|name| applications.iter().map(|dir| dir.join(name)).find(|path| path.is_dir()));
        targets.push(host(id, name, commands.get(id), app, root.map(|path| path.join("skills"))));
    }
    let dir = home.map(|root| root.join(".agents/skills")).unwrap_or_else(|| PathBuf::from(".agents/skills"));
    targets.push(AccessTarget {
        id: "custom".into(), name: "其他智能体".into(), available: true,
        command: None, desktop_app: None, config_dir: None,
        skills_dir: dir.to_string_lossy().into(), installed: installed_skills(&dir),
    });
    targets
}

fn host(id: &str, name: &str, command: Option<&PathBuf>, app: Option<PathBuf>, dir: Option<PathBuf>) -> AccessTarget {
    let config = dir.as_deref().and_then(Path::parent).filter(|path| path.is_dir());
    AccessTarget {
        id: id.into(), name: name.into(), available: command.is_some() || app.is_some(),
        command: command.map(|path| path.to_string_lossy().into()),
        desktop_app: app.map(|path| path.to_string_lossy().into()),
        config_dir: config.map(|path| path.to_string_lossy().into()),
        skills_dir: dir.as_ref().map(|path| path.to_string_lossy().into()).unwrap_or_default(),
        installed: dir.as_deref().map(installed_skills).unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configuration_residue_does_not_claim_an_installed_host() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir(home.path().join(".dsh")).unwrap();
        let targets = targets(Some(home.path()), &HashMap::new(), &[], None);
        let dsh = targets.iter().find(|target| target.id == "deepseek-harness").unwrap();
        assert!(!dsh.available);
        assert!(dsh.command.is_none() && dsh.desktop_app.is_none());
        assert!(dsh.config_dir.is_some());
    }

    #[test]
    fn cli_and_desktop_evidence_are_independent_even_with_shared_skills() {
        let home = tempfile::tempdir().unwrap();
        let apps = tempfile::tempdir().unwrap();
        std::fs::create_dir(apps.path().join("ChatGPT.app")).unwrap();
        std::fs::create_dir(apps.path().join("WorkBuddy AI.app")).unwrap();
        let targets = targets(Some(home.path()), &HashMap::new(), &[apps.path().into()], None);
        let codex = targets.iter().find(|target| target.id == "codex").unwrap();
        let desktop = targets.iter().find(|target| target.id == "chatgpt-desktop").unwrap();
        assert!(!codex.available);
        assert!(desktop.available && desktop.command.is_none() && desktop.desktop_app.is_some());
        assert_eq!(codex.skills_dir, desktop.skills_dir);
        assert!(targets.iter().find(|target| target.id == "workbuddy").unwrap().skills_dir.ends_with(".workbuddy-ai/skills"));
    }
}
