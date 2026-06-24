use coding_agent_monitor::{
    AcceptanceCoverageStatus, Action, ActionOutcome, AdvisorCredentialSource, AgentActivityStatus,
    ControlAction, ControlActionKind, ControlCaseFile, ControlPacket, ControlRationale,
    DashboardAdvisorCredentialKind, DashboardFilter, DashboardOptions, DashboardRowKind,
    DashboardSeverity, DashboardSnapshot, DevHistoryFinding, DevHistoryReport,
    DevHistorySourceReport, EntropyDelta, EntropyKind, Event, EventKind, Intervention,
    InterventionKind, OutcomeStatus, PacketPreconditions, PacketUrgency, ProbeRun, ProbeSpec,
    ProjectStore, RepoChangeKind, RepoHunkHistoryEntry, RepoTraceStatus, RequirementNode,
    RequirementSource, RuntimeValidationSurface, ValidationOutcome, VerificationFailureClass,
    VerificationRunStatus, VerificationStatus, VerifierRun, WorktreeLockRequest,
    WorktreeLockResult, build_control_case_file,
};

#[test]
fn dashboard_snapshot_summarizes_project_store_logs() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");

    store
        .append_event(&Event {
            agent: "codex".into(),
            kind: EventKind::DesignThought,
            content: Some("Keep durable design memory small.".into()),
            ..Event::default()
        })
        .expect("event");
    store
        .append_design(&coding_agent_monitor::DesignEntry {
            time: None,
            agent: "codex".into(),
            session: None,
            content: "Keep durable design memory small.".into(),
        })
        .expect("design");
    store
        .append_trace(&coding_agent_monitor::TraceEntry {
            time: None,
            agent: "codex".into(),
            session: None,
            file: "src/lib.rs".into(),
            line: Some(10),
            rationale: Some("Add dashboard snapshot.".into()),
            ..coding_agent_monitor::TraceEntry::default()
        })
        .expect("trace");
    store
        .append_intervention(&Intervention {
            kind: InterventionKind::AgentDegraded,
            action: Action::SpawnFreshAgent,
            agent: Some("claude-code".into()),
            reason: "agent appears to have lost design memory".into(),
        })
        .expect("intervention");

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");

    assert_eq!(snapshot.event_count, 1);
    assert_eq!(snapshot.design_count, 1);
    assert_eq!(snapshot.trace_count, 1);
    assert_eq!(snapshot.intervention_count, 1);
    assert_eq!(snapshot.active_agents, vec!["claude-code", "codex"]);
    assert_eq!(snapshot.recent_events.len(), 1);
    assert_eq!(snapshot.recent_interventions.len(), 1);
    assert_eq!(snapshot.rows.len(), 2);
    assert_eq!(snapshot.rows[0].number, 1);
    assert_eq!(snapshot.rows[0].kind, DashboardRowKind::Event);
    assert_eq!(snapshot.rows[1].kind, DashboardRowKind::Intervention);
    assert_eq!(snapshot.agent_health[0].agent, "claude-code");
    assert_eq!(snapshot.agent_health[0].score, -3);
    assert_eq!(snapshot.agent_sessions.len(), 2);
    assert_eq!(snapshot.agent_sessions[0].agent, "claude-code");
    assert_eq!(
        snapshot.agent_sessions[0].status,
        AgentActivityStatus::Degraded
    );
    assert_eq!(snapshot.agent_sessions[1].agent, "codex");
    assert_eq!(
        snapshot.agent_sessions[1].status,
        AgentActivityStatus::Active
    );
    assert_eq!(snapshot.severity, DashboardSeverity::Critical);
}

#[test]
fn dashboard_snapshot_marks_dedicated_coding_plan_advisor_endpoint_healthy() {
    let temp = tempfile::tempdir().expect("temp dir");
    let store = ProjectStore::open(temp.path()).expect("store");
    std::fs::create_dir_all(store.root().join("credentials").join("coding-plan"))
        .expect("credential dir");
    std::fs::write(
        store
            .root()
            .join("credentials")
            .join("coding-plan")
            .join("auth.json"),
        r#"{"OPENAI_API_KEY":"eyJheader.payload.signature"}"#,
    )
    .expect("credential profile");
    std::fs::write(
        store.root().join("config.json"),
        r#"{
          "advisor": {
            "enabled": true,
            "provider": {
              "endpoint": "https://coding-plan.example.test/v1/chat/completions",
              "model": "coding-plan-advisor",
              "credential_source": "coding_plan",
              "credential_file": "credentials/coding-plan/auth.json"
            }
          }
        }"#,
    )
    .expect("config");

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");

    assert_eq!(snapshot.advisor_status.severity, DashboardSeverity::Healthy);
    assert_eq!(
        snapshot.advisor_status.credential_source,
        AdvisorCredentialSource::CodingPlan
    );
    assert_eq!(
        snapshot.advisor_status.credential_kind,
        DashboardAdvisorCredentialKind::JwtBearer
    );
    assert_eq!(
        snapshot.advisor_status.endpoint_host.as_deref(),
        Some("coding-plan.example.test")
    );
    assert!(snapshot.advisor_status.uses_dedicated_profile);
}

#[test]
fn dashboard_snapshot_flags_incompatible_public_openai_coding_plan_jwt_pairing() {
    let temp = tempfile::tempdir().expect("temp dir");
    let store = ProjectStore::open(temp.path()).expect("store");
    std::fs::create_dir_all(store.root().join("credentials").join("coding-plan"))
        .expect("credential dir");
    std::fs::write(
        store
            .root()
            .join("credentials")
            .join("coding-plan")
            .join("auth.json"),
        r#"{"OPENAI_API_KEY":"eyJheader.payload.signature"}"#,
    )
    .expect("credential profile");
    std::fs::write(
        store.root().join("config.json"),
        r#"{
          "advisor": {
            "enabled": true,
            "provider": {
              "endpoint": "https://api.openai.com/v1/chat/completions",
              "model": "gpt-5.5",
              "credential_source": "coding_plan",
              "credential_file": "credentials/coding-plan/auth.json"
            }
          }
        }"#,
    )
    .expect("config");

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");

    assert_eq!(snapshot.severity, DashboardSeverity::Critical);
    assert_eq!(
        snapshot.advisor_status.severity,
        DashboardSeverity::Critical
    );
    assert_eq!(
        snapshot.advisor_status.credential_kind,
        DashboardAdvisorCredentialKind::JwtBearer
    );
    assert!(snapshot.advisor_status.message.contains("api.openai.com"));
    assert!(!snapshot.advisor_status.message.contains("eyJheader"));
}

#[test]
fn dashboard_snapshot_rejects_completed_malformed_trailing_jsonl_record() {
    use std::io::Write;

    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");

    store
        .append_event(&Event {
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some("working".into()),
            ..Event::default()
        })
        .expect("event");
    let mut events = std::fs::OpenOptions::new()
        .append(true)
        .open(store.root().join("events.jsonl"))
        .expect("events log");
    events
        .write_all(b"{\"agent\":\"codex\"\n")
        .expect("completed malformed line");

    let error = DashboardSnapshot::load(store.root(), 20)
        .expect_err("completed malformed history should fail loudly");

    assert!(error.to_string().contains("events.jsonl"));
    assert!(error.to_string().contains("line 2"));
}

#[test]
fn dashboard_snapshot_ignores_unterminated_partial_trailing_jsonl_record() {
    use std::io::Write;

    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");

    store
        .append_event(&Event {
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some("working".into()),
            ..Event::default()
        })
        .expect("event");
    let mut events = std::fs::OpenOptions::new()
        .append(true)
        .open(store.root().join("events.jsonl"))
        .expect("events log");
    events
        .write_all(b"{\"agent\":\"codex\"")
        .expect("unterminated partial line");

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");

    assert_eq!(snapshot.event_count, 1);
    assert_eq!(snapshot.recent_events.len(), 1);
    assert_eq!(
        snapshot.recent_events[0].content.as_deref(),
        Some("working")
    );
}

#[test]
fn dashboard_snapshot_counts_only_completed_design_and_trace_records() {
    use std::io::Write;

    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");

    store
        .append_design(&coding_agent_monitor::DesignEntry {
            time: None,
            agent: "codex".into(),
            session: None,
            content: "Keep durable design memory small.".into(),
        })
        .expect("design");
    store
        .append_trace(&coding_agent_monitor::TraceEntry {
            time: None,
            agent: "codex".into(),
            session: None,
            file: "src/lib.rs".into(),
            line: Some(10),
            rationale: Some("Add dashboard replay count.".into()),
            ..coding_agent_monitor::TraceEntry::default()
        })
        .expect("trace");
    std::fs::OpenOptions::new()
        .append(true)
        .open(store.root().join("design.jsonl"))
        .expect("design log")
        .write_all(b"{\"agent\":\"codex\"")
        .expect("partial design");
    std::fs::OpenOptions::new()
        .append(true)
        .open(store.root().join("trace.jsonl"))
        .expect("trace log")
        .write_all(b"{\"agent\":\"codex\"")
        .expect("partial trace");

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");

    assert_eq!(snapshot.design_count, 1);
    assert_eq!(snapshot.trace_count, 1);
}

#[test]
fn dashboard_snapshot_includes_verifier_runs_as_capture_rows() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");

    store
        .append_verifier_run(&VerifierRun {
            verifier_run_id: "verifier-run-smoke".into(),
            verifier_id: Some("smoke".into()),
            command: "cargo test".into(),
            status: VerificationRunStatus::Passed,
            started_at: "2026-06-22T12:00:00Z".into(),
            completed_at: Some("2026-06-22T12:00:02Z".into()),
            exit_code: Some(0),
            output_digest: "fnv1a64:cbf29ce484222325".into(),
            failure_class: None,
        })
        .expect("verifier run");

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");

    assert_eq!(snapshot.verifier_run_count, 1);
    assert_eq!(snapshot.recent_verifier_runs.len(), 1);
    assert_eq!(
        snapshot.recent_verifier_runs[0].verifier_id.as_deref(),
        Some("smoke")
    );
    assert_eq!(snapshot.rows.len(), 1);
    assert_eq!(snapshot.rows[0].kind, DashboardRowKind::VerifierRun);
    assert_eq!(snapshot.rows[0].protocol, "verifier");
    assert!(snapshot.rows[0].summary.contains("smoke"));
    assert!(snapshot.rows[0].detail.contains("cargo test"));
}

#[test]
fn dashboard_snapshot_includes_probe_runs_as_capture_rows_and_case_evidence() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");

    store
        .append_probe_run(&ProbeRun {
            probe_run_id: "probe-run-local-evidence".into(),
            advice_id: "advice-run-probe".into(),
            probe: ProbeSpec::LocalEvidence {
                target: Some("routine_next_step".into()),
            },
            status: OutcomeStatus::Succeeded,
            started_at: "2026-06-24T09:00:00Z".into(),
            completed_at: Some("2026-06-24T09:00:01Z".into()),
            summary: "local evidence probe for `routine_next_step` observed 2 recent evidence event(s); repo inspection found 1 changed file(s), 1 untraced, 0 missing rationale".into(),
            evidence_ids: vec!["evt-routine-question".into(), "repo-audit-src-lib-rs".into()],
            note: None,
        })
        .expect("probe run");

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");

    assert_eq!(snapshot.probe_run_count, 1);
    assert_eq!(snapshot.recent_probe_runs.len(), 1);
    assert_eq!(snapshot.rows.len(), 1);
    assert_eq!(snapshot.rows[0].kind, DashboardRowKind::ProbeRun);
    assert_eq!(snapshot.rows[0].protocol, "probe");
    assert!(snapshot.rows[0].summary.contains("local_evidence"));
    assert!(snapshot.rows[0].summary.contains("succeeded"));
    assert!(snapshot.rows[0].detail.contains("routine_next_step"));

    let filter = DashboardFilter::parse("kind:probe-run text:routine").expect("filter");
    assert_eq!(snapshot.filtered_rows(&filter).len(), 1);

    let case_file = build_control_case_file(temp.path(), &snapshot);
    let evidence = case_file
        .evidence
        .iter()
        .find(|item| item.kind == "ProbeRun")
        .expect("probe run case evidence");
    assert_eq!(evidence.id, "probe-run-local-evidence");
    assert_eq!(evidence.source_type.as_deref(), Some("probe"));
    assert!(evidence.summary.contains("routine_next_step"));
}

#[test]
fn dashboard_snapshot_labels_runtime_validation_probe_runs_by_surface() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");

    store
        .append_probe_run(&ProbeRun {
            probe_run_id: "probe-run-runtime-validation".into(),
            advice_id: "advice-run-runtime-validation".into(),
            probe: ProbeSpec::RuntimeValidation {
                surface: RuntimeValidationSurface::MlSystem,
                target: Some("ranking model".into()),
            },
            status: OutcomeStatus::Succeeded,
            started_at: "2026-06-24T09:00:00Z".into(),
            completed_at: Some("2026-06-24T09:00:01Z".into()),
            summary: "runtime validation evidence probe for ML system `ranking model` observed 1 recent evidence event(s)".into(),
            evidence_ids: vec!["evt-ml-validation".into()],
            note: None,
        })
        .expect("probe run");

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");

    assert_eq!(snapshot.rows.len(), 1);
    assert_eq!(snapshot.rows[0].kind, DashboardRowKind::ProbeRun);
    assert!(snapshot.rows[0].summary.contains("runtime_validation"));
    assert!(snapshot.rows[0].summary.contains("ml_system"));
    assert!(snapshot.rows[0].detail.contains("ranking model"));
    assert!(
        !snapshot.rows[0].summary.contains("browser_validation"),
        "runtime validation probes should not be labeled as browser-only: {:?}",
        snapshot.rows[0]
    );
}

#[test]
fn dashboard_snapshot_includes_worktree_lock_events_as_capture_rows() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    let first = store
        .try_acquire_worktree_lock(&WorktreeLockRequest {
            worktree: temp.path().display().to_string(),
            owner_agent: "codex".into(),
            session: Some("s1".into()),
        })
        .expect("first lock");
    assert!(matches!(first, WorktreeLockResult::Acquired(_)));
    let second = store
        .try_acquire_worktree_lock(&WorktreeLockRequest {
            worktree: temp.path().display().to_string(),
            owner_agent: "claude-code".into(),
            session: Some("s2".into()),
        })
        .expect("conflicting lock");
    assert!(matches!(second, WorktreeLockResult::Conflict { .. }));

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");

    assert_eq!(snapshot.lock_event_count, 2);
    assert_eq!(snapshot.recent_worktree_lock_events.len(), 2);
    let filter =
        DashboardFilter::parse("kind:worktree-lock agent:codex text:claude-code").expect("filter");
    let rows = snapshot.filtered_rows(&filter);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].kind, DashboardRowKind::WorktreeLock);
    assert_eq!(rows[0].severity, DashboardSeverity::Warning);
    assert_eq!(rows[0].agent.as_deref(), Some("codex"));
    assert_eq!(rows[0].protocol, "worktree-lock");
    assert!(rows[0].summary.contains("conflict"));
    assert!(rows[0].summary.contains("claude-code"));
    assert!(rows[0].detail.contains("lock_id"));
    assert!(rows[0].detail.contains("owner_agent"));
    assert!(rows[0].detail.contains("session"));
    assert!(rows[0].detail.contains("requested_owner"));
    assert!(rows[0].detail.contains("worktree"));
}

#[test]
fn dashboard_snapshot_includes_dev_history_findings_as_capture_rows() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");

    store
        .append_dev_history_report(&DevHistoryReport {
            workspace: temp.path().display().to_string(),
            generated_at: "2026-06-24T02:17:50Z".into(),
            sources: vec![DevHistorySourceReport {
                source: "codex".into(),
                history_root: "C:/Users/yys/.codex/sessions".into(),
                files: 1,
                bytes: 42,
                lines: 3,
                parsed: 3,
                sessions: 1,
                first_time: Some("2026-06-24T02:00:00Z".into()),
                last_time: Some("2026-06-24T02:01:00Z".into()),
                subagent_files: None,
                top_types: Vec::new(),
                top_payload_types: Vec::new(),
                top_content_types: Vec::new(),
                top_tools: Vec::new(),
                top_command_heads: Vec::new(),
                top_signals: Vec::new(),
                top_file_refs: Vec::new(),
            }],
            findings: vec![DevHistoryFinding {
                kind: "verification_entropy".into(),
                severity: "critical".into(),
                summary: "History shows stale verification risk.".into(),
                evidence: vec!["17 verification signals".into()],
                monitor_response: vec!["Force verification before continue.".into()],
            }],
        })
        .expect("dev history");

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");

    assert_eq!(snapshot.dev_history_count, 1);
    assert_eq!(snapshot.recent_dev_history.len(), 1);
    assert_eq!(snapshot.rows.len(), 1);
    assert_eq!(snapshot.rows[0].kind, DashboardRowKind::DevHistory);
    assert_eq!(snapshot.rows[0].severity, DashboardSeverity::Critical);
    assert_eq!(snapshot.rows[0].protocol, "dev-history");
    assert!(snapshot.rows[0].summary.contains("verification_entropy"));
    assert!(snapshot.rows[0].detail.contains("Force verification"));

    let filter = DashboardFilter::parse("kind:dev-history severity:critical")
        .expect("parse dev-history filter");
    assert_eq!(snapshot.filtered_rows(&filter).len(), 1);
}

#[test]
fn dashboard_snapshot_includes_decision_trails_as_capture_rows() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            event_id: Some("evt-decision-write".into()),
            agent: "codex".into(),
            kind: EventKind::FileChange,
            file: Some("src/lib.rs".into()),
            rationale: Some("Changed source after verifier.".into()),
            ..Event::default()
        })
        .expect("event");
    let base_snapshot = DashboardSnapshot::load(store.root(), 20).expect("base snapshot");
    let case_file = build_control_case_file(temp.path(), &base_snapshot);
    store.append_case_file(&case_file).expect("case file");
    let packet = ControlPacket {
        packet_id: "packet-dashboard-decision".into(),
        target_agent: "codex".into(),
        urgency: PacketUrgency::Urgent,
        title: "Verification required".into(),
        summary: "Source changed after verifier.".into(),
        instructions: Vec::new(),
        evidence_refs: vec!["evt-decision-write".into()],
        forbidden: Vec::new(),
        success_criteria: Vec::new(),
        preconditions: PacketPreconditions::default(),
    };
    let dispatch = store
        .dispatch_control_packet(&packet)
        .expect("dispatch packet");
    let advice = coding_agent_monitor::AdviceRun {
        advice_id: "advice-dashboard-decision".into(),
        case_file_id: case_file.case_file_id.clone(),
        advisor_used: false,
        advisor_error: None,
        advisor_decision: None,
        validation_outcome: ValidationOutcome::Approved(ControlAction::ForceVerification {
            suite: coding_agent_monitor::VerificationSuite::Full,
            blocking: true,
        }),
        final_action: ControlAction::ForceVerification {
            suite: coding_agent_monitor::VerificationSuite::Full,
            blocking: true,
        },
        control_rationale: ControlRationale {
            selected_action: ControlActionKind::ForceVerification,
            dominant_entropy: Some(EntropyKind::Verification),
            reason: "verification entropy".into(),
            expected_entropy_delta: vec![EntropyDelta {
                kind: EntropyKind::Verification,
                delta: -55,
            }],
            evidence_ids: vec!["evt-decision-write".into()],
            requirement_ids: Vec::new(),
        },
        packet: packet.clone(),
        dispatch_result: dispatch.clone(),
        packet_path: dispatch.path.clone(),
    };
    store.append_advice(&advice).expect("advice");
    store
        .append_action_outcome(&ActionOutcome {
            outcome_id: "outcome-dashboard-decision".into(),
            advice_id: advice.advice_id.clone(),
            action: ControlActionKind::ForceVerification,
            status: OutcomeStatus::Succeeded,
            expected_entropy_delta: Vec::new(),
            observed_entropy_delta: vec![EntropyDelta {
                kind: EntropyKind::Verification,
                delta: -40,
            }],
            observed_entropy_delta_evidence: Vec::new(),
            evidence_ids: vec!["verifier-run-dashboard".into()],
            requirement_ids: Vec::new(),
            note: Some("verifier passed".into()),
        })
        .expect("outcome");

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");

    assert_eq!(snapshot.decision_trail_count, 1);
    assert_eq!(snapshot.recent_decision_trails.len(), 1);
    let row = snapshot
        .rows
        .iter()
        .find(|row| row.kind == DashboardRowKind::DecisionTrail)
        .expect("decision trail row");
    assert_eq!(row.protocol, "decision-trail");
    assert_eq!(row.agent.as_deref(), Some("codex"));
    assert!(row.summary.contains("force_verification"));
    assert!(row.summary.contains("packet-dashboard-decision"));
    assert!(row.summary.contains("outbox_written"));
    assert!(row.summary.contains("1 outcome"));
    assert!(row.detail.contains("advice-dashboard-decision"));
    assert!(row.detail.contains("outcome-dashboard-decision"));

    let filter = DashboardFilter::parse("kind:decision-trail agent:codex text:verification")
        .expect("filter");
    assert_eq!(snapshot.filtered_rows(&filter).len(), 1);
}

#[test]
fn dashboard_verifier_row_summary_includes_failure_class() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");

    store
        .append_verifier_run(&VerifierRun {
            verifier_run_id: "verifier-run-compile".into(),
            verifier_id: Some("compile".into()),
            command: "cargo test".into(),
            status: VerificationRunStatus::Failed,
            started_at: "2026-06-22T12:00:00Z".into(),
            completed_at: Some("2026-06-22T12:00:02Z".into()),
            exit_code: Some(101),
            output_digest: "fnv1a64:cbf29ce484222325".into(),
            failure_class: Some(VerificationFailureClass::Compile),
        })
        .expect("verifier run");

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");

    assert!(snapshot.rows[0].summary.contains("compile"));
    assert!(snapshot.rows[0].summary.contains("compile failure"));
}

#[test]
fn dashboard_snapshot_includes_repo_hunk_history_as_capture_rows() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");

    store
        .append_repo_hunk_history(&RepoHunkHistoryEntry {
            history_id: "repo-hunk-1".into(),
            observed_at: "2026-06-22T12:00:00Z".into(),
            workspace: temp.path().display().to_string(),
            path: "src/lib.rs".into(),
            kind: RepoChangeKind::Modified,
            hunk_index: 0,
            old_start: 2,
            old_lines: 1,
            new_start: 2,
            new_lines: 1,
            trace_status: RepoTraceStatus::Untraced,
            matching_trace_count: 0,
            change_trace_status: RepoTraceStatus::Untraced,
            modified_at: Some(1),
            matching_trace_refs: Vec::new(),
        })
        .expect("repo hunk history");

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");

    assert_eq!(snapshot.severity, DashboardSeverity::Warning);
    assert_eq!(snapshot.repo_hunk_history_count, 1);
    assert_eq!(snapshot.repo_hunk_file_count, 1);
    assert_eq!(snapshot.recent_repo_hunks.len(), 1);
    assert_eq!(snapshot.recent_repo_hunk_files.len(), 1);
    assert_eq!(snapshot.recent_repo_hunk_files[0].path, "src/lib.rs");
    assert_eq!(
        snapshot.recent_repo_hunk_files[0].worst_trace_status,
        RepoTraceStatus::Untraced
    );
    assert_eq!(snapshot.rows.len(), 2);
    assert_eq!(snapshot.rows[0].kind, DashboardRowKind::RepoHunkFile);
    assert_eq!(snapshot.rows[0].severity, DashboardSeverity::Warning);
    assert_eq!(snapshot.rows[0].protocol, "repo-hunk-file");
    assert!(snapshot.rows[0].summary.contains("src/lib.rs"));
    assert!(snapshot.rows[0].summary.contains("untraced"));
    assert_eq!(snapshot.rows[1].kind, DashboardRowKind::RepoHunk);
    assert_eq!(snapshot.rows[1].severity, DashboardSeverity::Warning);
    assert_eq!(snapshot.rows[1].protocol, "repo-hunk");
    assert!(snapshot.rows[1].summary.contains("src/lib.rs"));
    assert!(snapshot.rows[1].summary.contains("untraced"));

    let filter = DashboardFilter::parse("kind:repo-hunk text:src/lib.rs").expect("filter");
    assert_eq!(snapshot.filtered_rows(&filter).len(), 1);
    let filter = DashboardFilter::parse("kind:repo-hunk-file text:src/lib.rs").expect("filter");
    assert_eq!(snapshot.filtered_rows(&filter).len(), 1);
}

#[test]
fn dashboard_snapshot_includes_requirements_as_capture_rows() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    let base_snapshot = DashboardSnapshot::load(store.root(), 20).expect("base snapshot");
    let mut case_file: ControlCaseFile = build_control_case_file(temp.path(), &base_snapshot);
    case_file.requirements = vec![RequirementNode {
        requirement_id: "req-credential-boundary".into(),
        source: RequirementSource::AcceptanceCriterion,
        text: "Coding-plan credentials must feed only the advisor provider.".into(),
        source_event_id: Some("evt-user-credential-boundary".into()),
        evidence_ids: vec!["evt-user-credential-boundary".into()],
        evidence_refs: Vec::new(),
        verifier_ids: vec![],
        verifier_commands: vec![],
        latest_verification_evidence_id: None,
        status: AcceptanceCoverageStatus::Unmapped,
        latest_status: Some(VerificationStatus::NotRun),
    }];
    store.append_case_file(&case_file).expect("case file");

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");

    assert_eq!(snapshot.severity, DashboardSeverity::Warning);
    assert_eq!(snapshot.requirement_count, 1);
    assert_eq!(snapshot.recent_requirements.len(), 1);
    assert_eq!(snapshot.rows.len(), 1);
    assert_eq!(snapshot.rows[0].kind, DashboardRowKind::Requirement);
    assert_eq!(snapshot.rows[0].severity, DashboardSeverity::Warning);
    assert_eq!(snapshot.rows[0].protocol, "requirement");
    assert!(snapshot.rows[0].summary.contains("credential"));
    assert!(snapshot.rows[0].summary.contains("unmapped"));

    let filter = DashboardFilter::parse("kind:requirement text:advisor").expect("filter");
    assert_eq!(snapshot.filtered_rows(&filter).len(), 1);
}

#[test]
fn dashboard_requirement_rows_include_bounded_proof_history_in_detail() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    let base_snapshot = DashboardSnapshot::load(store.root(), 20).expect("base snapshot");
    let mut old_case: ControlCaseFile = build_control_case_file(temp.path(), &base_snapshot);
    old_case.case_file_id = "case-old-requirement-proof".into();
    old_case.requirements = vec![RequirementNode {
        requirement_id: "req-advisor-proof".into(),
        source: RequirementSource::AcceptanceCriterion,
        text: "Advisor decisions must cite evidence.".into(),
        source_event_id: Some("evt-user-requirement".into()),
        evidence_ids: vec!["evt-user-requirement".into()],
        evidence_refs: Vec::new(),
        verifier_ids: vec!["verifier-advisor".into()],
        verifier_commands: vec!["cargo test advisor".into()],
        latest_verification_evidence_id: Some("evt-old-verifier".into()),
        status: AcceptanceCoverageStatus::Stale,
        latest_status: Some(VerificationStatus::Stale),
    }];
    store.append_case_file(&old_case).expect("old case file");
    let mut new_case = old_case.clone();
    new_case.case_file_id = "case-new-requirement-proof".into();
    new_case.requirements[0].latest_verification_evidence_id = Some("evt-new-verifier".into());
    new_case.requirements[0].status = AcceptanceCoverageStatus::Covered;
    new_case.requirements[0].latest_status = Some(VerificationStatus::Passed);
    store.append_case_file(&new_case).expect("new case file");

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");

    assert_eq!(snapshot.recent_requirement_proofs.len(), 2);
    assert_eq!(
        snapshot.recent_requirement_proofs[0].case_file_id,
        "case-new-requirement-proof"
    );
    assert_eq!(snapshot.rows.len(), 1);
    assert_eq!(snapshot.rows[0].kind, DashboardRowKind::Requirement);
    assert!(snapshot.rows[0].detail.contains("\"proofs\""));
    assert!(
        snapshot.rows[0]
            .detail
            .contains("case-new-requirement-proof")
    );
    assert!(
        snapshot.rows[0]
            .detail
            .contains("case-old-requirement-proof")
    );
    assert!(snapshot.rows[0].detail.contains("evt-new-verifier"));
}

#[test]
fn dashboard_requirement_rows_warn_when_covered_requirement_has_weak_proof() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    let base_snapshot = DashboardSnapshot::load(store.root(), 20).expect("base snapshot");
    let mut case_file: ControlCaseFile = build_control_case_file(temp.path(), &base_snapshot);
    case_file.case_file_id = "case-weak-requirement-proof".into();
    case_file.requirements = vec![RequirementNode {
        requirement_id: "req-advisor-proof".into(),
        source: RequirementSource::AcceptanceCriterion,
        text: "Advisor decisions must cite evidence.".into(),
        source_event_id: Some("evt-user-requirement".into()),
        evidence_ids: vec!["evt-user-requirement".into()],
        evidence_refs: Vec::new(),
        verifier_ids: vec!["verifier-advisor".into()],
        verifier_commands: vec!["cargo test advisor".into()],
        latest_verification_evidence_id: Some("evt-new-verifier".into()),
        status: AcceptanceCoverageStatus::Covered,
        latest_status: Some(VerificationStatus::Passed),
    }];
    store.append_case_file(&case_file).expect("case file");

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");

    assert_eq!(snapshot.rows.len(), 1);
    assert_eq!(snapshot.rows[0].kind, DashboardRowKind::Requirement);
    assert_eq!(snapshot.rows[0].severity, DashboardSeverity::Warning);
    assert!(snapshot.rows[0].summary.contains("covered"));
    assert!(snapshot.rows[0].summary.contains("proof 30"));
    assert!(snapshot.rows[0].detail.contains("no_trace_refs"));
    assert!(snapshot.rows[0].detail.contains("no_outcome_refs"));
}

#[test]
fn dashboard_filter_supports_wireshark_style_terms() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");

    store
        .append_event(&Event {
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some("normal planning".into()),
            ..Event::default()
        })
        .expect("event");
    store
        .append_intervention(&Intervention {
            kind: InterventionKind::AgentDegraded,
            action: Action::SpawnFreshAgent,
            agent: Some("claude-code".into()),
            reason: "agent appears to have lost design memory".into(),
        })
        .expect("intervention");

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let filter = DashboardFilter::parse("kind:intervention agent:claude-code text:memory")
        .expect("filter should parse");
    let rows = snapshot.filtered_rows(&filter);

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].kind, DashboardRowKind::Intervention);
    assert_eq!(rows[0].agent.as_deref(), Some("claude-code"));
}

#[test]
fn dashboard_filter_rejects_unknown_fields() {
    let err = DashboardFilter::parse("protocol:http").expect_err("unknown field should fail");

    assert!(err.to_string().contains("protocol"));
}

#[test]
fn empty_dashboard_reports_no_live_agents() {
    let temp = tempfile::tempdir().expect("temp dir");
    let store = ProjectStore::open(temp.path()).expect("store");

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");

    assert!(snapshot.agent_sessions.is_empty());
    assert_eq!(snapshot.active_agents.len(), 0);
    assert_eq!(snapshot.severity, DashboardSeverity::Healthy);
}

#[test]
fn dashboard_marks_agent_stale_when_last_seen_is_old() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");

    store
        .append_event(&Event {
            time: Some("2026-06-22T12:00:00Z".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some("working".into()),
            ..Event::default()
        })
        .expect("event");

    let snapshot = DashboardSnapshot::load_with_options(
        store.root(),
        DashboardOptions {
            recent_limit: 20,
            now: Some("2026-06-22T12:10:01Z".into()),
            stale_after_secs: Some(300),
        },
    )
    .expect("snapshot");

    assert_eq!(
        snapshot.agent_sessions[0].status,
        AgentActivityStatus::Stale
    );
    assert_eq!(
        snapshot.agent_sessions[0].last_seen.as_deref(),
        Some("2026-06-22T12:00:00Z")
    );
}

#[test]
fn dashboard_snapshot_limits_recent_activity() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");

    for index in 0..5 {
        store
            .append_event(&Event {
                agent: "codex".into(),
                kind: EventKind::ModelMessage,
                content: Some(format!("event {index}")),
                ..Event::default()
            })
            .expect("event");
    }

    let snapshot = DashboardSnapshot::load(store.root(), 2).expect("snapshot");

    assert_eq!(snapshot.event_count, 5);
    assert_eq!(snapshot.recent_events.len(), 2);
    assert_eq!(
        snapshot.recent_events[0].content.as_deref(),
        Some("event 3")
    );
    assert_eq!(
        snapshot.recent_events[1].content.as_deref(),
        Some("event 4")
    );
}

#[test]
fn dashboard_snapshot_marks_critical_when_agent_health_is_bad() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");

    for _ in 0..3 {
        store
            .append_intervention(&Intervention {
                kind: InterventionKind::AgentDegraded,
                action: Action::SpawnFreshAgent,
                agent: Some("codex".into()),
                reason: "agent appears to have lost design memory".into(),
            })
            .expect("intervention");
    }

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");

    assert_eq!(snapshot.severity, DashboardSeverity::Critical);
    assert_eq!(snapshot.agent_health[0].score, -9);
}
