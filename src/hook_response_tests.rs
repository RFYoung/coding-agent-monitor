//! Tests for pre-tool hook response handling: workspace security policy,
//! read-only judge control packets, event persistence, and native renderers.
//!
//! Included into the module via `#[path]` so they can reach its private
//! helpers as well as the binary crate root.

use super::*;

#[test]
fn hook_response_blocks_protected_path_from_workspace_security() {
    let workspace = tempfile::tempdir().expect("workspace");
    std::fs::create_dir_all(workspace.path().join(".agent-monitor")).expect("store");
    std::fs::write(
        workspace.path().join(".agent-monitor").join("config.json"),
        r#"{
          "security": {
            "protected_paths": ["protected/**"]
          }
        }"#,
    )
    .expect("config");
    let input = br#"{"event":"tool.execute.before","session":{"id":"open-live"},"tool":"edit","input":{"path":"protected/deploy.yml"}}"#;
    let mut output = Vec::new();

    run_hook_response(
        &input[..],
        &mut output,
        AgentKind::OpenCode,
        workspace.path(),
        None,
        HookResponseFormat::Generic,
    )
    .expect("hook response");

    let response: coding_agent_monitor::AdapterHookResponse =
        serde_json::from_slice(&output).expect("response json");
    assert_eq!(
        response.decision,
        coding_agent_monitor::AdapterHookDecision::Block
    );
    assert_eq!(response.session.as_deref(), Some("open-live"));
    assert!(response.reason.unwrap().contains("protected path"));
}

#[test]
fn hook_response_persists_attempt_and_block_decision() {
    let workspace = tempfile::tempdir().expect("workspace");
    std::fs::create_dir_all(workspace.path().join(".agent-monitor")).expect("store");
    std::fs::write(
        workspace.path().join(".agent-monitor").join("config.json"),
        r#"{
          "security": {
            "protected_paths": ["protected/**"]
          }
        }"#,
    )
    .expect("config");
    let input = br#"{"event":"tool.execute.before","id":"hook-attempt-1","session":{"id":"open-live"},"tool":"edit","input":{"path":"protected/deploy.yml"}}"#;
    let mut output = Vec::new();

    run_hook_response(
        &input[..],
        &mut output,
        AgentKind::OpenCode,
        workspace.path(),
        None,
        HookResponseFormat::Generic,
    )
    .expect("hook response");

    let events =
        std::fs::read_to_string(workspace.path().join(".agent-monitor").join("events.jsonl"))
            .expect("events");
    let events = events
        .lines()
        .map(|line| serde_json::from_str::<coding_agent_monitor::Event>(line).expect("event"))
        .collect::<Vec<_>>();

    assert_eq!(events.len(), 2);
    assert_eq!(events[0].kind, coding_agent_monitor::EventKind::ToolCall);
    assert_eq!(events[0].event_id.as_deref(), Some("hook-attempt-1"));
    assert_eq!(
        events[0].command.as_deref(),
        Some("edit protected/deploy.yml")
    );
    assert_eq!(
        events[1].kind,
        coding_agent_monitor::EventKind::InterventionResult
    );
    assert_eq!(
        events[1].event_id.as_deref(),
        Some("hook-attempt-1:hook-response")
    );
    assert_eq!(events[1].related_event_ids, vec!["hook-attempt-1"]);
    assert!(
        events[1]
            .content
            .as_deref()
            .is_some_and(|content| content.contains("protected path"))
    );
}

#[test]
fn hook_response_persists_attempt_and_ask_decision() {
    let workspace = tempfile::tempdir().expect("workspace");
    let input = br#"{"hook_event_name":"PreToolUse","id":"hook-ask-1","session_id":"claude-live","decision":"ask","reason":"needs approval for protected workflow edit","tool_name":"Edit","tool_input":{"file_path":"protected/deploy.yml"}}"#;
    let mut output = Vec::new();

    run_hook_response(
        &input[..],
        &mut output,
        AgentKind::ClaudeCode,
        workspace.path(),
        None,
        HookResponseFormat::Generic,
    )
    .expect("hook response");

    let response: coding_agent_monitor::AdapterHookResponse =
        serde_json::from_slice(&output).expect("response json");
    assert_eq!(
        response.decision,
        coding_agent_monitor::AdapterHookDecision::Ask
    );

    let events =
        std::fs::read_to_string(workspace.path().join(".agent-monitor").join("events.jsonl"))
            .expect("events");
    let events = events
        .lines()
        .map(|line| serde_json::from_str::<coding_agent_monitor::Event>(line).expect("event"))
        .collect::<Vec<_>>();

    assert_eq!(events.len(), 2);
    assert_eq!(
        events[1].kind,
        coding_agent_monitor::EventKind::InterventionResult
    );
    assert_eq!(
        events[1].event_id.as_deref(),
        Some("hook-ask-1:hook-response")
    );
    assert!(
        events[1]
            .content
            .as_deref()
            .is_some_and(|content| content.contains("hook response requested user authorization"))
    );
}

#[test]
fn claude_code_hook_response_blocks_write_under_read_only_judge_packet() {
    let workspace = tempfile::tempdir().expect("workspace");
    let mut store = ProjectStore::open(workspace.path()).expect("store");
    store
        .append_event(&Event {
            event_id: Some("session-started".into()),
            agent: agent_kind_label(AgentKind::ClaudeCode).into(),
            session: Some("claude-live".into()),
            agent_session_id: Some("claude-live".into()),
            kind: EventKind::AgentHealth,
            content: Some("session started".into()),
            ..Event::default()
        })
        .expect("append precondition event");
    store
        .append_event(&Event {
            event_id: Some("evidence-1".into()),
            agent: agent_kind_label(AgentKind::ClaudeCode).into(),
            session: Some("claude-live".into()),
            agent_session_id: Some("claude-live".into()),
            kind: EventKind::ModelMessage,
            content: Some("read-only judge evidence".into()),
            ..Event::default()
        })
        .expect("append packet evidence");
    store
        .write_control_packet(&coding_agent_monitor::ControlPacket {
            packet_id: "packet-read-only-judge".into(),
            target_agent: agent_kind_label(AgentKind::ClaudeCode).into(),
            urgency: coding_agent_monitor::PacketUrgency::Context,
            title: "Read-only judge review required".into(),
            summary: "The monitor selected a read-only judge review.".into(),
            instructions: vec![coding_agent_monitor::PacketInstruction {
                priority: coding_agent_monitor::PacketInstructionPriority::Must,
                text: "Act as a read-only judge and inspect the evidence without editing.".into(),
            }],
            evidence_refs: vec!["evidence-1".into()],
            forbidden: vec![
                "Do not edit files or mutate the worktree during judge review.".into(),
                "Do not run destructive commands or apply patches.".into(),
            ],
            success_criteria: vec!["Judge report returned without file changes.".into()],
            preconditions: coding_agent_monitor::PacketPreconditions {
                adapter: Some(agent_kind_label(AgentKind::ClaudeCode).into()),
                agent_session_id: Some("claude-live".into()),
                ..Default::default()
            },
        })
        .expect("write control packet");
    let input = br#"{"hook_event_name":"PreToolUse","session_id":"claude-live","tool_name":"Write","tool_input":{"file_path":"src/lib.rs"}}"#;
    let mut output = Vec::new();

    run_hook_response(
        &input[..],
        &mut output,
        AgentKind::ClaudeCode,
        workspace.path(),
        None,
        HookResponseFormat::ClaudeCode,
    )
    .expect("hook response");

    let value: serde_json::Value = serde_json::from_slice(&output).expect("response json");
    assert_eq!(
        value.pointer("/hookSpecificOutput/permissionDecision"),
        Some(&serde_json::Value::String("deny".into()))
    );
    assert!(
        value
            .pointer("/hookSpecificOutput/permissionDecisionReason")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|reason| reason.contains("read-only judge"))
    );

    let events =
        std::fs::read_to_string(workspace.path().join(".agent-monitor").join("events.jsonl"))
            .expect("events");
    assert!(
        events
            .lines()
            .filter_map(|line| serde_json::from_str::<Event>(line).ok())
            .any(|event| event.kind == EventKind::InterventionResult
                && event
                    .content
                    .as_deref()
                    .is_some_and(|content| content.contains("read-only judge")))
    );
}

#[test]
fn claude_code_hook_response_ignores_read_only_judge_packet_after_git_head_moves() {
    let workspace = tempfile::tempdir().expect("workspace");
    git(workspace.path(), ["init"]);
    git(
        workspace.path(),
        ["config", "user.email", "monitor@example.test"],
    );
    git(workspace.path(), ["config", "user.name", "Monitor Test"]);
    std::fs::create_dir_all(workspace.path().join("src")).expect("src dir");
    std::fs::write(
        workspace.path().join("src").join("lib.rs"),
        "pub fn one() {}\n",
    )
    .expect("initial source");
    git(workspace.path(), ["add", "src/lib.rs"]);
    git(workspace.path(), ["commit", "-m", "initial"]);
    let initial_head = git_stdout(workspace.path(), ["rev-parse", "HEAD"]);

    let mut store = ProjectStore::open(workspace.path()).expect("store");
    store
        .append_event(&Event {
            event_id: Some("session-started".into()),
            agent: agent_kind_label(AgentKind::ClaudeCode).into(),
            session: Some("claude-live".into()),
            agent_session_id: Some("claude-live".into()),
            kind: EventKind::AgentHealth,
            content: Some("session started".into()),
            ..Event::default()
        })
        .expect("append precondition event");
    store
        .append_event(&Event {
            event_id: Some("evidence-1".into()),
            agent: agent_kind_label(AgentKind::ClaudeCode).into(),
            session: Some("claude-live".into()),
            agent_session_id: Some("claude-live".into()),
            kind: EventKind::ModelMessage,
            content: Some("read-only judge evidence".into()),
            ..Event::default()
        })
        .expect("append packet evidence");
    store
        .write_control_packet(&coding_agent_monitor::ControlPacket {
            packet_id: "packet-stale-read-only-judge".into(),
            target_agent: agent_kind_label(AgentKind::ClaudeCode).into(),
            urgency: coding_agent_monitor::PacketUrgency::Context,
            title: "Read-only judge review required".into(),
            summary: "The monitor selected a read-only judge review.".into(),
            instructions: vec![coding_agent_monitor::PacketInstruction {
                priority: coding_agent_monitor::PacketInstructionPriority::Must,
                text: "Act as a read-only judge and inspect the evidence without editing.".into(),
            }],
            evidence_refs: vec!["evidence-1".into()],
            forbidden: vec![
                "Do not edit files or mutate the worktree during judge review.".into(),
                "Do not run destructive commands or apply patches.".into(),
            ],
            success_criteria: vec!["Judge report returned without file changes.".into()],
            preconditions: coding_agent_monitor::PacketPreconditions {
                git_head: Some(initial_head),
                adapter: Some(agent_kind_label(AgentKind::ClaudeCode).into()),
                agent_session_id: Some("claude-live".into()),
                ..Default::default()
            },
        })
        .expect("write control packet at initial head");

    std::fs::write(
        workspace.path().join("src").join("lib.rs"),
        "pub fn two() {}\n",
    )
    .expect("advance source");
    git(workspace.path(), ["add", "src/lib.rs"]);
    git(workspace.path(), ["commit", "-m", "advance head"]);
    let input = br#"{"hook_event_name":"PreToolUse","session_id":"claude-live","tool_name":"Write","tool_input":{"file_path":"src/lib.rs"}}"#;
    let mut output = Vec::new();

    run_hook_response(
        &input[..],
        &mut output,
        AgentKind::ClaudeCode,
        workspace.path(),
        None,
        HookResponseFormat::ClaudeCode,
    )
    .expect("hook response");

    assert!(
        output.is_empty(),
        "Claude Code allow responses should not emit a blocking payload: {}",
        String::from_utf8_lossy(&output)
    );
    let events =
        std::fs::read_to_string(workspace.path().join(".agent-monitor").join("events.jsonl"))
            .expect("events");
    assert!(
        !events
            .lines()
            .filter_map(|line| serde_json::from_str::<Event>(line).ok())
            .any(|event| event.kind == EventKind::InterventionResult
                && event
                    .content
                    .as_deref()
                    .is_some_and(|content| content.contains("read-only judge"))),
        "stale packet should not produce read-only judge intervention: {events}"
    );
}

#[test]
fn claude_code_hook_response_format_renders_native_denial() {
    let workspace = tempfile::tempdir().expect("workspace");
    let input = br#"{"hook_event_name":"PreToolUse","session_id":"claude-live","tool_name":"Bash","tool_input":{"command":"git clean -fdx"}}"#;
    let mut output = Vec::new();

    run_hook_response(
        &input[..],
        &mut output,
        AgentKind::ClaudeCode,
        workspace.path(),
        None,
        HookResponseFormat::ClaudeCode,
    )
    .expect("hook response");

    let value: serde_json::Value = serde_json::from_slice(&output).expect("response json");
    assert_eq!(
        value.pointer("/hookSpecificOutput/hookEventName"),
        Some(&serde_json::Value::String("PreToolUse".into()))
    );
    assert_eq!(
        value.pointer("/hookSpecificOutput/permissionDecision"),
        Some(&serde_json::Value::String("deny".into()))
    );
    assert!(
        value
            .pointer("/hookSpecificOutput/permissionDecisionReason")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|reason| reason.contains("git clean -fdx"))
    );
}

#[test]
fn claude_code_hook_response_format_renders_native_ask() {
    let workspace = tempfile::tempdir().expect("workspace");
    let input = br#"{"hook_event_name":"PreToolUse","session_id":"claude-live","decision":"ask","reason":"needs approval for destructive command","tool_name":"Bash","tool_input":{"command":"git clean -fdx"}}"#;
    let mut output = Vec::new();

    run_hook_response(
        &input[..],
        &mut output,
        AgentKind::ClaudeCode,
        workspace.path(),
        None,
        HookResponseFormat::ClaudeCode,
    )
    .expect("hook response");

    let value: serde_json::Value = serde_json::from_slice(&output).expect("response json");
    assert_eq!(
        value.pointer("/hookSpecificOutput/hookEventName"),
        Some(&serde_json::Value::String("PreToolUse".into()))
    );
    assert_eq!(
        value.pointer("/hookSpecificOutput/permissionDecision"),
        Some(&serde_json::Value::String("ask".into()))
    );
    assert!(
        value
            .pointer("/hookSpecificOutput/permissionDecisionReason")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|reason| reason.contains("needs approval"))
    );
}

#[test]
fn claude_code_hook_response_format_emits_no_output_on_allow() {
    let workspace = tempfile::tempdir().expect("workspace");
    let input = br#"{"hook_event_name":"PreToolUse","session_id":"claude-live","tool_name":"Read","tool_input":{"file_path":"src/lib.rs"}}"#;
    let mut output = Vec::new();

    run_hook_response(
        &input[..],
        &mut output,
        AgentKind::ClaudeCode,
        workspace.path(),
        None,
        HookResponseFormat::ClaudeCode,
    )
    .expect("hook response");

    assert!(output.is_empty());
}

#[test]
fn codex_hook_response_format_renders_native_denial() {
    let workspace = tempfile::tempdir().expect("workspace");
    let input = br#"{"hook_event_name":"PreToolUse","session_id":"codex-live","tool_name":"Shell","tool_input":{"command":"git clean -fdx"}}"#;
    let mut output = Vec::new();

    run_hook_response(
        &input[..],
        &mut output,
        AgentKind::Codex,
        workspace.path(),
        None,
        HookResponseFormat::Codex,
    )
    .expect("hook response");

    let value: serde_json::Value = serde_json::from_slice(&output).expect("response json");
    assert_eq!(
        value.pointer("/hookSpecificOutput/hookEventName"),
        Some(&serde_json::Value::String("PreToolUse".into()))
    );
    assert_eq!(
        value.pointer("/hookSpecificOutput/permissionDecision"),
        Some(&serde_json::Value::String("deny".into()))
    );
    assert!(
        value
            .pointer("/hookSpecificOutput/permissionDecisionReason")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|reason| reason.contains("git clean -fdx"))
    );
}

#[test]
fn codex_hook_response_format_emits_no_output_on_allow() {
    let workspace = tempfile::tempdir().expect("workspace");
    let input = br#"{"hook_event_name":"PreToolUse","session_id":"codex-live","tool_name":"Read","tool_input":{"file_path":"src/lib.rs"}}"#;
    let mut output = Vec::new();

    run_hook_response(
        &input[..],
        &mut output,
        AgentKind::Codex,
        workspace.path(),
        None,
        HookResponseFormat::Codex,
    )
    .expect("hook response");

    assert!(output.is_empty());
}

#[test]
fn opencode_hook_response_format_renders_native_denial() {
    let workspace = tempfile::tempdir().expect("workspace");
    let input = br#"{"event":"tool.execute.before","session":{"id":"open-live"},"tool":"bash","input":{"command":"git clean -fdx"}}"#;
    let mut output = Vec::new();

    run_hook_response(
        &input[..],
        &mut output,
        AgentKind::OpenCode,
        workspace.path(),
        None,
        HookResponseFormat::OpenCode,
    )
    .expect("hook response");

    let value: serde_json::Value = serde_json::from_slice(&output).expect("response json");
    assert_eq!(
        value.pointer("/action"),
        Some(&serde_json::Value::String("block".into()))
    );
    assert_eq!(
        value.pointer("/decision"),
        Some(&serde_json::Value::String("block".into()))
    );
    assert!(value.pointer("/hookSpecificOutput").is_none());
    assert!(
        value
            .pointer("/message")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|reason| reason.contains("git clean -fdx"))
    );
}

#[test]
fn opencode_hook_response_format_emits_no_output_on_allow() {
    let workspace = tempfile::tempdir().expect("workspace");
    let input = br#"{"event":"tool.execute.before","session":{"id":"open-live"},"tool":"read","input":{"path":"src/lib.rs"}}"#;
    let mut output = Vec::new();

    run_hook_response(
        &input[..],
        &mut output,
        AgentKind::OpenCode,
        workspace.path(),
        None,
        HookResponseFormat::OpenCode,
    )
    .expect("hook response");

    assert!(output.is_empty());
}

#[test]
fn hook_response_rejects_adapter_without_blocking_capability() {
    let workspace = tempfile::tempdir().expect("workspace");
    let input = br#"{"event":"pi.wrapper.command_started","command":"git clean -fdx"}"#;
    let mut output = Vec::new();

    let error = run_hook_response(
        &input[..],
        &mut output,
        AgentKind::Pi,
        workspace.path(),
        None,
        HookResponseFormat::Generic,
    )
    .expect_err("Pi cannot consume hook block responses by default");

    assert!(error.contains("does not support pre-tool blocking"));
    assert!(output.is_empty());
}

#[test]
fn hook_response_rejects_disabled_adapter() {
    let workspace = tempfile::tempdir().expect("workspace");
    std::fs::create_dir_all(workspace.path().join(".agent-monitor")).expect("store");
    std::fs::write(
        workspace.path().join(".agent-monitor").join("config.json"),
        r#"{
          "adapters": {
            "claude_code": { "enabled": false }
          }
        }"#,
    )
    .expect("config");
    let input = br#"{"hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{"command":"git clean -fdx"}}"#;
    let mut output = Vec::new();

    let error = run_hook_response(
        &input[..],
        &mut output,
        AgentKind::ClaudeCode,
        workspace.path(),
        None,
        HookResponseFormat::ClaudeCode,
    )
    .expect_err("disabled adapter should not produce hook responses");

    assert!(error.contains("disabled"));
    assert!(output.is_empty());
}

fn git<const N: usize>(workspace: &std::path::Path, args: [&str; N]) {
    let output = std::process::Command::new("git")
        .current_dir(workspace)
        .args(args)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git command failed: {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn git_stdout<const N: usize>(workspace: &std::path::Path, args: [&str; N]) -> String {
    let output = std::process::Command::new("git")
        .current_dir(workspace)
        .args(args)
        .output()
        .expect("run git");
    assert!(
        output.status.success(),
        "git command failed: {}\nstdout:\n{}\nstderr:\n{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("git stdout utf8")
        .trim()
        .to_string()
}
