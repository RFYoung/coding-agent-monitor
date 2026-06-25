//! Shared helpers for `agent-monitor` command dispatch and parsing: default
//! credential-source resolution and injection-workspace selection.

use coding_agent_monitor::RunningAgent;
use std::env;
use std::path::{Path, PathBuf};

pub(crate) fn default_home_dir() -> PathBuf {
    env::var_os("USERPROFILE")
        .or_else(|| env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

pub(crate) fn default_coding_plan_credential_source() -> PathBuf {
    default_home_dir().join(".coding-plan").join("auth.json")
}

pub(crate) fn reject_local_cli_auth_source(path: &Path, label: &str) -> Result<(), String> {
    // Parse-time guard mirrors config-write validation: users may point the
    // monitor at a dedicated coding-plan profile, never raw Codex/Claude stores.
    if let Some(cli_dir) = local_cli_auth_dir(path) {
        return Err(format!(
            "{label} {} points at local CLI auth directory {cli_dir}; use a dedicated coding-plan credential profile outside Codex/Claude CLI config",
            path.display()
        ));
    }
    Ok(())
}

fn local_cli_auth_dir(path: &Path) -> Option<&'static str> {
    path.components().find_map(|component| {
        let std::path::Component::Normal(value) = component else {
            return None;
        };
        match value.to_string_lossy().to_ascii_lowercase().as_str() {
            ".codex" => Some(".codex"),
            ".claude" => Some(".claude"),
            _ => None,
        }
    })
}

pub(crate) fn injection_workspace_for(
    agent: &RunningAgent,
    requested: Option<&PathBuf>,
) -> Option<PathBuf> {
    // Auto-detected agent cwd is useful only when it is the user's project, not
    // a plugin cache, dependency tree, or monitor temp directory.
    if let Some(requested) = requested {
        return Some(requested.clone());
    }

    let cwd = agent.cwd.clone()?;
    if is_agent_support_directory(&cwd) {
        None
    } else {
        Some(cwd)
    }
}

fn is_agent_support_directory(path: &Path) -> bool {
    let normalized = path.to_string_lossy().replace('/', "\\").to_lowercase();
    [
        "\\.codex\\plugins\\cache\\",
        "\\.claude\\skills\\",
        "\\node_modules\\",
        "\\.agent-monitor\\tmp\\",
    ]
    .iter()
    .any(|segment| normalized.contains(segment))
}

#[cfg(test)]
#[path = "cli_support_tests.rs"]
mod tests;
