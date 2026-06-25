//! CLI parsing and command-dispatch tests for the `agent-monitor` binary.
//!
//! Included into `main.rs` via `#[path]` so these tests can exercise the
//! binary crate's private parsers and helpers.

use super::*;
use coding_agent_monitor::{AgentKind, InstallMode};

#[test]
fn parses_monitor_command_with_workspace_store() {
    let command = parse_cli([
        "--workspace=.",
        "--retry-limit=3",
        "--fallbacks=claude-code,pi",
    ])
    .expect("parse monitor command");

    match command {
        CliCommand::Monitor { config, workspace } => {
            assert_eq!(workspace, Some(PathBuf::from(".")));
            assert_eq!(config.retry_limit, 3);
            assert_eq!(config.fallback_agents, vec!["claude-code", "pi"]);
        }
        CliCommand::InstallInjection { .. }
        | CliCommand::InjectRunning { .. }
        | CliCommand::Wrap { .. }
        | CliCommand::Judge { .. }
        | CliCommand::Ingest { .. }
        | CliCommand::HookResponse { .. }
        | CliCommand::ConfigAdvisor { .. }
        | CliCommand::ConfigVerifier { .. }
        | CliCommand::ConfigRuntimeAuth { .. }
        | CliCommand::ConfigImportLocal { .. }
        | CliCommand::ConfigImportCodingPlanCredentials { .. }
        | CliCommand::Advise { .. }
        | CliCommand::Trail { .. }
        | CliCommand::DevHistory { .. }
        | CliCommand::Calibration { .. }
        | CliCommand::Blame { .. }
        | CliCommand::RepoHunks { .. }
        | CliCommand::Requirements { .. }
        | CliCommand::CompletionCertificate { .. }
        | CliCommand::Trace { .. }
        | CliCommand::Handoff { .. }
        | CliCommand::MemoryPromote { .. }
        | CliCommand::RepoAudit { .. }
        | CliCommand::Verify { .. }
        | CliCommand::Probe { .. }
        | CliCommand::Demo { .. } => {
            panic!("expected monitor command")
        }
    }
}

#[test]
fn monitor_command_rejects_empty_fallbacks() {
    let error = parse_cli(["--fallbacks=,,"]).expect_err("empty fallbacks should be rejected");

    assert!(error.contains("--fallbacks must include at least one agent"));
}

#[test]
fn parses_inject_command() {
    let command = parse_cli([
        "inject",
        "--agent=claude-code",
        "--workspace=.",
        "--overwrite=true",
    ])
    .expect("parse inject command");

    match command {
        CliCommand::InstallInjection {
            agent,
            workspace,
            mode,
        } => {
            assert_eq!(agent, AgentKind::ClaudeCode);
            assert_eq!(workspace, PathBuf::from("."));
            assert_eq!(mode, InstallMode::CreateOrOverwrite);
        }
        CliCommand::Monitor { .. }
        | CliCommand::InjectRunning { .. }
        | CliCommand::Wrap { .. }
        | CliCommand::Judge { .. }
        | CliCommand::Ingest { .. }
        | CliCommand::HookResponse { .. }
        | CliCommand::ConfigAdvisor { .. }
        | CliCommand::ConfigVerifier { .. }
        | CliCommand::ConfigRuntimeAuth { .. }
        | CliCommand::ConfigImportLocal { .. }
        | CliCommand::ConfigImportCodingPlanCredentials { .. }
        | CliCommand::Advise { .. }
        | CliCommand::Trail { .. }
        | CliCommand::DevHistory { .. }
        | CliCommand::Calibration { .. }
        | CliCommand::Blame { .. }
        | CliCommand::RepoHunks { .. }
        | CliCommand::Requirements { .. }
        | CliCommand::CompletionCertificate { .. }
        | CliCommand::Trace { .. }
        | CliCommand::Handoff { .. }
        | CliCommand::MemoryPromote { .. }
        | CliCommand::RepoAudit { .. }
        | CliCommand::Verify { .. }
        | CliCommand::Probe { .. }
        | CliCommand::Demo { .. } => {
            panic!("expected injection command")
        }
    }
}

#[test]
fn inject_command_defaults_to_managed_block_merge() {
    let command =
        parse_cli(["inject", "--agent=codex", "--workspace=."]).expect("parse inject command");

    match command {
        CliCommand::InstallInjection { mode, .. } => {
            assert_eq!(mode, InstallMode::MergeManagedBlock);
        }
        _ => panic!("expected injection command"),
    }
}

#[test]
fn parses_inject_running_command() {
    let command = parse_cli(["inject-running", "--workspace=.", "--overwrite=true"])
        .expect("parse inject-running command");

    match command {
        CliCommand::InjectRunning {
            workspace, mode, ..
        } => {
            assert_eq!(workspace, Some(PathBuf::from(".")));
            assert_eq!(mode, InstallMode::CreateOrOverwrite);
        }
        _ => panic!("expected inject-running command"),
    }
}

#[test]
fn inject_running_defaults_to_managed_block_merge() {
    let command =
        parse_cli(["inject-running", "--workspace=."]).expect("parse inject-running command");

    match command {
        CliCommand::InjectRunning { mode, .. } => {
            assert_eq!(mode, InstallMode::MergeManagedBlock);
        }
        _ => panic!("expected inject-running command"),
    }
}

#[test]
fn inject_running_without_workspace_uses_process_cwd_later() {
    let command = parse_cli(["inject-running"]).expect("parse inject-running command");

    match command {
        CliCommand::InjectRunning {
            workspace, mode, ..
        } => {
            assert_eq!(workspace, None);
            assert_eq!(mode, InstallMode::MergeManagedBlock);
        }
        _ => panic!("expected inject-running command"),
    }
}

#[test]
fn inject_running_defaults_to_dry_run() {
    let command = parse_cli(["inject-running"]).expect("parse inject-running command");

    match command {
        CliCommand::InjectRunning { apply, .. } => {
            assert!(!apply);
        }
        _ => panic!("expected inject-running command"),
    }
}

#[test]
fn inject_running_apply_flag_enables_writes() {
    let command = parse_cli(["inject-running", "--apply"]).expect("parse inject-running command");

    match command {
        CliCommand::InjectRunning { apply, .. } => {
            assert!(apply);
        }
        _ => panic!("expected inject-running command"),
    }
}

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

#[test]
fn parses_wrap_command_after_double_dash() {
    let command = parse_cli([
        "wrap",
        "--agent=codex",
        "--workspace=.",
        "--session=s1",
        "--",
        "codex",
        "exec",
    ])
    .expect("parse wrap command");

    match command {
        CliCommand::Wrap {
            agent,
            workspace,
            session,
            command,
        } => {
            assert_eq!(agent, AgentKind::Codex);
            assert_eq!(workspace, PathBuf::from("."));
            assert_eq!(session.as_deref(), Some("s1"));
            assert_eq!(command, vec!["codex", "exec"]);
        }
        _ => panic!("expected wrap command"),
    }
}

#[test]
fn parses_judge_command_with_external_command_after_separator() {
    let command = parse_cli([
        "judge",
        "--workspace=E:/demo",
        "--write-intervention=true",
        "--",
        "claude",
        "-p",
        "review",
    ])
    .expect("parse judge command");

    match command {
        CliCommand::Judge {
            workspace,
            write_intervention,
            external_command,
        } => {
            assert_eq!(workspace, PathBuf::from("E:/demo"));
            assert!(write_intervention);
            assert_eq!(external_command, vec!["claude", "-p", "review"]);
        }
        _ => panic!("expected judge command"),
    }
}

#[test]
fn judge_review_intervention_preserves_spawn_judge_action() {
    let report = AgentReviewReport {
        workspace: "E:/demo".into(),
        status: coding_agent_monitor::AgentReviewStatus::Intervene,
        findings: vec![coding_agent_monitor::AgentReviewFinding {
            severity: coding_agent_monitor::DashboardSeverity::Critical,
            category: "suspicious_untraced_change".into(),
            agent: Some("codex".into()),
            evidence: "src/lib.rs has dirty git hunks without trace evidence".into(),
            recommended_action: AgentReviewAction::SpawnJudgeAgent,
        }],
    };

    let interventions = interventions_from_review(&report);

    assert_eq!(interventions.len(), 1);
    assert_eq!(
        interventions[0].kind,
        coding_agent_monitor::InterventionKind::SuspiciousChange
    );
    assert_eq!(
        interventions[0].action,
        coding_agent_monitor::Action::SpawnJudgeAgent
    );
}

#[test]
fn external_judge_prompt_is_bounded_and_evidence_first() {
    assert!(EXTERNAL_JUDGE_PROMPT.contains("Output exactly one line"));
    assert!(EXTERNAL_JUDGE_PROMPT.contains("continue | force_verification | handoff | restart"));
    assert!(EXTERNAL_JUDGE_PROMPT.contains("evidence=<ids/files/tests>"));
    assert!(EXTERNAL_JUDGE_PROMPT.contains("Judge the control loop"));
    assert!(EXTERNAL_JUDGE_PROMPT.contains("unverified completion"));
    assert!(EXTERNAL_JUDGE_PROMPT.contains("stale verification"));
    assert!(EXTERNAL_JUDGE_PROMPT.contains("intended-environment validation"));
    assert!(EXTERNAL_JUDGE_PROMPT.contains("mobile/native/system/ML"));
    assert!(EXTERNAL_JUDGE_PROMPT.contains("Prefer force_verification over handoff"));
    assert!(EXTERNAL_JUDGE_PROMPT.contains("Do not propose broad refactors"));
}

#[test]
fn parses_advise_command() {
    let command = parse_cli(["advise", "--workspace=E:/demo"]).expect("parse advise command");

    match command {
        CliCommand::Advise { workspace } => {
            assert_eq!(workspace, PathBuf::from("E:/demo"));
        }
        _ => panic!("expected advise command"),
    }
}

#[test]
fn parses_trace_command_with_requirement_binding() {
    let command = parse_cli([
        "trace",
        "--workspace=E:/demo",
        "--agent=codex",
        "--session=s1",
        "--event-id=evt-trace-proof",
        "--file=src/lib.rs",
        "--line=10",
        "--line-end=18",
        "--rationale=Implement project-contract proof mapping.",
        "--related-event=evt-user-goal",
        "--requirement-id=req-contract-every-meaningful-change",
    ])
    .expect("parse trace command");

    match command {
        CliCommand::Trace { workspace, entry } => {
            assert_eq!(workspace, PathBuf::from("E:/demo"));
            assert_eq!(entry.agent, "codex");
            assert_eq!(entry.session.as_deref(), Some("s1"));
            assert_eq!(entry.event_id.as_deref(), Some("evt-trace-proof"));
            assert_eq!(entry.file, "src/lib.rs");
            assert_eq!(entry.line, Some(10));
            assert_eq!(entry.line_end, Some(18));
            assert_eq!(
                entry.rationale.as_deref(),
                Some("Implement project-contract proof mapping.")
            );
            assert_eq!(entry.related_event_ids, vec!["evt-user-goal".to_string()]);
            assert_eq!(
                entry.requirement_ids,
                vec!["req-contract-every-meaningful-change".to_string()]
            );
        }
        _ => panic!("expected trace command"),
    }
}

#[test]
fn trace_command_requires_rationale() {
    let error = parse_cli(["trace", "--file=src/lib.rs"])
        .expect_err("trace command should require rationale");

    assert!(error.contains("trace requires --rationale"));
}

#[test]
fn parses_ingest_command() {
    let command = parse_cli([
        "ingest",
        "--adapter=codex",
        "--workspace=E:/demo",
        "--session=codex-live",
        "--retry-limit=0",
        "--fallbacks=pi,opencode",
    ])
    .expect("parse ingest command");

    match command {
        CliCommand::Ingest {
            adapter,
            workspace,
            session,
            config,
        } => {
            assert_eq!(adapter, AgentKind::Codex);
            assert_eq!(workspace, PathBuf::from("E:/demo"));
            assert_eq!(session.as_deref(), Some("codex-live"));
            assert_eq!(config.retry_limit, 0);
            assert_eq!(config.fallback_agents, vec!["pi", "opencode"]);
        }
        _ => panic!("expected ingest command"),
    }
}

#[test]
fn ingest_command_rejects_empty_fallbacks() {
    let error = parse_cli(["ingest", "--adapter=codex", "--fallbacks=,,"])
        .expect_err("empty fallbacks should be rejected");

    assert!(error.contains("--fallbacks must include at least one agent"));
}

#[test]
fn parses_hook_response_command() {
    let command = parse_cli([
        "hook-response",
        "--adapter=opencode",
        "--workspace=E:/demo",
        "--session=open-live",
        "--format=generic",
    ])
    .expect("parse hook-response command");

    match command {
        CliCommand::HookResponse {
            adapter,
            workspace,
            session,
            format,
        } => {
            assert_eq!(adapter, AgentKind::OpenCode);
            assert_eq!(workspace, PathBuf::from("E:/demo"));
            assert_eq!(session.as_deref(), Some("open-live"));
            assert_eq!(format, HookResponseFormat::Generic);
        }
        _ => panic!("expected hook-response command"),
    }
}

#[test]
fn parses_claude_code_hook_response_format() {
    let command = parse_cli([
        "hook-response",
        "--adapter=claude-code",
        "--format=claude-code",
    ])
    .expect("parse hook-response command");

    match command {
        CliCommand::HookResponse { format, .. } => {
            assert_eq!(format, HookResponseFormat::ClaudeCode);
        }
        _ => panic!("expected hook-response command"),
    }
}

#[test]
fn parses_codex_hook_response_format() {
    let command = parse_cli(["hook-response", "--adapter=codex", "--format=codex"])
        .expect("parse hook-response command");

    match command {
        CliCommand::HookResponse { format, .. } => {
            assert_eq!(format, HookResponseFormat::Codex);
        }
        _ => panic!("expected hook-response command"),
    }
}

#[test]
fn parses_opencode_hook_response_format() {
    let command = parse_cli(["hook-response", "--adapter=opencode", "--format=opencode"])
        .expect("parse hook-response command");

    match command {
        CliCommand::HookResponse { format, .. } => {
            assert_eq!(format, HookResponseFormat::OpenCode);
        }
        _ => panic!("expected hook-response command"),
    }
}

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
    assert!(!value.pointer("/hookSpecificOutput").is_some());
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

#[test]
fn parses_config_advisor_command() {
    let command = parse_cli([
        "config",
        "advisor",
        "--workspace=E:/demo",
        "--endpoint=https://api.example.test/v1/chat/completions",
        "--model=gpt-5.5",
        "--api-key-env=CAM_ADVISOR_KEY",
    ])
    .expect("parse config advisor command");

    match command {
        CliCommand::ConfigAdvisor { workspace, update } => {
            assert_eq!(workspace, PathBuf::from("E:/demo"));
            assert_eq!(
                update.endpoint,
                "https://api.example.test/v1/chat/completions"
            );
            assert_eq!(update.model, "gpt-5.5");
            assert_eq!(update.api_key_env, "CAM_ADVISOR_KEY");
            assert_eq!(update.credential_source, None);
            assert_eq!(update.credential_file, None);
            assert!(update.enabled);
        }
        _ => panic!("expected config advisor command"),
    }
}

#[test]
fn parses_config_advisor_command_with_coding_plan_credentials() {
    let command = parse_cli([
        "config",
        "advisor",
        "--workspace=E:/demo",
        "--endpoint=https://api.openai.com/v1/chat/completions",
        "--model=gpt-5.5",
        "--credential-source=coding-plan",
        "--credential-file=credentials/coding-plan/auth.json",
    ])
    .expect("parse config advisor command");

    match command {
        CliCommand::ConfigAdvisor { workspace, update } => {
            assert_eq!(workspace, PathBuf::from("E:/demo"));
            assert_eq!(
                update.endpoint,
                "https://api.openai.com/v1/chat/completions"
            );
            assert_eq!(update.model, "gpt-5.5");
            assert_eq!(update.api_key_env, "OPENAI_API_KEY");
            assert_eq!(
                update.credential_source,
                Some(AdvisorCredentialSource::CodingPlan)
            );
            assert_eq!(
                update.credential_file.as_deref(),
                Some("credentials/coding-plan/auth.json")
            );
        }
        _ => panic!("expected config advisor command"),
    }
}

#[test]
fn config_advisor_command_requires_api_key_env_name() {
    let error = parse_cli([
        "config",
        "advisor",
        "--endpoint=https://api.example.test/v1/chat/completions",
        "--model=gpt-5.5",
    ])
    .expect_err("missing api key env should fail");

    assert!(error.contains("--api-key-env"));
}

#[test]
fn config_advisor_command_requires_credential_file_for_coding_plan_source() {
    let error = parse_cli([
        "config",
        "advisor",
        "--endpoint=https://api.openai.com/v1/chat/completions",
        "--model=gpt-5.5",
        "--credential-source=coding-plan",
    ])
    .expect_err("missing credential file should fail");

    assert!(error.contains("--credential-file"));
}

#[test]
fn config_advisor_command_rejects_claude_plan_credential_source() {
    let error = parse_cli([
        "config",
        "advisor",
        "--endpoint=https://api.example.test/v1/messages",
        "--model=claude-opus-4",
        "--credential-source=claude-plan",
        "--credential-file=credentials/advisor.json",
    ])
    .expect_err("claude-plan should not be accepted as an advisor credential source");

    assert!(error.contains("coding-plan"));
    assert!(!error.contains("claude-plan"));
}

#[test]
fn config_advisor_command_rejects_codex_plan_credential_source_alias() {
    let error = parse_cli([
        "config",
        "advisor",
        "--endpoint=https://api.openai.com/v1/chat/completions",
        "--model=gpt-5.5",
        "--credential-source=codex-plan",
        "--credential-file=credentials/coding-plan/auth.json",
    ])
    .expect_err("codex-plan should not be accepted as a credential source");

    assert!(error.contains("coding-plan"));
    assert!(!error.contains("codex-plan"));
}

#[test]
fn config_import_local_rejects_copy_credentials_flag() {
    let error = parse_cli([
        "config",
        "import-local",
        "--workspace=E:/demo",
        "--home=C:/Users/yys",
        "--codex=true",
        "--claude-code=false",
        "--copy-credentials=true",
    ])
    .expect_err("copying local cli credentials should be rejected");

    assert!(error.contains("--advisor-credential-file"));
    assert!(error.contains("dedicated advisor credentials"));
}

#[test]
fn parses_config_import_local_command_with_dedicated_advisor_credentials() {
    let command = parse_cli([
        "config",
        "import-local",
        "--workspace=E:/demo",
        "--home=C:/Users/yys",
        "--advisor-credential-source=coding-plan",
        "--advisor-credential-file=C:/Users/yys/coding-plan/auth.json",
    ])
    .expect("parse config import-local command");

    match command {
        CliCommand::ConfigImportLocal {
            workspace,
            home,
            options,
        } => {
            assert_eq!(workspace, PathBuf::from("E:/demo"));
            assert_eq!(home, PathBuf::from("C:/Users/yys"));
            assert!(options.codex);
            assert!(options.claude_code);
            assert!(!options.copy_credentials);
            assert_eq!(
                options.advisor_credential_source,
                Some(AdvisorCredentialSource::CodingPlan)
            );
            assert_eq!(
                options.advisor_credential_file.as_deref(),
                Some("C:/Users/yys/coding-plan/auth.json")
            );
        }
        _ => panic!("expected config import-local command"),
    }
}

#[test]
fn parses_config_runtime_auth_local_broker_command() {
    let command = parse_cli([
        "config",
        "runtime-auth",
        "--workspace=E:/demo",
        "--agent=codex",
        "--style=local-auth-broker",
        "--endpoint=http://127.0.0.1:8787/v1",
        "--profile-id=cc-switch-codex",
        "--account-id=chatgpt-pro",
        "--model=gpt-5.5",
        "--api-format=openai_responses",
        "--health-status=healthy",
    ])
    .expect("parse runtime auth command");

    match command {
        CliCommand::ConfigRuntimeAuth {
            workspace,
            agent,
            runtime_auth,
        } => {
            assert_eq!(workspace, PathBuf::from("E:/demo"));
            assert_eq!(agent, AgentKind::Codex);
            assert_eq!(
                runtime_auth.style,
                coding_agent_monitor::RuntimeAuthStyle::LocalAuthBroker
            );
            assert_eq!(
                runtime_auth.endpoint.as_deref(),
                Some("http://127.0.0.1:8787/v1")
            );
            assert_eq!(runtime_auth.profile_id.as_deref(), Some("cc-switch-codex"));
            assert_eq!(runtime_auth.account_id.as_deref(), Some("chatgpt-pro"));
            assert_eq!(runtime_auth.model.as_deref(), Some("gpt-5.5"));
            assert_eq!(runtime_auth.api_format.as_deref(), Some("openai_responses"));
            assert_eq!(runtime_auth.health_status.as_deref(), Some("healthy"));
        }
        _ => panic!("expected runtime auth command"),
    }
}

#[test]
fn parses_config_import_coding_plan_credentials_command() {
    let command = parse_cli([
        "config",
        "import-coding-plan-credentials",
        "--workspace=E:/demo",
        "--source-file=C:/Users/yys/coding-plan/auth.json",
        "--endpoint=https://api.openai.com/v1/chat/completions",
        "--model=gpt-5.5",
    ])
    .expect("parse coding-plan credential import command");

    match command {
        CliCommand::ConfigImportCodingPlanCredentials {
            workspace,
            source_file,
            endpoint,
            model,
        } => {
            assert_eq!(workspace, PathBuf::from("E:/demo"));
            assert_eq!(
                source_file,
                PathBuf::from("C:/Users/yys/coding-plan/auth.json")
            );
            assert_eq!(
                endpoint.as_deref(),
                Some("https://api.openai.com/v1/chat/completions")
            );
            assert_eq!(model.as_deref(), Some("gpt-5.5"));
        }
        _ => panic!("expected config import-coding-plan-credentials command"),
    }
}

#[test]
fn config_import_coding_plan_credentials_defaults_to_dedicated_profile() {
    let command = parse_cli(["config", "import-coding-plan-credentials"])
        .expect("parse coding-plan credential import command");

    match command {
        CliCommand::ConfigImportCodingPlanCredentials { source_file, .. } => {
            assert!(source_file.ends_with(PathBuf::from(".coding-plan").join("auth.json")));
            assert!(!source_file.to_string_lossy().contains(".codex"));
            assert!(!source_file.to_string_lossy().contains(".claude"));
        }
        _ => panic!("expected config import-coding-plan-credentials command"),
    }
}

#[test]
fn config_import_coding_plan_credentials_rejects_cli_auth_source_path() {
    let error = parse_cli([
        "config",
        "import-coding-plan-credentials",
        "--source-file=C:/Users/yys/.codex/auth.json",
    ])
    .expect_err("local CLI auth must not parse as coding-plan source");

    assert!(error.contains("local CLI auth"));
    assert!(error.contains("dedicated coding-plan"));
}

#[test]
fn parses_config_verifier_command() {
    let command = parse_cli([
        "config",
        "verifier",
        "--workspace=E:/demo",
        "--id=smoke",
        "--command=cargo test --quiet",
        "--scope=full",
        "--timeout-secs=900",
        "--path=src/lib.rs",
        "--path=tests/entropy_control.rs",
        "--acceptance-pattern=runtime_validation:native_gui",
    ])
    .expect("parse config verifier command");

    match command {
        CliCommand::ConfigVerifier {
            workspace,
            verifier,
        } => {
            assert_eq!(workspace, PathBuf::from("E:/demo"));
            assert_eq!(verifier.id, "smoke");
            assert_eq!(verifier.command, "cargo test --quiet");
            assert_eq!(
                verifier.scope,
                coding_agent_monitor::VerificationScope::Full
            );
            assert_eq!(verifier.timeout_secs, 900);
            assert_eq!(
                verifier.paths,
                vec![
                    "src/lib.rs".to_string(),
                    "tests/entropy_control.rs".to_string()
                ]
            );
            assert_eq!(
                verifier.acceptance_patterns,
                vec!["runtime_validation:native_gui".to_string()]
            );
        }
        _ => panic!("expected config verifier command"),
    }
}

#[test]
fn config_verifier_command_rejects_empty_command() {
    let error = parse_cli([
        "config",
        "verifier",
        "--id=smoke",
        "--command= ",
        "--scope=targeted",
    ])
    .expect_err("empty verifier command should fail");

    assert!(error.contains("--command"));
}

#[test]
fn parses_trail_command() {
    let command = parse_cli(["trail", "--workspace=E:/demo"]).expect("parse trail command");

    match command {
        CliCommand::Trail { workspace } => {
            assert_eq!(workspace, PathBuf::from("E:/demo"));
        }
        _ => panic!("expected trail command"),
    }
}

#[test]
fn parses_dev_history_command() {
    let command = parse_cli([
        "dev-history",
        "--workspace=F:/rag_sys",
        "--home=C:/Users/yys",
        "--codex-sessions=C:/tmp/codex",
        "--claude-projects=C:/tmp/claude",
        "--top=7",
        "--write",
    ])
    .expect("parse dev-history command");

    match command {
        CliCommand::DevHistory {
            workspace,
            codex_sessions_root,
            claude_projects_root,
            top_limit,
            write,
            export_raw,
            raw_output_root,
            raw_package_name,
        } => {
            assert_eq!(workspace, PathBuf::from("F:/rag_sys"));
            assert_eq!(codex_sessions_root, PathBuf::from("C:/tmp/codex"));
            assert_eq!(claude_projects_root, PathBuf::from("C:/tmp/claude"));
            assert_eq!(top_limit, 7);
            assert!(write);
            assert!(!export_raw);
            assert_eq!(raw_output_root, None);
            assert_eq!(raw_package_name, None);
        }
        _ => panic!("expected dev-history command"),
    }
}

#[test]
fn parses_dev_history_raw_export_command() {
    let command = parse_cli([
        "dev-history",
        "--workspace=F:/rag_sys",
        "--home=C:/Users/yys",
        "--export-raw",
        "--output=F:/rag_sys/.agent-monitor/exports",
        "--package-name=rag-sys-raw",
    ])
    .expect("parse raw dev-history command");

    match command {
        CliCommand::DevHistory {
            workspace,
            export_raw,
            raw_output_root,
            raw_package_name,
            ..
        } => {
            assert_eq!(workspace, PathBuf::from("F:/rag_sys"));
            assert!(export_raw);
            assert_eq!(
                raw_output_root,
                Some(PathBuf::from("F:/rag_sys/.agent-monitor/exports"))
            );
            assert_eq!(raw_package_name.as_deref(), Some("rag-sys-raw"));
        }
        _ => panic!("expected dev-history command"),
    }
}

#[test]
fn dev_history_command_defaults_to_local_cli_history_roots() {
    let command =
        parse_cli(["dev-history", "--home=C:/Users/yys"]).expect("parse dev-history command");

    match command {
        CliCommand::DevHistory {
            codex_sessions_root,
            claude_projects_root,
            ..
        } => {
            assert_eq!(
                codex_sessions_root,
                PathBuf::from("C:/Users/yys")
                    .join(".codex")
                    .join("sessions")
            );
            assert_eq!(
                claude_projects_root,
                PathBuf::from("C:/Users/yys")
                    .join(".claude")
                    .join("projects")
            );
        }
        _ => panic!("expected dev-history command"),
    }
}

#[test]
fn parses_memory_promote_command() {
    let command = parse_cli([
        "memory",
        "promote",
        "--workspace=E:/demo",
        "--memory-id=mem-evt-design",
        "--source=manual_review",
    ])
    .expect("parse memory promote command");

    match command {
        CliCommand::MemoryPromote {
            workspace,
            memory_id,
            source,
        } => {
            assert_eq!(workspace, PathBuf::from("E:/demo"));
            assert_eq!(memory_id, "mem-evt-design");
            assert_eq!(source, MemorySource::ManualReview);
        }
        _ => panic!("expected memory promote command"),
    }
}

#[test]
fn memory_promote_command_rejects_agent_claim_source() {
    let error = parse_cli([
        "memory",
        "promote",
        "--memory-id=mem-evt-design",
        "--source=agent_claim",
    ])
    .expect_err("agent_claim source should not parse for promotion");

    assert!(error.contains("trusted source"));
}

#[test]
fn memory_promote_command_rejects_unknown_source() {
    let error = parse_cli([
        "memory",
        "promote",
        "--memory-id=mem-evt-design",
        "--source=unknown",
    ])
    .expect_err("unknown source should not parse for promotion");

    assert!(error.contains("--source must be one of"));
}

#[test]
fn parses_blame_command() {
    let command = parse_cli([
        "blame",
        "--workspace=E:/demo",
        "--file=src/lib.rs",
        "--line=42",
        "--limit=3",
    ])
    .expect("parse blame command");

    match command {
        CliCommand::Blame {
            workspace,
            file,
            line,
            limit,
        } => {
            assert_eq!(workspace, PathBuf::from("E:/demo"));
            assert_eq!(file, "src/lib.rs");
            assert_eq!(line, Some(42));
            assert_eq!(limit, 3);
        }
        _ => panic!("expected blame command"),
    }
}

#[test]
fn blame_command_rejects_zero_line() {
    let error = parse_cli(["blame", "--file=src/lib.rs", "--line=0"])
        .expect_err("zero line should be rejected");

    assert!(error.contains("--line must be a positive integer"));
}

#[test]
fn parses_handoff_command() {
    let command = parse_cli(["handoff", "--workspace=E:/demo", "--agent=claude-code"])
        .expect("parse handoff command");

    match command {
        CliCommand::Handoff { workspace, agent } => {
            assert_eq!(workspace, PathBuf::from("E:/demo"));
            assert_eq!(agent, AgentKind::ClaudeCode);
        }
        _ => panic!("expected handoff command"),
    }
}

#[test]
fn parses_repo_audit_command() {
    let command =
        parse_cli(["repo-audit", "--workspace=E:/demo"]).expect("parse repo-audit command");

    match command {
        CliCommand::RepoAudit { workspace } => {
            assert_eq!(workspace, PathBuf::from("E:/demo"));
        }
        _ => panic!("expected repo-audit command"),
    }
}

#[test]
fn parses_repo_hunks_command() {
    let command = parse_cli([
        "repo-hunks",
        "--workspace=E:/demo",
        "--file=src/lib.rs",
        "--line=42",
        "--limit=5",
    ])
    .expect("parse repo-hunks command");

    match command {
        CliCommand::RepoHunks {
            workspace,
            file,
            line,
            limit,
        } => {
            assert_eq!(workspace, PathBuf::from("E:/demo"));
            assert_eq!(file.as_deref(), Some("src/lib.rs"));
            assert_eq!(line, Some(42));
            assert_eq!(limit, 5);
        }
        _ => panic!("expected repo-hunks command"),
    }
}

#[test]
fn parses_requirements_command() {
    let command = parse_cli([
        "requirements",
        "--workspace=E:/demo",
        "--status=unmapped",
        "--requirement=req-docs",
        "--text=coding-plan",
        "--max-proof-score=49",
        "--limit=5",
    ])
    .expect("parse requirements command");

    match command {
        CliCommand::Requirements { workspace, query } => {
            assert_eq!(workspace, PathBuf::from("E:/demo"));
            assert_eq!(
                query.status,
                Some(coding_agent_monitor::AcceptanceCoverageStatus::Unmapped)
            );
            assert_eq!(query.requirement_id.as_deref(), Some("req-docs"));
            assert_eq!(query.text.as_deref(), Some("coding-plan"));
            assert_eq!(query.max_proof_score, Some(49));
            assert_eq!(query.limit, 5);
        }
        _ => panic!("expected requirements command"),
    }
}

#[test]
fn parses_completion_certificate_command() {
    let command = parse_cli([
        "completion-certificate",
        "--workspace=E:/demo",
        "--status=covered",
        "--requirement=req-api",
        "--text=advisor",
        "--max-proof-score=60",
        "--limit=3",
    ])
    .expect("parse completion-certificate command");

    match command {
        CliCommand::CompletionCertificate { workspace, query } => {
            assert_eq!(workspace, PathBuf::from("E:/demo"));
            assert_eq!(
                query.status,
                Some(coding_agent_monitor::AcceptanceCoverageStatus::Covered)
            );
            assert_eq!(query.requirement_id.as_deref(), Some("req-api"));
            assert_eq!(query.text.as_deref(), Some("advisor"));
            assert_eq!(query.max_proof_score, Some(60));
            assert_eq!(query.limit, 3);
        }
        _ => panic!("expected completion-certificate command"),
    }
}

#[test]
fn requirements_command_rejects_invalid_max_proof_score() {
    let error = parse_cli(["requirements", "--max-proof-score=101"])
        .expect_err("proof score must be bounded");

    assert!(error.contains("--max-proof-score"));
}

#[test]
fn parses_calibration_command() {
    let command = parse_cli([
        "calibration",
        "--workspace=E:/demo",
        "--action=force_verification",
        "--limit=7",
    ])
    .expect("parse calibration command");

    match command {
        CliCommand::Calibration { workspace, query } => {
            assert_eq!(workspace, PathBuf::from("E:/demo"));
            assert_eq!(
                query.action,
                Some(coding_agent_monitor::ControlActionKind::ForceVerification)
            );
            assert_eq!(query.limit, 7);
        }
        _ => panic!("expected calibration command"),
    }
}

#[test]
fn parses_run_probe_action_kind() {
    let command = parse_cli([
        "calibration",
        "--workspace=E:/demo",
        "--action=run_probe",
        "--limit=7",
    ])
    .expect("parse calibration command");

    match command {
        CliCommand::Calibration { query, .. } => {
            assert_eq!(
                query.action,
                Some(coding_agent_monitor::ControlActionKind::RunProbe)
            );
        }
        _ => panic!("expected calibration command"),
    }
}

#[test]
fn repo_hunks_command_rejects_zero_line() {
    let error = parse_cli(["repo-hunks", "--line=0"]).expect_err("zero line should be rejected");

    assert!(error.contains("--line must be a positive integer"));
}

#[test]
fn parses_verify_command() {
    let command = parse_cli(["verify", "--workspace=E:/demo", "--verifier=smoke"])
        .expect("parse verify command");

    match command {
        CliCommand::Verify {
            workspace,
            verifier_id,
        } => {
            assert_eq!(workspace, PathBuf::from("E:/demo"));
            assert_eq!(verifier_id, "smoke");
        }
        _ => panic!("expected verify command"),
    }
}

#[test]
fn parses_probe_command() {
    let command = parse_cli(["probe", "--workspace=E:/demo"]).expect("parse probe command");

    match command {
        CliCommand::Probe { workspace } => {
            assert_eq!(workspace, PathBuf::from("E:/demo"));
        }
        _ => panic!("expected probe command"),
    }
}

#[test]
fn parses_demo_command() {
    let command = parse_cli(["demo", "--workspace=E:/coding-agent-monitor-demo"])
        .expect("parse demo command");

    match command {
        CliCommand::Demo { workspace } => {
            assert_eq!(workspace, PathBuf::from("E:/coding-agent-monitor-demo"));
        }
        _ => panic!("expected demo command"),
    }
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
