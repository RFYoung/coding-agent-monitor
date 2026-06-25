//! Tests for shared command helpers: injection-workspace selection.
//!
//! Included into the module via `#[path]` so they can reach its private
//! helpers as well as the binary crate root.

use super::*;
use crate::*;

#[test]
fn injection_workspace_prefers_explicit_workspace_then_process_cwd() {
    let agent = coding_agent_monitor::RunningAgent::new(1, AgentKind::Codex, "codex.exe")
        .with_cwd(Some(PathBuf::from("F:/agent-repo")));

    assert_eq!(
        injection_workspace_for(&agent, Some(&PathBuf::from("F:/explicit"))),
        Some(PathBuf::from("F:/explicit"))
    );
    assert_eq!(
        injection_workspace_for(&agent, None),
        Some(PathBuf::from("F:/agent-repo"))
    );
}

#[test]
fn injection_workspace_skips_agent_support_directories_without_explicit_workspace() {
    let codex_cache = coding_agent_monitor::RunningAgent::new(1, AgentKind::Codex, "codex.exe")
        .with_cwd(Some(PathBuf::from(
            "C:/Users/yys/.codex/plugins/cache/openai-bundled/chrome",
        )));
    let claude_skill =
        coding_agent_monitor::RunningAgent::new(2, AgentKind::ClaudeCode, "node.exe").with_cwd(
            Some(PathBuf::from("C:/Users/yys/.claude/skills/ppt-polish")),
        );

    assert_eq!(injection_workspace_for(&codex_cache, None), None);
    assert_eq!(injection_workspace_for(&claude_skill, None), None);
    assert_eq!(
        injection_workspace_for(&codex_cache, Some(&PathBuf::from("F:/real-project"))),
        Some(PathBuf::from("F:/real-project"))
    );
}
