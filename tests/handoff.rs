use coding_agent_monitor::{
    AgentKind, ControlPacket, Event, EventKind, PacketInstruction, PacketInstructionPriority,
    PacketPreconditions, PacketUrgency, ProjectStore, VerificationRunStatus, VerifierRun,
    WorktreeLockRequest, WorktreeLockResult, handoff_workspace,
};

#[test]
fn handoff_workspace_writes_adapter_packet_with_memory_trace_and_verification() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:00:00Z".into()),
            event_id: Some("evt-memory".into()),
            run_id: Some("run-handoff".into()),
            agent: "codex".into(),
            session: Some("s1".into()),
            agent_session_id: Some("session-handoff".into()),
            kind: EventKind::DesignThought,
            content: Some(
                "The supervisor must judge agents externally and preserve durable design memory."
                    .into(),
            ),
            ..Event::default()
        })
        .expect("memory event");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:01:00Z".into()),
            event_id: Some("evt-change".into()),
            agent: "codex".into(),
            session: Some("s1".into()),
            kind: EventKind::FileChange,
            file: Some("src/lib.rs".into()),
            line: Some(42),
            rationale: Some("Add handoff packet generation.".into()),
            ..Event::default()
        })
        .expect("change event");
    store
        .append_verifier_run(&VerifierRun {
            verifier_run_id: "verifier-failed".into(),
            verifier_id: Some("rust_full".into()),
            command: "cargo test".into(),
            status: VerificationRunStatus::Failed,
            started_at: "2026-06-22T12:02:00Z".into(),
            completed_at: Some("2026-06-22T12:02:10Z".into()),
            exit_code: Some(1),
            output_digest: "fnv1a64:cbf29ce484222325".into(),
            failure_class: None,
        })
        .expect("verifier run");

    let handoff = handoff_workspace(temp.path(), AgentKind::ClaudeCode).expect("handoff");

    assert_eq!(handoff.packet.target_agent, "claude-code");
    assert!(handoff.packet.title.to_lowercase().contains("handoff"));
    assert!(handoff.packet.summary.contains("failed"));
    assert!(
        handoff
            .packet
            .instructions
            .iter()
            .any(|instruction| instruction.text.contains("durable design memory"))
    );
    assert!(
        handoff
            .packet
            .instructions
            .iter()
            .any(|instruction| instruction.text.contains("src/lib.rs")
                && instruction.text.contains("Add handoff packet generation"))
    );
    assert!(
        handoff
            .packet
            .evidence_refs
            .iter()
            .any(|id| id == "evt-change")
    );
    assert_eq!(handoff.case_file.memory_candidates.len(), 1);
    assert_eq!(
        handoff.packet.preconditions.adapter.as_deref(),
        Some("claude-code")
    );
    assert_eq!(handoff.packet.preconditions.run_id, None);
    assert_eq!(handoff.packet.preconditions.agent_session_id, None);
    assert_eq!(
        handoff.dispatch_result.path.as_deref(),
        handoff.packet_path.as_deref()
    );

    let path = handoff.packet_path.as_ref().expect("packet path");
    assert!(path.contains("claude-code"));
    let rendered = std::fs::read_to_string(path).expect("rendered packet");
    assert!(rendered.contains("Target agent: claude-code"));
    assert!(rendered.contains("Action: Fresh agent handoff"));
    assert!(rendered.contains("agent-monitor blame"));
    assert!(rendered.contains("evt-change"));
    assert!(rendered.contains("Top failure hypothesis"));
    assert!(rendered.contains("Missing evidence"));
    let latest_path = temp
        .path()
        .join(".agent-monitor")
        .join("outbox")
        .join("claude-code")
        .join("latest.md");
    let latest = std::fs::read_to_string(latest_path).expect("latest packet");
    assert_eq!(latest, rendered);
    assert!(store.root().join("case-files.jsonl").exists());
    assert!(store.root().join("packets.jsonl").exists());
    assert!(store.root().join("dispatch.jsonl").exists());
    let lock_log = std::fs::read_to_string(store.root().join("locks.jsonl"))
        .expect("handoff lock should be logged");
    assert!(lock_log.contains("\"kind\":\"acquired\""));
    assert!(lock_log.contains("\"owner_agent\":\"claude-code\""));
}

#[test]
fn handoff_workspace_rejects_locked_worktree_without_dispatching_packet() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    let existing = store
        .try_acquire_worktree_lock(&WorktreeLockRequest {
            worktree: temp.path().display().to_string(),
            owner_agent: "codex".into(),
            session: Some("s1".into()),
        })
        .expect("existing lock");
    assert!(matches!(existing, WorktreeLockResult::Acquired(_)));

    let error = handoff_workspace(temp.path(), AgentKind::ClaudeCode)
        .expect_err("locked handoff should fail");

    assert!(error.to_string().contains("already locked"));
    assert!(
        !store
            .root()
            .join("outbox")
            .join("claude-code")
            .join("latest.md")
            .exists()
    );
    let lock_log =
        std::fs::read_to_string(store.root().join("locks.jsonl")).expect("lock log should exist");
    assert!(lock_log.contains("\"kind\":\"conflict\""));
    assert!(lock_log.contains("\"requested_owner\":\"claude-code\""));
}

#[test]
fn handoff_workspace_rejects_writable_handoff_when_parallel_limit_is_zero() {
    let temp = tempfile::tempdir().expect("temp dir");
    let store = ProjectStore::open(temp.path()).expect("store");
    std::fs::write(
        store.root().join("config.json"),
        r#"{
          "policy": {
            "max_parallel_writable_agents": 0
          }
        }"#,
    )
    .expect("config");

    let error = handoff_workspace(temp.path(), AgentKind::ClaudeCode)
        .expect_err("parallel writable limit should block handoff");

    assert!(error.to_string().contains("max_parallel_writable_agents"));
    assert!(!store.root().join("outbox").join("claude-code").exists());
    assert!(!store.root().join("locks.jsonl").exists());
}

#[test]
fn handoff_workspace_rejects_pi_without_safe_capability_override() {
    let temp = tempfile::tempdir().expect("temp dir");
    let store = ProjectStore::open(temp.path()).expect("store");

    let error = handoff_workspace(temp.path(), AgentKind::Pi)
        .expect_err("pi handoff should require wrapper");

    assert!(error.to_string().contains("adapter capabilities"));
    assert!(!store.root().join("outbox").join("pi").exists());
    assert!(!store.root().join("locks.jsonl").exists());
}

#[test]
fn handoff_workspace_rejects_disabled_adapter_before_side_effects() {
    let temp = tempfile::tempdir().expect("temp dir");
    let store = ProjectStore::open(temp.path()).expect("store");
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

    let error = handoff_workspace(temp.path(), AgentKind::ClaudeCode)
        .expect_err("disabled adapter handoff should fail");

    assert!(error.to_string().contains("disabled"));
    assert!(!store.root().join("outbox").join("claude-code").exists());
    assert!(!store.root().join("locks.jsonl").exists());
}

#[test]
fn handoff_workspace_allows_pi_when_config_marks_wrapper_safe() {
    let temp = tempfile::tempdir().expect("temp dir");
    let store = ProjectStore::open(temp.path()).expect("store");
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

    let handoff = handoff_workspace(temp.path(), AgentKind::Pi).expect("pi handoff");

    assert_eq!(handoff.packet.target_agent, "pi");
    assert!(
        store
            .root()
            .join("outbox")
            .join("pi")
            .join("latest.md")
            .exists()
    );
    let lock_log =
        std::fs::read_to_string(store.root().join("locks.jsonl")).expect("lock log should exist");
    assert!(lock_log.contains("\"owner_agent\":\"pi\""));
}

#[test]
fn handoff_packet_prefers_active_durable_memory_over_unverified_candidates() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_memory(&coding_agent_monitor::MemoryCandidate {
            memory_id: "mem-active".into(),
            scope: coding_agent_monitor::MemoryScope::Project,
            claim: "Adapters must treat monitor packets as execution control.".into(),
            status: coding_agent_monitor::MemoryStatus::Active,
            source: coding_agent_monitor::MemorySource::ManualReview,
            evidence_ids: vec!["review-1".into()],
            confidence: 90,
        })
        .expect("memory");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:00:00Z".into()),
            event_id: Some("evt-unverified-memory".into()),
            agent: "codex".into(),
            kind: EventKind::DesignThought,
            content: Some("Unverified agent-only memory candidate.".into()),
            ..Event::default()
        })
        .expect("event");

    let handoff = handoff_workspace(temp.path(), AgentKind::ClaudeCode).expect("handoff");
    let memory_instruction = handoff
        .packet
        .instructions
        .iter()
        .find(|instruction| instruction.text.contains("durable memory"))
        .expect("memory instruction");

    assert!(
        memory_instruction
            .text
            .contains("Adapters must treat monitor packets as execution control")
    );
    assert!(
        memory_instruction
            .text
            .contains("Unverified memory candidate")
    );
    assert!(
        memory_instruction
            .text
            .find("Adapters must treat")
            .expect("active memory position")
            < memory_instruction
                .text
                .find("Unverified memory candidate")
                .expect("candidate position")
    );
}

#[test]
fn handoff_omits_upstream_tainted_memory_candidate_from_packet() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:00:00Z".into()),
            event_id: Some("evt-secret-memory".into()),
            agent: "codex".into(),
            session: Some("s1".into()),
            kind: EventKind::DesignThought,
            content: Some("Temporary token [REDACTED]".into()),
            redaction_status: Some("tainted".into()),
            redaction_rules: vec!["upstream_secret".into()],
            ..Event::default()
        })
        .expect("memory event");

    let handoff = handoff_workspace(temp.path(), AgentKind::ClaudeCode).expect("handoff");
    let rendered = std::fs::read_to_string(handoff.packet_path.expect("packet path"))
        .expect("rendered handoff packet");

    assert!(!rendered.contains("Temporary token"));
    assert!(rendered.contains("No durable design memory candidates are present"));
    assert!(store.root().join("case-files.jsonl").exists());
    assert!(store.root().join("packets.jsonl").exists());
    assert!(store.root().join("dispatch.jsonl").exists());
}

#[test]
fn packet_dispatch_refuses_id_collision_and_preserves_latest_pointer() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    let first = test_packet("packet-fixed", "First packet");
    let second = test_packet("packet-fixed", "Second packet");

    let first_path = store
        .write_control_packet(&first)
        .expect("first packet write");
    let collision = store
        .write_control_packet(&second)
        .expect_err("packet id collision should be rejected");

    assert!(collision.to_string().contains("already exists"));
    let immutable = std::fs::read_to_string(first_path).expect("immutable packet");
    let latest = std::fs::read_to_string(
        temp.path()
            .join(".agent-monitor")
            .join("outbox")
            .join("codex")
            .join("latest.md"),
    )
    .expect("latest packet");
    assert!(immutable.contains("First packet"));
    assert!(!immutable.contains("Second packet"));
    assert_eq!(latest, immutable);
}

#[test]
fn packet_dispatch_refuses_same_packet_id_with_different_urgency() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    let first = test_packet("packet-fixed", "First packet");
    let mut second = test_packet("packet-fixed", "Second packet");
    second.urgency = PacketUrgency::Urgent;

    store
        .write_control_packet(&first)
        .expect("first packet write");
    let collision = store
        .write_control_packet(&second)
        .expect_err("packet id collision should be rejected across urgencies");

    assert!(collision.to_string().contains("already exists"));
}

#[test]
fn packet_dispatch_updates_latest_pointer_for_new_packet() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    let first = test_packet("packet-one", "First packet");
    let second = test_packet("packet-two", "Second packet");

    store
        .write_control_packet(&first)
        .expect("first packet write");
    store
        .write_control_packet(&second)
        .expect("second packet write");

    let latest = std::fs::read_to_string(
        temp.path()
            .join(".agent-monitor")
            .join("outbox")
            .join("codex")
            .join("latest.md"),
    )
    .expect("latest packet");
    assert!(latest.contains("Second packet"));
    assert!(!latest.contains("First packet"));
}

#[test]
fn packet_dispatch_rejects_adapter_precondition_mismatch() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    let mut packet = test_packet("packet-adapter-mismatch", "Adapter mismatch");
    packet.target_agent = "codex".into();
    packet.preconditions.adapter = Some("claude-code".into());

    let error = store
        .dispatch_control_packet(&packet)
        .expect_err("adapter precondition mismatch should fail");

    assert!(error.to_string().contains("adapter"));
    assert!(!store.root().join("outbox").join("codex").exists());
    assert!(!store.root().join("dispatch.jsonl").exists());
}

#[test]
fn packet_dispatch_rejects_stale_run_id_precondition() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            event_id: Some("evt-current-run".into()),
            run_id: Some("run-current".into()),
            agent: "codex".into(),
            kind: EventKind::AgentHealth,
            content: Some("current run is active".into()),
            ..Event::default()
        })
        .expect("event");
    let mut packet = test_packet("packet-stale-run", "Stale run");
    packet.preconditions.run_id = Some("run-old".into());

    let error = store
        .dispatch_control_packet(&packet)
        .expect_err("stale run precondition should fail");

    assert!(error.to_string().contains("run_id"));
    assert!(!store.root().join("outbox").join("codex").exists());
    assert!(!store.root().join("dispatch.jsonl").exists());
}

#[test]
fn packet_dispatch_rejects_stale_agent_session_precondition() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            event_id: Some("evt-current-session".into()),
            agent: "codex".into(),
            session: Some("session-current".into()),
            agent_session_id: Some("agent-session-current".into()),
            kind: EventKind::AgentHealth,
            content: Some("current session is active".into()),
            ..Event::default()
        })
        .expect("event");
    let mut packet = test_packet("packet-stale-session", "Stale session");
    packet.preconditions.agent_session_id = Some("agent-session-old".into());

    let error = store
        .dispatch_control_packet(&packet)
        .expect_err("stale session precondition should fail");

    assert!(error.to_string().contains("agent_session_id"));
    assert!(!store.root().join("outbox").join("codex").exists());
    assert!(!store.root().join("dispatch.jsonl").exists());
}

#[test]
fn packet_dispatch_checks_run_and_session_preconditions_for_target_agent() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            event_id: Some("evt-codex-current".into()),
            run_id: Some("run-codex-current".into()),
            agent: "codex".into(),
            session: Some("codex-session-current".into()),
            agent_session_id: Some("codex-agent-session-current".into()),
            kind: EventKind::AgentHealth,
            content: Some("codex session is active".into()),
            ..Event::default()
        })
        .expect("codex event");
    store
        .append_event(&Event {
            event_id: Some("evt-claude-newer".into()),
            run_id: Some("run-claude-newer".into()),
            agent: "claude-code".into(),
            session: Some("claude-session-newer".into()),
            agent_session_id: Some("claude-agent-session-newer".into()),
            kind: EventKind::AgentHealth,
            content: Some("claude emitted a later event".into()),
            ..Event::default()
        })
        .expect("claude event");

    let mut packet = test_packet("packet-target-scoped-session", "Target scoped session");
    packet.target_agent = "codex".into();
    packet.preconditions.run_id = Some("run-codex-current".into());
    packet.preconditions.agent_session_id = Some("codex-agent-session-current".into());

    store
        .dispatch_control_packet(&packet)
        .expect("codex packet should validate against codex event");

    assert!(
        store
            .root()
            .join("outbox")
            .join("codex")
            .join("latest.md")
            .exists()
    );
    assert!(store.root().join("dispatch.jsonl").exists());
}

#[test]
fn dispatch_does_not_log_dispatch_when_latest_publication_fails() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    let outbox = temp
        .path()
        .join(".agent-monitor")
        .join("outbox")
        .join("codex");
    std::fs::create_dir_all(outbox.join("latest.md")).expect("blocking latest directory");

    let error = store
        .dispatch_control_packet(&test_packet("packet-blocked-latest", "Blocked latest"))
        .expect_err("latest publication should fail");

    assert!(error.to_string().contains("latest.md"));
    assert!(!store.root().join("dispatch.jsonl").exists());
}

fn test_packet(packet_id: &str, title: &str) -> ControlPacket {
    ControlPacket {
        packet_id: packet_id.into(),
        target_agent: "codex".into(),
        urgency: PacketUrgency::FollowUp,
        title: title.into(),
        summary: "packet summary".into(),
        instructions: vec![PacketInstruction {
            priority: PacketInstructionPriority::Must,
            text: "Follow this packet.".into(),
        }],
        evidence_refs: Vec::new(),
        forbidden: Vec::new(),
        success_criteria: Vec::new(),
        preconditions: PacketPreconditions::default(),
    }
}
