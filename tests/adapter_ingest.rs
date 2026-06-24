use coding_agent_monitor::{
    Action, AdapterHookDecision, AdapterIngestOptions, AdviceRun, AgentKind, Config, ControlAction,
    DashboardSnapshot, Event, EventKind, Intervention, InterventionKind, ProjectStore,
    SecurityConfig, adapter_hook_response, build_control_case_file, normalize_adapter_event,
    run_adapter_jsonl_with_store,
};
use serde_json::json;

#[test]
fn codex_agent_message_normalizes_to_model_message() {
    let raw = json!({
        "type": "agent_message",
        "id": "codex-evt-1",
        "timestamp": "2026-06-22T18:20:10Z",
        "model": "gpt-5",
        "message": "Implementation complete. I did not run tests."
    });

    let event = normalize_adapter_event(AgentKind::Codex, Some("codex-session"), &raw)
        .expect("normalized event");

    assert_eq!(event.kind, EventKind::ModelMessage);
    assert_eq!(event.agent, "codex");
    assert_eq!(event.session.as_deref(), Some("codex-session"));
    assert_eq!(event.event_id.as_deref(), Some("codex-evt-1"));
    assert_eq!(event.time.as_deref(), Some("2026-06-22T18:20:10Z"));
    assert_eq!(event.model.as_deref(), Some("gpt-5"));
    assert_eq!(
        event.content.as_deref(),
        Some("Implementation complete. I did not run tests.")
    );
}

#[test]
fn codex_exec_thread_started_normalizes_to_session_health() {
    let raw = json!({
        "type": "thread.started",
        "thread_id": "codex-thread-1"
    });

    let event = normalize_adapter_event(AgentKind::Codex, None, &raw)
        .expect("normalized thread started event");

    assert_eq!(event.kind, EventKind::AgentHealth);
    assert_eq!(event.agent, "codex");
    assert_eq!(event.session.as_deref(), Some("codex-thread-1"));
    assert_eq!(event.agent_session_id.as_deref(), Some("codex-thread-1"));
    assert_eq!(event.content.as_deref(), Some("thread started"));
}

#[test]
fn codex_exec_command_item_started_normalizes_to_tool_call() {
    let raw = json!({
        "type": "item.started",
        "thread_id": "codex-thread-1",
        "item": {
            "id": "item-command-1",
            "type": "command_execution",
            "command": "cargo test parser::handles_nested",
            "status": "in_progress"
        }
    });

    let event = normalize_adapter_event(AgentKind::Codex, None, &raw)
        .expect("normalized command item start");

    assert_eq!(event.kind, EventKind::ToolCall);
    assert_eq!(event.agent, "codex");
    assert_eq!(event.event_id.as_deref(), Some("item-command-1"));
    assert_eq!(event.session.as_deref(), Some("codex-thread-1"));
    assert_eq!(
        event.command.as_deref(),
        Some("cargo test parser::handles_nested")
    );
    assert_eq!(
        event.content.as_deref(),
        Some("tool command: cargo test parser::handles_nested")
    );
}

#[test]
fn codex_exec_command_item_completed_normalizes_to_command_result() {
    let raw = json!({
        "type": "item.completed",
        "thread_id": "codex-thread-1",
        "item": {
            "id": "item-command-2",
            "type": "command_execution",
            "command": "npm test",
            "status": "failed",
            "exit_code": 1,
            "output": "upstream service unavailable"
        }
    });

    let event = normalize_adapter_event(AgentKind::Codex, None, &raw)
        .expect("normalized command item completion");

    assert_eq!(event.kind, EventKind::CommandResult);
    assert_eq!(event.agent, "codex");
    assert_eq!(event.event_id.as_deref(), Some("item-command-2"));
    assert_eq!(event.session.as_deref(), Some("codex-thread-1"));
    assert_eq!(event.command.as_deref(), Some("npm test"));
    assert_eq!(event.exit_code, Some(1));
    assert_eq!(
        event.content.as_deref(),
        Some("upstream service unavailable")
    );
}

#[test]
fn codex_exec_agent_message_item_completed_normalizes_to_model_message() {
    let raw = json!({
        "type": "item.completed",
        "thread_id": "codex-thread-1",
        "item": {
            "id": "item-message-1",
            "type": "agent_message",
            "text": "Tests passed; implementation complete."
        }
    });

    let event = normalize_adapter_event(AgentKind::Codex, None, &raw)
        .expect("normalized agent message item");

    assert_eq!(event.kind, EventKind::ModelMessage);
    assert_eq!(event.agent, "codex");
    assert_eq!(event.event_id.as_deref(), Some("item-message-1"));
    assert_eq!(event.session.as_deref(), Some("codex-thread-1"));
    assert_eq!(
        event.content.as_deref(),
        Some("Tests passed; implementation complete.")
    );
}

#[test]
fn normalized_pass_through_event_uses_default_session_when_session_is_empty() {
    let raw = json!({
        "kind": "model_message",
        "agent": "codex",
        "session": "",
        "content": "Still working."
    });

    let event = normalize_adapter_event(AgentKind::Codex, Some("fallback-session"), &raw)
        .expect("normalized event");

    assert_eq!(event.kind, EventKind::ModelMessage);
    assert_eq!(event.session.as_deref(), Some("fallback-session"));
}

#[test]
fn legacy_event_json_deserializes_with_empty_provenance_fields() {
    let event: Event = serde_json::from_str(
        r#"{"agent":"codex","kind":"model_message","content":"legacy event"}"#,
    )
    .expect("legacy event");

    assert_eq!(event.kind, EventKind::ModelMessage);
    assert_eq!(event.agent, "codex");
    assert!(event.project_id.is_none());
    assert!(event.run_id.is_none());
    assert!(event.agent_session_id.is_none());
    assert!(event.adapter.is_none());
    assert!(event.source_type.is_none());
    assert!(event.redaction_status.is_none());
    assert!(event.redaction_rules.is_empty());
}

#[test]
fn adapter_normalization_preserves_provenance_fields() {
    let raw = json!({
        "event": "tool.execute.before",
        "id": "open-evt-42",
        "timestamp": "2026-06-22T18:20:10.991Z",
        "observed_at": "2026-06-22T18:20:11.209Z",
        "project_id": "proj-1",
        "run_id": "run-1",
        "adapter_version": "0.7.0",
        "seq": 1842,
        "cwd": "F:\\coding-agent-monitor",
        "worktree": "F:\\coding-agent-monitor\\.worktrees\\cam-run-123",
        "git": {
            "head": "abc123",
            "branch": "cam/run-123",
            "dirty": true
        },
        "actor": "agent",
        "source": {
            "type": "hook",
            "path": ".opencode/log.jsonl",
            "offset": 99123,
            "hash": "blake3:feed"
        },
        "redaction": {
            "status": "clean",
            "rules": ["env_secret"]
        },
        "session": { "id": "open-live" },
        "tool": "bash",
        "input": { "command": "git status" }
    });

    let event = normalize_adapter_event(AgentKind::OpenCode, None, &raw).expect("normalized event");

    assert_eq!(event.kind, EventKind::ToolCall);
    assert_eq!(event.adapter.as_deref(), Some("opencode"));
    assert_eq!(event.adapter_version.as_deref(), Some("0.7.0"));
    assert_eq!(event.project_id.as_deref(), Some("proj-1"));
    assert_eq!(event.run_id.as_deref(), Some("run-1"));
    assert_eq!(event.agent_session_id.as_deref(), Some("open-live"));
    assert_eq!(event.seq, Some(1842));
    assert_eq!(
        event.observed_at.as_deref(),
        Some("2026-06-22T18:20:11.209Z")
    );
    assert_eq!(
        event.occurred_at.as_deref(),
        Some("2026-06-22T18:20:10.991Z")
    );
    assert_eq!(event.cwd.as_deref(), Some("F:\\coding-agent-monitor"));
    assert_eq!(
        event.worktree.as_deref(),
        Some("F:\\coding-agent-monitor\\.worktrees\\cam-run-123")
    );
    assert_eq!(event.git_head.as_deref(), Some("abc123"));
    assert_eq!(event.git_branch.as_deref(), Some("cam/run-123"));
    assert_eq!(event.git_dirty, Some(true));
    assert_eq!(event.actor.as_deref(), Some("agent"));
    assert_eq!(event.source_type.as_deref(), Some("hook"));
    assert_eq!(event.source_path.as_deref(), Some(".opencode/log.jsonl"));
    assert_eq!(event.source_offset, Some(99123));
    assert_eq!(event.source_hash.as_deref(), Some("blake3:feed"));
    assert_eq!(event.redaction_status.as_deref(), Some("clean"));
    assert_eq!(event.redaction_rules, vec!["env_secret".to_string()]);
}

#[test]
fn wrapped_plugin_properties_preserve_replay_provenance_fields() {
    let raw = json!({
        "event": {
            "type": "tool.execute.before",
            "properties": {
                "id": "open-wrapped-evt-42",
                "timestamp": "2026-06-22T18:20:10.991Z",
                "observed_at": "2026-06-22T18:20:11.209Z",
                "project_id": "proj-wrapped",
                "run_id": "run-wrapped",
                "adapter_version": "0.8.0",
                "seq": 2048,
                "cwd": "F:\\coding-agent-monitor",
                "worktree": "F:\\coding-agent-monitor\\.worktrees\\wrapped",
                "git": {
                    "head": "def456",
                    "branch": "cam/wrapped",
                    "dirty": true
                },
                "actor": "agent",
                "source": {
                    "type": "plugin",
                    "path": ".opencode/plugins/agent-monitor.js",
                    "offset": 42,
                    "hash": "blake3:wrapped"
                },
                "redaction": {
                    "status": "redacted",
                    "rules": ["token_like"]
                },
                "sessionID": "open-live",
                "tool": "bash",
                "input": { "command": "cargo test" }
            }
        }
    });

    let event = normalize_adapter_event(AgentKind::OpenCode, None, &raw)
        .expect("normalized wrapped provenance event");

    assert_eq!(event.kind, EventKind::ToolCall);
    assert_eq!(event.event_id.as_deref(), Some("open-wrapped-evt-42"));
    assert_eq!(event.time.as_deref(), Some("2026-06-22T18:20:10.991Z"));
    assert_eq!(event.project_id.as_deref(), Some("proj-wrapped"));
    assert_eq!(event.run_id.as_deref(), Some("run-wrapped"));
    assert_eq!(event.adapter_version.as_deref(), Some("0.8.0"));
    assert_eq!(event.seq, Some(2048));
    assert_eq!(
        event.observed_at.as_deref(),
        Some("2026-06-22T18:20:11.209Z")
    );
    assert_eq!(
        event.occurred_at.as_deref(),
        Some("2026-06-22T18:20:10.991Z")
    );
    assert_eq!(event.cwd.as_deref(), Some("F:\\coding-agent-monitor"));
    assert_eq!(
        event.worktree.as_deref(),
        Some("F:\\coding-agent-monitor\\.worktrees\\wrapped")
    );
    assert_eq!(event.git_head.as_deref(), Some("def456"));
    assert_eq!(event.git_branch.as_deref(), Some("cam/wrapped"));
    assert_eq!(event.git_dirty, Some(true));
    assert_eq!(event.actor.as_deref(), Some("agent"));
    assert_eq!(event.source_type.as_deref(), Some("plugin"));
    assert_eq!(
        event.source_path.as_deref(),
        Some(".opencode/plugins/agent-monitor.js")
    );
    assert_eq!(event.source_offset, Some(42));
    assert_eq!(event.source_hash.as_deref(), Some("blake3:wrapped"));
    assert_eq!(event.redaction_status.as_deref(), Some("redacted"));
    assert_eq!(event.redaction_rules, vec!["token_like".to_string()]);
}

#[test]
fn claude_stream_json_ingest_persists_normalized_event_and_dispatches_advice() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    let input = br#"{"type":"assistant","session_id":"claude-live","message":{"model":"claude-fable","content":[{"type":"text","text":"This is a good point to stop. Should I continue?"}]}}
"#;
    let mut output = Vec::new();

    run_adapter_jsonl_with_store(
        &input[..],
        &mut output,
        AdapterIngestOptions {
            adapter: AgentKind::ClaudeCode,
            session: Some("fallback-session".into()),
            config: Config::default(),
        },
        &mut store,
    )
    .expect("ingest adapter jsonl");

    let events =
        std::fs::read_to_string(temp.path().join(".agent-monitor/events.jsonl")).expect("events");
    assert!(events.contains("\"agent\":\"claude-code\""));
    assert!(events.contains("\"session\":\"claude-live\""));
    assert!(events.contains("\"kind\":\"model_message\""));
    assert!(events.contains("This is a good point to stop"));

    assert!(output.is_empty());
    let advice_log =
        std::fs::read_to_string(store.root().join("advice.jsonl")).expect("advice log");
    let advice: AdviceRun =
        serde_json::from_str(advice_log.lines().next().expect("one advice")).expect("advice json");
    assert_eq!(advice.packet.target_agent, "claude-code");
    assert!(store.root().join("outbox").join("claude-code").exists());
}

#[test]
fn adapter_jsonl_ingest_stamps_raw_line_provenance_when_source_is_missing() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    let input = br#"{"type":"assistant","session_id":"claude-live","message":{"content":[{"type":"text","text":"Still working from adapter stream."}]}}
"#;
    let mut output = Vec::new();

    run_adapter_jsonl_with_store(
        &input[..],
        &mut output,
        AdapterIngestOptions {
            adapter: AgentKind::ClaudeCode,
            session: Some("fallback-session".into()),
            config: Config::default(),
        },
        &mut store,
    )
    .expect("ingest adapter jsonl");

    let events =
        std::fs::read_to_string(temp.path().join(".agent-monitor/events.jsonl")).expect("events");
    let event: Event =
        serde_json::from_str(events.lines().next().expect("one event")).expect("event json");
    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);
    let evidence = case_file
        .evidence
        .iter()
        .find(|item| Some(item.id.as_str()) == event.event_id.as_deref())
        .expect("event evidence");

    assert_eq!(event.source_type.as_deref(), Some("adapter_jsonl"));
    assert_eq!(event.source_offset, Some(1));
    assert!(
        event
            .source_hash
            .as_deref()
            .is_some_and(|hash| hash.starts_with("fnv1a64:")),
        "missing source hash: {:?}",
        event.source_hash
    );
    assert_eq!(evidence.source_type.as_deref(), Some("adapter_jsonl"));
    assert_eq!(evidence.source_offset, Some(1));
    assert_eq!(evidence.source_hash, event.source_hash);
}

#[test]
fn claude_content_block_delta_normalizes_to_low_signal_command_output() {
    let raw = json!({
        "type": "content_block_delta",
        "session_id": "claude-live",
        "delta": {
            "type": "text_delta",
            "text": "This is a good point to stop. Should I continue?"
        }
    });

    let event = normalize_adapter_event(AgentKind::ClaudeCode, None, &raw)
        .expect("normalized content delta");

    assert_eq!(event.kind, EventKind::CommandOutput);
    assert_eq!(event.agent, "claude-code");
    assert_eq!(event.session.as_deref(), Some("claude-live"));
    assert_eq!(
        event.content.as_deref(),
        Some("message delta: This is a good point to stop. Should I continue?")
    );
}

#[test]
fn adapter_message_delta_ingest_records_low_signal_output_without_control_advice() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    let input = br#"{"type":"content_block_delta","session_id":"claude-live","delta":{"type":"text_delta","text":"This is a good point to stop. Should I continue?"}}
"#;
    let mut output = Vec::new();

    run_adapter_jsonl_with_store(
        &input[..],
        &mut output,
        AdapterIngestOptions {
            adapter: AgentKind::ClaudeCode,
            session: Some("fallback-session".into()),
            config: Config::default(),
        },
        &mut store,
    )
    .expect("ingest adapter jsonl");

    let events =
        std::fs::read_to_string(temp.path().join(".agent-monitor/events.jsonl")).expect("events");
    assert!(events.contains("\"agent\":\"claude-code\""));
    assert!(events.contains("\"session\":\"claude-live\""));
    assert!(events.contains("\"kind\":\"command_output\""));
    assert!(events.contains("message delta: This is a good point to stop"));
    assert!(output.is_empty());
    assert!(!temp.path().join(".agent-monitor/advice.jsonl").exists());
}

#[test]
fn adapter_ingest_rejects_disabled_adapter_without_persisting_events() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    std::fs::write(
        store.root().join("config.json"),
        r#"{
          "adapters": {
            "claude_code": {
              "enabled": false
            }
          }
        }"#,
    )
    .expect("config");
    let input =
        br#"{"type":"assistant","message":{"content":[{"type":"text","text":"Still working."}]}}
"#;
    let mut output = Vec::new();

    let err = run_adapter_jsonl_with_store(
        &input[..],
        &mut output,
        AdapterIngestOptions {
            adapter: AgentKind::ClaudeCode,
            session: Some("fallback-session".into()),
            config: Config::default(),
        },
        &mut store,
    )
    .expect_err("disabled adapter ingest should fail");

    assert!(err.to_string().contains("disabled"));
    assert!(output.is_empty());
    assert!(!store.root().join("events.jsonl").exists());
}

#[test]
fn opencode_tool_result_ingest_dispatches_validated_service_failure_advice() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    let input = br#"{"event":"tool.execute.after","session":{"id":"open-live"},"tool":"bash","input":{"command":"npm test"},"output":"upstream service unavailable","exit_code":1}
"#;
    let mut output = Vec::new();

    run_adapter_jsonl_with_store(
        &input[..],
        &mut output,
        AdapterIngestOptions {
            adapter: AgentKind::OpenCode,
            session: None,
            config: Config::default(),
        },
        &mut store,
    )
    .expect("ingest opencode jsonl");

    let events =
        std::fs::read_to_string(temp.path().join(".agent-monitor/events.jsonl")).expect("events");
    assert!(events.contains("\"kind\":\"command_result\""));
    assert!(events.contains("\"agent\":\"opencode\""));
    assert!(events.contains("\"session\":\"open-live\""));
    assert!(events.contains("\"command\":\"npm test\""));
    assert!(events.contains("\"exit_code\":1"));

    assert!(output.is_empty());
    let advice_log =
        std::fs::read_to_string(store.root().join("advice.jsonl")).expect("advice log");
    let advice: AdviceRun =
        serde_json::from_str(advice_log.lines().next().expect("one advice")).expect("advice json");
    assert_eq!(advice.packet.target_agent, "opencode");
}

#[test]
fn pi_wrapper_command_result_ingest_dispatches_validated_service_failure_advice() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    let input = br#"{"event":"pi.wrapper.command_result","session":{"id":"pi-sidecar-1"},"command":"python -m pytest","exit_code":1,"stderr":"upstream service unavailable"}
"#;
    let mut output = Vec::new();

    run_adapter_jsonl_with_store(
        &input[..],
        &mut output,
        AdapterIngestOptions {
            adapter: AgentKind::Pi,
            session: None,
            config: Config::default(),
        },
        &mut store,
    )
    .expect("ingest pi wrapper jsonl");

    let events =
        std::fs::read_to_string(temp.path().join(".agent-monitor/events.jsonl")).expect("events");
    assert!(events.contains("\"kind\":\"command_result\""));
    assert!(events.contains("\"agent\":\"pi\""));
    assert!(events.contains("\"session\":\"pi-sidecar-1\""));
    assert!(events.contains("\"command\":\"python -m pytest\""));
    assert!(events.contains("\"exit_code\":1"));
    assert!(events.contains("upstream service unavailable"));

    assert!(output.is_empty());
    let advice_log =
        std::fs::read_to_string(store.root().join("advice.jsonl")).expect("advice log");
    let advice: AdviceRun =
        serde_json::from_str(advice_log.lines().next().expect("one advice")).expect("advice json");
    assert_eq!(advice.packet.target_agent, "pi");
}

#[test]
fn pi_wrapper_session_error_normalizes_context_limit_taxonomy() {
    let raw = json!({
        "event": "pi.wrapper.session_error",
        "session": { "id": "pi-sidecar-1" },
        "error": {
            "code": "context_length_exceeded",
            "message": "maximum context length exceeded"
        }
    });

    let event =
        normalize_adapter_event(AgentKind::Pi, None, &raw).expect("normalized pi session error");

    assert_eq!(event.kind, EventKind::AgentHealth);
    assert_eq!(event.agent, "pi");
    assert_eq!(event.session.as_deref(), Some("pi-sidecar-1"));
    assert_eq!(
        event.content.as_deref(),
        Some("session error [context_limit]: maximum context length exceeded")
    );
}

#[test]
fn adapter_repeated_service_failures_trigger_control_switch_packet() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    let input = (1..=3)
        .map(|index| {
            format!(
                r#"{{"event":"tool.execute.after","id":"evt-stream-service-{index}","timestamp":"2026-06-22T18:2{index}:00Z","session":{{"id":"open-live"}},"tool":"bash","input":{{"command":"node scripts/probe.js"}},"output":"upstream service unavailable","exit_code":1}}"#
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    let mut output = Vec::new();

    run_adapter_jsonl_with_store(
        input.as_bytes(),
        &mut output,
        AdapterIngestOptions {
            adapter: AgentKind::OpenCode,
            session: None,
            config: Config::default(),
        },
        &mut store,
    )
    .expect("adapter ingest should trigger switch advice");

    let advice_log =
        std::fs::read_to_string(store.root().join("advice.jsonl")).expect("advice log");
    let advice_lines = advice_log.lines().collect::<Vec<_>>();
    assert_eq!(advice_lines.len(), 1);
    let advice: AdviceRun = serde_json::from_str(advice_lines[0]).expect("advice json");

    assert_eq!(
        advice.final_action,
        ControlAction::SwitchAgent {
            target_agent: "claude-code".into(),
        }
    );
    assert_eq!(advice.packet.target_agent, "claude-code");
    assert!(
        advice
            .packet
            .evidence_refs
            .contains(&"evt-stream-service-3".into())
    );
    assert!(
        store
            .root()
            .join("outbox")
            .join("claude-code")
            .join("latest.md")
            .exists()
    );
}

#[test]
fn adapter_verification_failure_triggers_control_advice_packet() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    let input = br#"{"event":"verification.failed","timestamp":"2026-06-22T18:20:10Z","session":{"id":"open-live"},"command":"cargo test","exit_code":1,"output":"test failed"}
"#;
    let mut output = Vec::new();

    run_adapter_jsonl_with_store(
        &input[..],
        &mut output,
        AdapterIngestOptions {
            adapter: AgentKind::OpenCode,
            session: None,
            config: Config::default(),
        },
        &mut store,
    )
    .expect("adapter ingest should persist and advise");

    let advice_log =
        std::fs::read_to_string(store.root().join("advice.jsonl")).expect("advice log");
    let advice: AdviceRun =
        serde_json::from_str(advice_log.lines().next().expect("one advice")).expect("advice json");

    assert!(matches!(
        advice.final_action,
        ControlAction::ForceVerification { blocking: true, .. }
    ));
    assert_eq!(advice.packet.target_agent, "opencode");
    assert!(
        store
            .root()
            .join("outbox")
            .join("opencode")
            .join("latest.md")
            .exists()
    );
}

#[test]
fn opencode_tool_execute_before_normalizes_to_tool_call_event() {
    let raw = json!({
        "event": "tool.execute.before",
        "session": { "id": "open-live" },
        "tool": "bash",
        "input": { "command": "git status" }
    });

    let event =
        normalize_adapter_event(AgentKind::OpenCode, None, &raw).expect("normalized tool call");

    assert_eq!(event.kind, EventKind::ToolCall);
    assert_eq!(event.agent, "opencode");
    assert_eq!(event.session.as_deref(), Some("open-live"));
    assert_eq!(event.command.as_deref(), Some("git status"));
    assert_eq!(event.content.as_deref(), Some("tool command: git status"));
}

#[test]
fn opencode_wrapped_plugin_properties_tool_execute_before_normalizes_command_and_session() {
    let raw = json!({
        "event": {
            "type": "tool.execute.before",
            "properties": {
                "sessionID": "open-live",
                "tool": "bash",
                "input": { "command": "npm test" }
            }
        }
    });

    let event = normalize_adapter_event(AgentKind::OpenCode, None, &raw)
        .expect("normalized wrapped plugin tool call");

    assert_eq!(event.kind, EventKind::ToolCall);
    assert_eq!(event.agent, "opencode");
    assert_eq!(event.session.as_deref(), Some("open-live"));
    assert_eq!(event.agent_session_id.as_deref(), Some("open-live"));
    assert_eq!(event.command.as_deref(), Some("npm test"));
    assert_eq!(event.content.as_deref(), Some("tool command: npm test"));
}

#[test]
fn opencode_diff_changed_normalizes_to_repo_diff_event() {
    let raw = json!({
        "event": "diff.changed",
        "session": { "id": "open-live" },
        "path": "src/lib.rs",
        "rationale": "Patch parser branch."
    });

    let event =
        normalize_adapter_event(AgentKind::OpenCode, None, &raw).expect("normalized repo diff");

    assert_eq!(event.kind, EventKind::RepoDiff);
    assert_eq!(event.agent, "opencode");
    assert_eq!(event.session.as_deref(), Some("open-live"));
    assert_eq!(event.file.as_deref(), Some("src/lib.rs"));
    assert_eq!(event.rationale.as_deref(), Some("Patch parser branch."));
}

#[test]
fn opencode_wrapped_plugin_diff_changed_preserves_file_range_and_rationale() {
    let raw = json!({
        "event": {
            "type": "diff.changed",
            "properties": {
                "sessionID": "open-live",
                "path": "src/lib.rs",
                "line": 12,
                "lineEnd": 18,
                "rationale": "Patch adapter diff handling."
            }
        }
    });

    let event =
        normalize_adapter_event(AgentKind::OpenCode, None, &raw).expect("normalized repo diff");

    assert_eq!(event.kind, EventKind::RepoDiff);
    assert_eq!(event.agent, "opencode");
    assert_eq!(event.session.as_deref(), Some("open-live"));
    assert_eq!(event.file.as_deref(), Some("src/lib.rs"));
    assert_eq!(event.line, Some(12));
    assert_eq!(event.line_end, Some(18));
    assert_eq!(
        event.rationale.as_deref(),
        Some("Patch adapter diff handling.")
    );
}

#[test]
fn opencode_session_idle_normalizes_to_agent_health_event() {
    let raw = json!({
        "event": "session.idle",
        "session": { "id": "open-live" },
        "content": "This is a good point to stop. Should I continue?"
    });

    let event =
        normalize_adapter_event(AgentKind::OpenCode, None, &raw).expect("normalized idle event");

    assert_eq!(event.kind, EventKind::AgentHealth);
    assert_eq!(event.agent, "opencode");
    assert_eq!(event.session.as_deref(), Some("open-live"));
    assert_eq!(event.content.as_deref(), Some("session idle"));
}

#[test]
fn opencode_session_error_normalizes_rate_limit_taxonomy() {
    let raw = json!({
        "event": "session.error",
        "session": { "id": "open-live" },
        "error": {
            "code": "rate_limit_exceeded",
            "message": "provider returned 429"
        }
    });

    let event =
        normalize_adapter_event(AgentKind::OpenCode, None, &raw).expect("normalized session error");

    assert_eq!(event.kind, EventKind::AgentHealth);
    assert_eq!(event.agent, "opencode");
    assert_eq!(event.session.as_deref(), Some("open-live"));
    assert_eq!(
        event.content.as_deref(),
        Some("session error [rate_limit]: provider returned 429")
    );
}

#[test]
fn opencode_wrapped_session_compacted_normalizes_to_context_compaction_health_event() {
    let raw = json!({
        "event": {
            "type": "session.compacted",
            "session": { "id": "open-live" }
        }
    });

    let event = normalize_adapter_event(AgentKind::OpenCode, None, &raw)
        .expect("normalized wrapped compaction event");

    assert_eq!(event.kind, EventKind::AgentHealth);
    assert_eq!(event.agent, "opencode");
    assert_eq!(event.session.as_deref(), Some("open-live"));
    assert_eq!(
        event.content.as_deref(),
        Some("context compaction completed")
    );
}

#[test]
fn claude_session_error_normalizes_context_limit_taxonomy() {
    let raw = json!({
        "type": "session_error",
        "session_id": "claude-live",
        "error_type": "context_length_exceeded",
        "message": "maximum context length exceeded"
    });

    let event = normalize_adapter_event(AgentKind::ClaudeCode, None, &raw)
        .expect("normalized session error");

    assert_eq!(event.kind, EventKind::AgentHealth);
    assert_eq!(event.agent, "claude-code");
    assert_eq!(event.session.as_deref(), Some("claude-live"));
    assert_eq!(
        event.content.as_deref(),
        Some("session error [context_limit]: maximum context length exceeded")
    );
}

#[test]
fn claude_stop_failure_normalizes_to_session_error_health_event() {
    let raw = json!({
        "hook_event_name": "StopFailure",
        "session_id": "claude-live",
        "error_type": "rate_limit_exceeded",
        "message": "provider returned 429"
    });

    let event = normalize_adapter_event(AgentKind::ClaudeCode, None, &raw)
        .expect("normalized stop failure");

    assert_eq!(event.kind, EventKind::AgentHealth);
    assert_eq!(event.agent, "claude-code");
    assert_eq!(event.session.as_deref(), Some("claude-live"));
    assert_eq!(
        event.content.as_deref(),
        Some("session error [rate_limit]: provider returned 429")
    );
}

#[test]
fn claude_subagent_stop_normalizes_to_agent_health_event() {
    let raw = json!({
        "hook_event_name": "SubagentStop",
        "session_id": "claude-live",
        "transcript_path": ".claude/projects/session.jsonl"
    });

    let event = normalize_adapter_event(AgentKind::ClaudeCode, None, &raw)
        .expect("normalized subagent stop");

    assert_eq!(event.kind, EventKind::AgentHealth);
    assert_eq!(event.agent, "claude-code");
    assert_eq!(event.session.as_deref(), Some("claude-live"));
    assert_eq!(event.content.as_deref(), Some("subagent stopped"));
}

#[test]
fn claude_posttooluse_failure_normalizes_to_failed_command_result() {
    let raw = json!({
        "hook_event_name": "PostToolUseFailure",
        "session_id": "claude-live",
        "tool_name": "Bash",
        "tool_input": { "command": "npm test" },
        "error": "process exited with status 1"
    });

    let event = normalize_adapter_event(AgentKind::ClaudeCode, None, &raw)
        .expect("normalized post-tool failure");

    assert_eq!(event.kind, EventKind::CommandResult);
    assert_eq!(event.agent, "claude-code");
    assert_eq!(event.session.as_deref(), Some("claude-live"));
    assert_eq!(event.command.as_deref(), Some("npm test"));
    assert_eq!(event.exit_code, Some(1));
    assert_eq!(
        event.content.as_deref(),
        Some("process exited with status 1")
    );
}

#[test]
fn claude_pretooluse_denied_normalizes_to_permission_intervention_result() {
    let raw = json!({
        "hook_event_name": "PreToolUse",
        "session_id": "claude-live",
        "decision": "deny",
        "reason": "permission denied by policy",
        "tool_name": "Bash",
        "tool_input": { "command": "git clean -fdx" }
    });

    let event = normalize_adapter_event(AgentKind::ClaudeCode, None, &raw)
        .expect("normalized permission denial");

    assert_eq!(event.kind, EventKind::InterventionResult);
    assert_eq!(event.agent, "claude-code");
    assert_eq!(event.session.as_deref(), Some("claude-live"));
    assert_eq!(event.command.as_deref(), Some("git clean -fdx"));
    assert_eq!(
        event.content.as_deref(),
        Some("permission denied: permission denied by policy")
    );
}

#[test]
fn explicit_permission_denied_event_normalizes_to_permission_intervention_result() {
    let raw = json!({
        "event": "permission.denied",
        "session": { "id": "open-live" },
        "reason": "policy blocked network access",
        "tool": "bash",
        "input": { "command": "curl https://example.com" }
    });

    let event = normalize_adapter_event(AgentKind::OpenCode, None, &raw)
        .expect("normalized permission denial");

    assert_eq!(event.kind, EventKind::InterventionResult);
    assert_eq!(event.agent, "opencode");
    assert_eq!(event.session.as_deref(), Some("open-live"));
    assert_eq!(event.command.as_deref(), Some("curl https://example.com"));
    assert_eq!(
        event.content.as_deref(),
        Some("permission denied: policy blocked network access")
    );
}

#[test]
fn explicit_permission_requested_event_normalizes_to_permission_intervention_result() {
    let raw = json!({
        "event": "permission.requested",
        "session": { "id": "open-live" },
        "reason": "needs approval for protected workflow edit",
        "tool": "bash",
        "input": { "command": "git status --short" }
    });

    let event = normalize_adapter_event(AgentKind::OpenCode, None, &raw)
        .expect("normalized permission request");

    assert_eq!(event.kind, EventKind::InterventionResult);
    assert_eq!(event.agent, "opencode");
    assert_eq!(event.session.as_deref(), Some("open-live"));
    assert_eq!(event.command.as_deref(), Some("git status --short"));
    assert_eq!(
        event.content.as_deref(),
        Some("permission requested: needs approval for protected workflow edit")
    );
}

#[test]
fn claude_pretooluse_ask_normalizes_to_permission_request_intervention_result() {
    let raw = json!({
        "hook_event_name": "PreToolUse",
        "session_id": "claude-live",
        "decision": "ask",
        "reason": "needs approval for destructive command",
        "tool_name": "Bash",
        "tool_input": { "command": "git clean -fdx" }
    });

    let event = normalize_adapter_event(AgentKind::ClaudeCode, None, &raw)
        .expect("normalized permission request");

    assert_eq!(event.kind, EventKind::InterventionResult);
    assert_eq!(event.agent, "claude-code");
    assert_eq!(event.session.as_deref(), Some("claude-live"));
    assert_eq!(event.command.as_deref(), Some("git clean -fdx"));
    assert_eq!(
        event.content.as_deref(),
        Some("permission requested: needs approval for destructive command")
    );
}

#[test]
fn claude_pretooluse_ask_emits_ask_hook_response() {
    let raw = json!({
        "hook_event_name": "PreToolUse",
        "session_id": "claude-live",
        "decision": "ask",
        "reason": "needs approval for destructive command",
        "tool_name": "Bash",
        "tool_input": { "command": "git clean -fdx" }
    });

    let response = adapter_hook_response(
        AgentKind::ClaudeCode,
        None,
        &raw,
        &SecurityConfig::default(),
    )
    .expect("hook response");

    assert_eq!(response.decision, AdapterHookDecision::Ask);
    assert_eq!(response.session.as_deref(), Some("claude-live"));
    assert_eq!(
        response.reason.as_deref(),
        Some("permission requested: needs approval for destructive command")
    );
}

#[test]
fn claude_pretooluse_allow_normalizes_to_tool_call_event() {
    let raw = json!({
        "hook_event_name": "PreToolUse",
        "session_id": "claude-live",
        "decision": "allow",
        "tool_name": "Bash",
        "tool_input": { "command": "git status --short" }
    });

    let event =
        normalize_adapter_event(AgentKind::ClaudeCode, None, &raw).expect("normalized tool call");

    assert_eq!(event.kind, EventKind::ToolCall);
    assert_eq!(event.agent, "claude-code");
    assert_eq!(event.session.as_deref(), Some("claude-live"));
    assert_eq!(event.command.as_deref(), Some("git status --short"));
    assert_eq!(
        event.content.as_deref(),
        Some("tool command: git status --short")
    );
}

#[test]
fn claude_pretooluse_destructive_command_without_decision_requests_permission() {
    let raw = json!({
        "hook_event_name": "PreToolUse",
        "session_id": "claude-live",
        "tool_name": "Bash",
        "tool_input": { "command": "git clean -fdx" }
    });

    let event = normalize_adapter_event(AgentKind::ClaudeCode, None, &raw)
        .expect("normalized destructive pre-tool use");

    assert_eq!(event.kind, EventKind::InterventionResult);
    assert_eq!(event.agent, "claude-code");
    assert_eq!(event.session.as_deref(), Some("claude-live"));
    assert_eq!(event.command.as_deref(), Some("git clean -fdx"));
    assert_eq!(
        event.content.as_deref(),
        Some("permission requested for command: git clean -fdx")
    );
}

#[test]
fn claude_pretooluse_destructive_command_emits_block_hook_response() {
    let raw = json!({
        "hook_event_name": "PreToolUse",
        "session_id": "claude-live",
        "tool_name": "Bash",
        "tool_input": { "command": "git clean -fdx" }
    });

    let response = adapter_hook_response(
        AgentKind::ClaudeCode,
        None,
        &raw,
        &SecurityConfig::default(),
    )
    .expect("hook response");

    assert_eq!(response.decision, AdapterHookDecision::Block);
    assert_eq!(response.session.as_deref(), Some("claude-live"));
    let reason = response.reason.as_deref().expect("block reason");
    assert!(reason.contains("destructive command"));
    assert!(reason.contains("git clean -fdx"));
}

#[test]
fn codex_pretooluse_apply_patch_protected_path_emits_block_hook_response() {
    let raw = json!({
        "hook_event_name": "PreToolUse",
        "session_id": "codex-live",
        "tool_name": "apply_patch",
        "tool_input": {
            "command": "*** Begin Patch\n*** Update File: .github/workflows/deploy.yml\n@@\n-old\n+new\n*** End Patch\n"
        }
    });

    let response = adapter_hook_response(AgentKind::Codex, None, &raw, &SecurityConfig::default())
        .expect("hook response");

    assert_eq!(response.decision, AdapterHookDecision::Block);
    assert_eq!(response.session.as_deref(), Some("codex-live"));
    let reason = response.reason.as_deref().expect("block reason");
    assert!(reason.contains("protected path"));
    assert!(reason.contains(".github/workflows/deploy.yml"));
}

#[test]
fn codex_pretooluse_apply_patch_patch_field_protected_path_emits_block_hook_response() {
    let raw = json!({
        "hook_event_name": "PreToolUse",
        "session_id": "codex-live",
        "tool_name": "apply_patch",
        "tool_input": {
            "patch": "*** Begin Patch\n*** Update File: .github/workflows/deploy.yml\n@@\n-old\n+new\n*** End Patch\n"
        }
    });

    let response = adapter_hook_response(AgentKind::Codex, None, &raw, &SecurityConfig::default())
        .expect("hook response");

    assert_eq!(response.decision, AdapterHookDecision::Block);
    assert_eq!(response.session.as_deref(), Some("codex-live"));
    let reason = response.reason.as_deref().expect("block reason");
    assert!(reason.contains("protected path"));
    assert!(reason.contains(".github/workflows/deploy.yml"));
}

#[test]
fn codex_pretooluse_apply_patch_move_to_protected_path_emits_block_hook_response() {
    let raw = json!({
        "hook_event_name": "PreToolUse",
        "session_id": "codex-live",
        "tool_name": "apply_patch",
        "tool_input": {
            "command": "*** Begin Patch\n*** Update File: scripts/deploy.yml\n*** Move to: .github/workflows/deploy.yml\n@@\n-old\n+new\n*** End Patch\n"
        }
    });

    let response = adapter_hook_response(AgentKind::Codex, None, &raw, &SecurityConfig::default())
        .expect("hook response");

    assert_eq!(response.decision, AdapterHookDecision::Block);
    assert_eq!(response.session.as_deref(), Some("codex-live"));
    let reason = response.reason.as_deref().expect("block reason");
    assert!(reason.contains("protected path"));
    assert!(reason.contains(".github/workflows/deploy.yml"));
}

#[test]
fn opencode_tool_execute_before_protected_path_emits_block_hook_response() {
    let raw = json!({
        "event": "tool.execute.before",
        "session": { "id": "open-live" },
        "tool": "edit",
        "input": { "path": ".github/workflows/deploy.yml" }
    });

    let response =
        adapter_hook_response(AgentKind::OpenCode, None, &raw, &SecurityConfig::default())
            .expect("hook response");

    assert_eq!(response.decision, AdapterHookDecision::Block);
    assert_eq!(response.session.as_deref(), Some("open-live"));
    let reason = response.reason.as_deref().expect("block reason");
    assert!(reason.contains("protected path"));
    assert!(reason.contains(".github/workflows/deploy.yml"));
}

#[test]
fn claude_pretooluse_read_emits_allow_hook_response() {
    let raw = json!({
        "hook_event_name": "PreToolUse",
        "session_id": "claude-live",
        "tool_name": "Read",
        "tool_input": { "file_path": "src/lib.rs" }
    });

    let response = adapter_hook_response(
        AgentKind::ClaudeCode,
        None,
        &raw,
        &SecurityConfig::default(),
    )
    .expect("hook response");

    assert_eq!(response.decision, AdapterHookDecision::Allow);
    assert_eq!(response.session.as_deref(), Some("claude-live"));
    assert_eq!(response.reason, None);
}

#[test]
fn claude_pretooluse_read_normalizes_tool_name_and_file_path() {
    let raw = json!({
        "hook_event_name": "PreToolUse",
        "session_id": "claude-live",
        "decision": "allow",
        "tool_name": "Read",
        "tool_input": { "file_path": "src/lib.rs" }
    });

    let event =
        normalize_adapter_event(AgentKind::ClaudeCode, None, &raw).expect("normalized tool call");

    assert_eq!(event.kind, EventKind::ToolCall);
    assert_eq!(event.agent, "claude-code");
    assert_eq!(event.session.as_deref(), Some("claude-live"));
    assert_eq!(event.command.as_deref(), Some("Read src/lib.rs"));
    assert_eq!(
        event.content.as_deref(),
        Some("tool command: Read src/lib.rs")
    );
}

#[test]
fn claude_pretooluse_task_normalizes_bounded_subagent_tool_label() {
    let raw = json!({
        "hook_event_name": "PreToolUse",
        "session_id": "claude-live",
        "decision": "allow",
        "tool_name": "Task",
        "tool_input": {
            "description": "Inspect parser invariants",
            "prompt": "Long subagent prompt that should not become a command label"
        }
    });

    let event =
        normalize_adapter_event(AgentKind::ClaudeCode, None, &raw).expect("normalized tool call");

    assert_eq!(event.kind, EventKind::ToolCall);
    assert_eq!(event.agent, "claude-code");
    assert_eq!(event.session.as_deref(), Some("claude-live"));
    assert_eq!(event.command.as_deref(), Some("Task"));
    assert_eq!(event.content.as_deref(), Some("tool command: Task"));
}

#[test]
fn claude_posttooluse_write_success_normalizes_to_file_change() {
    let raw = json!({
        "hook_event_name": "PostToolUse",
        "session_id": "claude-live",
        "tool_name": "Write",
        "tool_input": { "file_path": "src/lib.rs" },
        "status": "success",
        "reason": "Add monitor hook normalization."
    });

    let event = normalize_adapter_event(AgentKind::ClaudeCode, None, &raw)
        .expect("normalized write result");

    assert_eq!(event.kind, EventKind::FileChange);
    assert_eq!(event.agent, "claude-code");
    assert_eq!(event.session.as_deref(), Some("claude-live"));
    assert_eq!(event.file.as_deref(), Some("src/lib.rs"));
    assert_eq!(event.command.as_deref(), Some("Write src/lib.rs"));
    assert_eq!(
        event.rationale.as_deref(),
        Some("Add monitor hook normalization.")
    );
}

#[test]
fn codex_posttooluse_apply_patch_success_normalizes_to_file_change() {
    let raw = json!({
        "hook_event_name": "PostToolUse",
        "session_id": "codex-live",
        "tool_name": "apply_patch",
        "tool_input": {
            "command": "*** Begin Patch\n*** Update File: src/lib.rs\n@@\n-old\n+new\n*** End Patch\n"
        },
        "status": "success",
        "reason": "Patch monitor control path."
    });

    let event =
        normalize_adapter_event(AgentKind::Codex, None, &raw).expect("normalized patch result");

    assert_eq!(event.kind, EventKind::FileChange);
    assert_eq!(event.agent, "codex");
    assert_eq!(event.session.as_deref(), Some("codex-live"));
    assert_eq!(event.file.as_deref(), Some("src/lib.rs"));
    assert_eq!(event.command.as_deref(), Some("apply_patch src/lib.rs"));
    assert_eq!(
        event.rationale.as_deref(),
        Some("Patch monitor control path.")
    );
}

#[test]
fn codex_posttooluse_apply_patch_move_success_normalizes_to_destination_file_change() {
    let raw = json!({
        "hook_event_name": "PostToolUse",
        "session_id": "codex-live",
        "tool_name": "apply_patch",
        "tool_input": {
            "command": "*** Begin Patch\n*** Update File: src/old_name.rs\n*** Move to: src/new_name.rs\n@@\n-old\n+new\n*** End Patch\n"
        },
        "status": "success",
        "reason": "Rename monitored module."
    });

    let event =
        normalize_adapter_event(AgentKind::Codex, None, &raw).expect("normalized patch result");

    assert_eq!(event.kind, EventKind::FileChange);
    assert_eq!(event.agent, "codex");
    assert_eq!(event.session.as_deref(), Some("codex-live"));
    assert_eq!(event.file.as_deref(), Some("src/new_name.rs"));
    assert_eq!(
        event.command.as_deref(),
        Some("apply_patch src/new_name.rs")
    );
    assert_eq!(event.rationale.as_deref(), Some("Rename monitored module."));
}

#[test]
fn claude_posttooluse_bash_nested_response_normalizes_to_command_result() {
    let raw = json!({
        "hook_event_name": "PostToolUse",
        "session_id": "claude-live",
        "tool_name": "Bash",
        "tool_input": { "command": "npm test" },
        "tool_response": {
            "status": "failed",
            "exit_code": 1,
            "stdout": "upstream service unavailable"
        }
    });

    let event =
        normalize_adapter_event(AgentKind::ClaudeCode, None, &raw).expect("normalized bash result");

    assert_eq!(event.kind, EventKind::CommandResult);
    assert_eq!(event.agent, "claude-code");
    assert_eq!(event.session.as_deref(), Some("claude-live"));
    assert_eq!(event.command.as_deref(), Some("npm test"));
    assert_eq!(event.exit_code, Some(1));
    assert_eq!(
        event.content.as_deref(),
        Some("upstream service unavailable")
    );
}

#[test]
fn opencode_tool_execute_after_edit_success_normalizes_to_file_change() {
    let raw = json!({
        "event": "tool.execute.after",
        "session": { "id": "open-live" },
        "tool": "edit",
        "input": { "path": "src/adapter_ingest.rs" },
        "status": "succeeded",
        "rationale": "Normalize adapter write completion."
    });

    let event =
        normalize_adapter_event(AgentKind::OpenCode, None, &raw).expect("normalized edit result");

    assert_eq!(event.kind, EventKind::FileChange);
    assert_eq!(event.agent, "opencode");
    assert_eq!(event.session.as_deref(), Some("open-live"));
    assert_eq!(event.file.as_deref(), Some("src/adapter_ingest.rs"));
    assert_eq!(event.command.as_deref(), Some("edit src/adapter_ingest.rs"));
    assert_eq!(
        event.rationale.as_deref(),
        Some("Normalize adapter write completion.")
    );
}

#[test]
fn failed_file_edit_tool_result_stays_non_change_event() {
    let raw = json!({
        "event": "tool.execute.after",
        "session": { "id": "open-live" },
        "tool": "edit",
        "input": { "path": "src/lib.rs" },
        "status": "failed",
        "error": { "message": "patch did not apply" }
    });

    let event = normalize_adapter_event(AgentKind::OpenCode, None, &raw)
        .expect("normalized failed edit result");

    assert_eq!(event.kind, EventKind::CommandResult);
    assert_eq!(event.file, None);
    assert_eq!(event.command.as_deref(), Some("edit src/lib.rs"));
    assert_eq!(event.exit_code, Some(1));
}

#[test]
fn opencode_wrapped_plugin_tool_execute_after_failed_edit_uses_properties_exit_code_without_file_change()
 {
    let raw = json!({
        "event": {
            "type": "tool.execute.after",
            "properties": {
                "sessionID": "open-live",
                "tool": "edit",
                "input": { "path": "src/lib.rs" },
                "exitCode": 1,
                "error": { "message": "patch did not apply" }
            }
        }
    });

    let event = normalize_adapter_event(AgentKind::OpenCode, None, &raw)
        .expect("normalized failed wrapped edit result");

    assert_eq!(event.kind, EventKind::CommandResult);
    assert_eq!(event.agent, "opencode");
    assert_eq!(event.session.as_deref(), Some("open-live"));
    assert_eq!(event.command.as_deref(), Some("edit src/lib.rs"));
    assert_eq!(event.exit_code, Some(1));
    assert_eq!(event.content.as_deref(), Some("patch did not apply"));
    assert_eq!(event.file, None);
}

#[test]
fn claude_user_prompt_submit_normalizes_to_user_instruction() {
    let raw = json!({
        "hook_event_name": "UserPromptSubmit",
        "session_id": "claude-live",
        "prompt": "Acceptance: parser preserves comments."
    });

    let event =
        normalize_adapter_event(AgentKind::ClaudeCode, None, &raw).expect("normalized user prompt");

    assert_eq!(event.kind, EventKind::UserInstruction);
    assert_eq!(event.agent, "claude-code");
    assert_eq!(event.session.as_deref(), Some("claude-live"));
    assert_eq!(
        event.content.as_deref(),
        Some("Acceptance: parser preserves comments.")
    );
}

#[test]
fn claude_precompact_normalizes_to_context_compaction_health_event() {
    let raw = json!({
        "hook_event_name": "PreCompact",
        "session_id": "claude-live",
        "trigger": "auto"
    });

    let event = normalize_adapter_event(AgentKind::ClaudeCode, None, &raw)
        .expect("normalized pre-compact hook");

    assert_eq!(event.kind, EventKind::AgentHealth);
    assert_eq!(event.agent, "claude-code");
    assert_eq!(event.session.as_deref(), Some("claude-live"));
    assert_eq!(
        event.content.as_deref(),
        Some("context compaction requested: auto")
    );
}

#[test]
fn claude_notification_permission_wait_normalizes_to_permission_request() {
    let raw = json!({
        "hook_event_name": "Notification",
        "session_id": "claude-live",
        "message": "Claude needs your permission to use Bash",
        "tool_name": "Bash",
        "tool_input": { "command": "git clean -fdx" }
    });

    let event = normalize_adapter_event(AgentKind::ClaudeCode, None, &raw)
        .expect("normalized notification");

    assert_eq!(event.kind, EventKind::InterventionResult);
    assert_eq!(event.agent, "claude-code");
    assert_eq!(event.session.as_deref(), Some("claude-live"));
    assert_eq!(event.command.as_deref(), Some("git clean -fdx"));
    assert_eq!(
        event.content.as_deref(),
        Some("permission requested: Claude needs your permission to use Bash")
    );
}

#[test]
fn claude_notification_normalizes_to_agent_health_event() {
    let raw = json!({
        "hook_event_name": "Notification",
        "session_id": "claude-live",
        "message": "Claude Code is waiting for background shell output"
    });

    let event = normalize_adapter_event(AgentKind::ClaudeCode, None, &raw)
        .expect("normalized notification");

    assert_eq!(event.kind, EventKind::AgentHealth);
    assert_eq!(event.agent, "claude-code");
    assert_eq!(event.session.as_deref(), Some("claude-live"));
    assert_eq!(
        event.content.as_deref(),
        Some("notification: Claude Code is waiting for background shell output")
    );
}

#[test]
fn adapter_ingest_skips_malformed_line_and_continues_with_warning_event() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    let input = br#"{bad json}
{"type":"agent_message","id":"good-after-bad","message":"This is a good point to stop. Should I continue?"}
"#;
    let mut output = Vec::new();

    run_adapter_jsonl_with_store(
        &input[..],
        &mut output,
        AdapterIngestOptions {
            adapter: AgentKind::Codex,
            session: Some("codex-live".into()),
            config: Config::default(),
        },
        &mut store,
    )
    .expect("malformed adapter line should not stop ingest");

    let events =
        std::fs::read_to_string(temp.path().join(".agent-monitor/events.jsonl")).expect("events");
    assert!(events.contains("adapter ingest skipped malformed JSONL line 1"));
    assert!(!events.contains("key must be a string"));
    assert!(events.contains("\"event_id\":\"good-after-bad\""));

    assert!(output.is_empty());
    assert!(store.root().join("advice.jsonl").exists());
}

#[test]
fn adapter_ingest_skips_unsafe_pi_fallback_by_default() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    let input = br#"{"event":"tool.execute.after","tool":"bash","output":"upstream service unavailable","exit_code":1}
"#;
    let mut output = Vec::new();

    run_adapter_jsonl_with_store(
        &input[..],
        &mut output,
        AdapterIngestOptions {
            adapter: AgentKind::OpenCode,
            session: Some("open-live".into()),
            config: Config {
                open_work: true,
                retry_limit: 0,
                fallback_agents: vec!["pi".into()],
            },
        },
        &mut store,
    )
    .expect("ingest with configured policy");

    let intervention: Intervention =
        serde_json::from_slice(&output).expect("one intervention jsonl record");
    assert_eq!(intervention.kind, InterventionKind::ServiceFailure);
    assert_eq!(intervention.action, Action::RetrySameAgent);
    assert_eq!(intervention.agent.as_deref(), Some("opencode"));
    assert!(intervention.reason.contains("no fallback"));
}

#[test]
fn adapter_ingest_allows_configured_safe_pi_fallback() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    std::fs::write(
        store.root().join("config.json"),
        r#"{
          "adapters": {
            "pi": {
              "supports_workspace_write_mode": true,
              "requires_external_sandbox": false
            }
          }
        }"#,
    )
    .expect("config");
    let input = br#"{"event":"tool.execute.after","tool":"bash","output":"upstream service unavailable","exit_code":1}
"#;
    let mut output = Vec::new();

    run_adapter_jsonl_with_store(
        &input[..],
        &mut output,
        AdapterIngestOptions {
            adapter: AgentKind::OpenCode,
            session: Some("open-live".into()),
            config: Config {
                open_work: true,
                retry_limit: 0,
                fallback_agents: vec!["pi".into()],
            },
        },
        &mut store,
    )
    .expect("ingest with configured policy");

    let intervention: Intervention =
        serde_json::from_slice(&output).expect("one intervention jsonl record");
    assert_eq!(intervention.kind, InterventionKind::ServiceFailure);
    assert_eq!(intervention.action, Action::SwitchAgent);
    assert_eq!(intervention.agent.as_deref(), Some("pi"));
}

#[test]
fn adapter_ingest_skips_disabled_fallback_agent() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    std::fs::write(
        store.root().join("config.json"),
        r#"{
          "adapters": {
            "claude_code": {
              "enabled": false
            }
          }
        }"#,
    )
    .expect("config");
    let input = br#"{"event":"tool.execute.after","tool":"bash","output":"upstream service unavailable","exit_code":1}
"#;
    let mut output = Vec::new();

    run_adapter_jsonl_with_store(
        &input[..],
        &mut output,
        AdapterIngestOptions {
            adapter: AgentKind::OpenCode,
            session: Some("open-live".into()),
            config: Config {
                open_work: true,
                retry_limit: 0,
                fallback_agents: vec!["claude-code".into(), "codex".into()],
            },
        },
        &mut store,
    )
    .expect("ingest with fallback policy");

    let intervention: Intervention =
        serde_json::from_slice(&output).expect("one intervention jsonl record");
    assert_eq!(intervention.kind, InterventionKind::ServiceFailure);
    assert_eq!(intervention.action, Action::SwitchAgent);
    assert_eq!(intervention.agent.as_deref(), Some("codex"));
}

#[test]
fn adapter_ingest_records_session_idle_as_agent_health_without_triggering_policy() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    let input =
        br#"{"event":"session.idle","content":"This is a good point to stop. Should I continue?"}
"#;
    let mut output = Vec::new();

    run_adapter_jsonl_with_store(
        &input[..],
        &mut output,
        AdapterIngestOptions {
            adapter: AgentKind::OpenCode,
            session: Some("open-live".into()),
            config: Config::default(),
        },
        &mut store,
    )
    .expect("session idle event should be recorded as agent health");

    assert!(output.is_empty());
    let events =
        std::fs::read_to_string(temp.path().join(".agent-monitor/events.jsonl")).expect("events");
    assert!(events.contains("\"kind\":\"agent_health\""));
    assert!(events.contains("\"content\":\"session idle\""));
    assert!(!events.contains("This is a good point to stop"));
}

#[test]
fn adapter_session_idle_triggers_control_advice_for_existing_stale_verification() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:00:00Z".into()),
            event_id: Some("evt-idle-stale-write".into()),
            agent: "codex".into(),
            kind: EventKind::FileChange,
            file: Some("src/lib.rs".into()),
            rationale: Some("Change source before idle event.".into()),
            ..Event::default()
        })
        .expect("event");
    let input = br#"{"event":"session.idle","session":{"id":"open-live"}}
"#;
    let mut output = Vec::new();

    run_adapter_jsonl_with_store(
        &input[..],
        &mut output,
        AdapterIngestOptions {
            adapter: AgentKind::OpenCode,
            session: None,
            config: Config::default(),
        },
        &mut store,
    )
    .expect("session idle should trigger control advice");

    let advice_log =
        std::fs::read_to_string(store.root().join("advice.jsonl")).expect("advice log");
    let advice: AdviceRun =
        serde_json::from_str(advice_log.lines().next().expect("one advice")).expect("advice json");

    assert!(matches!(
        advice.final_action,
        ControlAction::ForceVerification { blocking: true, .. }
    ));
    assert!(
        advice
            .packet
            .evidence_refs
            .contains(&"evt-idle-stale-write".into())
    );
}

#[test]
fn adapter_successful_write_tool_result_triggers_stale_verification_advice() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    let input = br#"{"hook_event_name":"PostToolUse","session_id":"claude-live","tool_name":"Write","tool_input":{"file_path":"src/lib.rs"},"status":"success","reason":"Implement adapter write hook normalization."}
"#;
    let mut output = Vec::new();

    run_adapter_jsonl_with_store(
        &input[..],
        &mut output,
        AdapterIngestOptions {
            adapter: AgentKind::ClaudeCode,
            session: None,
            config: Config::default(),
        },
        &mut store,
    )
    .expect("successful write hook should trigger control advice");

    let events = std::fs::read_to_string(store.root().join("events.jsonl")).expect("events");
    assert!(events.contains("\"kind\":\"file_change\""));
    assert!(events.contains("\"file\":\"src/lib.rs\""));

    let advice_log =
        std::fs::read_to_string(store.root().join("advice.jsonl")).expect("advice log");
    let advice: AdviceRun =
        serde_json::from_str(advice_log.lines().next().expect("one advice")).expect("advice json");

    assert!(matches!(
        advice.final_action,
        ControlAction::ForceVerification { blocking: true, .. }
    ));
    assert_eq!(advice.packet.target_agent, "claude-code");
}

#[test]
fn codex_apply_patch_ingest_records_each_changed_file_without_duplicate_advice() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    let raw = json!({
        "hook_event_name": "PostToolUse",
        "event_id": "codex-patch-1",
        "session_id": "codex-live",
        "tool_name": "apply_patch",
        "tool_input": {
            "command": "*** Begin Patch\n*** Update File: src/agent.rs\n@@\n-old\n+new\n*** Update File: tests/agent.rs\n@@\n-old\n+new\n*** End Patch\n"
        },
        "status": "success",
        "reason": "Trace implementation and test changes together."
    });
    let input = format!("{raw}\n");
    let mut output = Vec::new();

    run_adapter_jsonl_with_store(
        input.as_bytes(),
        &mut output,
        AdapterIngestOptions {
            adapter: AgentKind::Codex,
            session: None,
            config: Config::default(),
        },
        &mut store,
    )
    .expect("multi-file patch should be ingested");

    let events = std::fs::read_to_string(store.root().join("events.jsonl")).expect("events");
    let changed_files = events
        .lines()
        .map(|line| serde_json::from_str::<Event>(line).expect("event json"))
        .filter(|event| event.kind == EventKind::FileChange)
        .map(|event| event.file.expect("file"))
        .collect::<Vec<_>>();
    assert_eq!(changed_files, vec!["src/agent.rs", "tests/agent.rs"]);

    let trace = std::fs::read_to_string(store.root().join("trace.jsonl")).expect("trace");
    let traced_files = trace
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("trace json"))
        .map(|entry| {
            entry
                .get("file")
                .and_then(serde_json::Value::as_str)
                .expect("trace file")
                .to_string()
        })
        .collect::<Vec<_>>();
    assert_eq!(traced_files, vec!["src/agent.rs", "tests/agent.rs"]);

    let advice_log =
        std::fs::read_to_string(store.root().join("advice.jsonl")).expect("advice log");
    assert_eq!(advice_log.lines().count(), 1);
}

#[test]
fn adapter_permission_request_triggers_bounded_user_decision_advice() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    let input = br#"{"hook_event_name":"PreToolUse","session_id":"claude-live","decision":"ask","reason":"needs approval for destructive command","tool_name":"Bash","tool_input":{"command":"git clean -fdx"}}
"#;
    let mut output = Vec::new();

    run_adapter_jsonl_with_store(
        &input[..],
        &mut output,
        AdapterIngestOptions {
            adapter: AgentKind::ClaudeCode,
            session: None,
            config: Config::default(),
        },
        &mut store,
    )
    .expect("permission request should trigger control advice");

    let events =
        std::fs::read_to_string(temp.path().join(".agent-monitor/events.jsonl")).expect("events");
    assert!(events.contains("\"kind\":\"intervention_result\""));
    assert!(events.contains("permission requested: needs approval for destructive command"));

    let advice_log =
        std::fs::read_to_string(store.root().join("advice.jsonl")).expect("advice log");
    let advice: AdviceRun =
        serde_json::from_str(advice_log.lines().next().expect("one advice")).expect("advice json");

    assert!(matches!(advice.final_action, ControlAction::AskUser { .. }));
    assert!(
        advice
            .packet
            .summary
            .contains("User authorization is required")
    );
}

#[test]
fn adapter_destructive_pretooluse_without_decision_triggers_user_decision_advice() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    let input = br#"{"hook_event_name":"PreToolUse","session_id":"claude-live","tool_name":"Bash","tool_input":{"command":"git clean -fdx"}}
"#;
    let mut output = Vec::new();

    run_adapter_jsonl_with_store(
        &input[..],
        &mut output,
        AdapterIngestOptions {
            adapter: AgentKind::ClaudeCode,
            session: None,
            config: Config::default(),
        },
        &mut store,
    )
    .expect("destructive pre-tool use should trigger control advice");

    let events =
        std::fs::read_to_string(temp.path().join(".agent-monitor/events.jsonl")).expect("events");
    assert!(events.contains("\"kind\":\"intervention_result\""));
    assert!(events.contains("permission requested for command: git clean -fdx"));

    let advice_log =
        std::fs::read_to_string(store.root().join("advice.jsonl")).expect("advice log");
    let advice: AdviceRun =
        serde_json::from_str(advice_log.lines().next().expect("one advice")).expect("advice json");

    assert!(matches!(advice.final_action, ControlAction::AskUser { .. }));
    assert!(
        advice
            .packet
            .summary
            .contains("User authorization is required")
    );
}

#[test]
fn repeated_permission_requests_trigger_permission_aware_retry_advice() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    let input = br#"{"event":"permission.requested","session":{"id":"open-live"},"reason":"needs approval for shell command","input":{"command":"git status --short"}}
{"event":"permission.requested","session":{"id":"open-live"},"reason":"needs approval for shell command","input":{"command":"git status --short"}}
"#;
    let mut output = Vec::new();

    run_adapter_jsonl_with_store(
        &input[..],
        &mut output,
        AdapterIngestOptions {
            adapter: AgentKind::OpenCode,
            session: None,
            config: Config::default(),
        },
        &mut store,
    )
    .expect("repeated permission requests should trigger control advice");

    let advice_log =
        std::fs::read_to_string(store.root().join("advice.jsonl")).expect("advice log");
    let last_advice = advice_log.lines().last().expect("latest advice");
    let advice: AdviceRun = serde_json::from_str(last_advice).expect("advice json");

    assert!(matches!(
        advice.final_action,
        ControlAction::RetryAgent {
            target_agent: Some(ref agent),
            max_attempts: 1
        } if agent == "opencode"
    ));
    assert!(
        advice
            .packet
            .summary
            .contains("repeated failure pattern requires one changed recovery step")
    );
}

#[test]
fn adapter_ingest_sanitizes_unsupported_event_type_before_warning() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    let input = br#"{"event":"This is a good point to stop. upstream service unavailable","content":"ignored"}
"#;
    let mut output = Vec::new();

    run_adapter_jsonl_with_store(
        &input[..],
        &mut output,
        AdapterIngestOptions {
            adapter: AgentKind::OpenCode,
            session: Some("open-live".into()),
            config: Config::default(),
        },
        &mut store,
    )
    .expect("unsupported event should be warning only");

    assert!(output.is_empty());
    let events =
        std::fs::read_to_string(temp.path().join(".agent-monitor/events.jsonl")).expect("events");
    assert!(events.contains("adapter ingest ignored unsupported event type"));
    assert!(!events.contains("good point to stop"));
    assert!(!events.contains("upstream service unavailable"));
}
