use coding_agent_monitor::{AgentKind, RunningAgent, RunningProcess, detect_running_agents};
use std::path::PathBuf;

#[test]
fn detects_running_coding_agents_from_process_names_and_commands() {
    let processes = vec![
        RunningProcess::new(100, "codex.exe", "codex --model gpt-5"),
        RunningProcess::new(101, "node.exe", "node C:/npm/claude-code"),
        RunningProcess::new(102, "opencode", "opencode run"),
        RunningProcess::new(103, "python.exe", "python -m pi_agent"),
        RunningProcess::new(104, "git.exe", "git status"),
    ];

    let agents = detect_running_agents(&processes);

    assert_eq!(
        agents,
        vec![
            RunningAgent::new(100, AgentKind::Codex, "codex.exe"),
            RunningAgent::new(101, AgentKind::ClaudeCode, "node.exe"),
            RunningAgent::new(102, AgentKind::OpenCode, "opencode"),
            RunningAgent::new(103, AgentKind::Pi, "python.exe"),
        ]
    );
}

#[test]
fn deduplicates_same_process_when_name_and_command_both_match() {
    let processes = vec![RunningProcess::new(200, "claude-code.exe", "claude-code")];

    let agents = detect_running_agents(&processes);

    assert_eq!(
        agents,
        vec![RunningAgent::new(
            200,
            AgentKind::ClaudeCode,
            "claude-code.exe"
        )]
    );
}

#[test]
fn detected_agents_preserve_process_working_directory() {
    let processes =
        vec![RunningProcess::new(300, "codex.exe", "codex").with_cwd(PathBuf::from("F:/repo-a"))];

    let agents = detect_running_agents(&processes);

    assert_eq!(agents[0].cwd, Some(PathBuf::from("F:/repo-a")));
}
