use super::*;

#[test]
fn case_file_evidence_preserves_event_source_and_redaction_provenance() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            event_id: Some("evt-provenance".into()),
            agent: "opencode".into(),
            kind: EventKind::ToolCall,
            content: Some("tool command: cargo test".into()),
            command: Some("cargo test".into()),
            source_type: Some("hook".into()),
            source_path: Some(".opencode/events.jsonl".into()),
            source_offset: Some(99123),
            source_hash: Some("blake3:abc123".into()),
            redaction_status: Some("tainted".into()),
            redaction_rules: vec!["env_secret".into(), "token_like".into()],
            ..Event::default()
        })
        .expect("event");
    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");

    let case_file = build_control_case_file(temp.path(), &snapshot);
    let evidence = case_file
        .evidence
        .iter()
        .find(|item| item.id == "evt-provenance")
        .expect("event evidence");

    assert_eq!(evidence.source.as_deref(), Some("cargo test"));
    assert_eq!(evidence.source_type.as_deref(), Some("hook"));
    assert_eq!(
        evidence.source_path.as_deref(),
        Some(".opencode/events.jsonl")
    );
    assert_eq!(evidence.source_offset, Some(99123));
    assert_eq!(evidence.source_hash.as_deref(), Some("blake3:abc123"));
    assert_eq!(evidence.redaction_status, RedactionStatus::Tainted);
    assert_eq!(
        evidence.redaction_rules,
        vec!["env_secret".to_string(), "token_like".to_string()]
    );
}

#[test]
fn case_file_replay_metadata_captures_repo_state_and_input_high_watermarks() {
    let temp = tempfile::tempdir().expect("temp dir");
    let head = init_git_repo(temp.path());
    std::fs::write(temp.path().join("seed.txt"), "dirty\n").expect("dirty workspace");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            event_id: Some("evt-low-seq".into()),
            seq: Some(7),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some("working".into()),
            ..Event::default()
        })
        .expect("low seq event");
    store
        .append_event(&Event {
            event_id: Some("evt-high-seq".into()),
            seq: Some(11),
            agent: "codex".into(),
            kind: EventKind::AgentHealth,
            content: Some("still active".into()),
            ..Event::default()
        })
        .expect("high seq event");
    store
        .append_verifier_run(&VerifierRun {
            verifier_run_id: "verifier-replay-boundary".into(),
            verifier_id: Some("rust_full".into()),
            command: "cargo test".into(),
            status: VerificationRunStatus::Passed,
            started_at: "2026-06-22T12:00:00Z".into(),
            completed_at: Some("2026-06-22T12:00:10Z".into()),
            exit_code: Some(0),
            output_digest: "fnv1a64:cbf29ce484222325".into(),
            failure_class: None,
        })
        .expect("verifier run");
    store
        .append_repo_hunk_history(&RepoHunkHistoryEntry {
            history_id: "repo-hunk-replay-boundary".into(),
            observed_at: "2026-06-22T12:00:11Z".into(),
            workspace: temp.path().display().to_string(),
            path: "seed.txt".into(),
            kind: RepoChangeKind::Modified,
            hunk_index: 0,
            old_start: 1,
            old_lines: 1,
            new_start: 1,
            new_lines: 1,
            trace_status: RepoTraceStatus::Untraced,
            matching_trace_count: 0,
            change_trace_status: RepoTraceStatus::Untraced,
            modified_at: Some(1),
            matching_trace_refs: Vec::new(),
        })
        .expect("repo hunk history");
    store
        .append_dev_history_report(&DevHistoryReport {
            workspace: temp.path().display().to_string(),
            generated_at: "2026-06-24T02:17:50Z".into(),
            sources: vec![DevHistorySourceReport {
                source: "codex".into(),
                history_root: "C:/Users/yys/.codex/sessions".into(),
                files: 1,
                bytes: 100,
                lines: 3,
                parsed: 3,
                sessions: 1,
                first_time: None,
                last_time: None,
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
                kind: "external_history_present".into(),
                severity: "info".into(),
                summary: "Local history evidence exists.".into(),
                evidence: vec!["1 local history file matched the workspace".into()],
                monitor_response: vec!["Use safe aggregate history evidence.".into()],
            }],
        })
        .expect("dev history");
    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");

    let case_file = build_control_case_file(temp.path(), &snapshot);

    assert_eq!(case_file.replay.git_head.as_deref(), Some(head.as_str()));
    assert!(
        case_file
            .replay
            .git_branch
            .as_deref()
            .is_some_and(|branch| !branch.is_empty())
    );
    assert_eq!(case_file.replay.git_dirty, Some(true));
    assert_eq!(case_file.replay.input.event_count, 2);
    assert_eq!(case_file.replay.input.max_event_seq, Some(11));
    assert_eq!(case_file.replay.input.intervention_count, 0);
    assert_eq!(case_file.replay.input.verifier_run_count, 1);
    assert_eq!(case_file.replay.input.repo_hunk_history_count, 1);
    assert_eq!(case_file.replay.input.dev_history_count, 1);
}
