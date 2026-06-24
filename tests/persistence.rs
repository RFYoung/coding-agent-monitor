use coding_agent_monitor::{
    AdviceRun, Config, ControlAction, Event, EventKind, Intervention, InterventionKind,
    ProjectStore, TraceEntry, run_jsonl_with_store,
};
use std::{
    path::Path,
    process::Command,
    sync::{Arc, Barrier},
    thread,
};

#[test]
fn persisted_jsonl_records_events_interventions_design_and_trace() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store should open");
    let input = br#"{"agent":"codex","session":"s1","kind":"design_thought","content":"Store durable design, not raw transcript."}
{"agent":"codex","session":"s1","kind":"file_change","file":"src/lib.rs","line":12,"rationale":"Add persistent project store."}
{"agent":"codex","session":"s1","kind":"model_message","content":"upstream service unavailable"}
"#;
    let mut output = Vec::new();

    run_jsonl_with_store(&input[..], &mut output, Config::default(), &mut store)
        .expect("jsonl run should persist");

    let root = temp.path().join(".agent-monitor");
    assert!(root.join("events.jsonl").exists());
    assert!(root.join("interventions.jsonl").exists());
    assert!(root.join("design.jsonl").exists());
    assert!(root.join("trace.jsonl").exists());
    assert!(root.join("tmp").is_dir());

    let events = std::fs::read_to_string(root.join("events.jsonl")).expect("events log");
    assert_eq!(events.lines().count(), 3);

    let design = std::fs::read_to_string(root.join("design.jsonl")).expect("design log");
    assert!(design.contains("Store durable design"));

    let trace = std::fs::read_to_string(root.join("trace.jsonl")).expect("trace log");
    assert!(trace.contains("src/lib.rs"));
    assert!(trace.contains("Add persistent project store"));

    let interventions =
        std::fs::read_to_string(root.join("interventions.jsonl")).expect("intervention log");
    let intervention: Intervention =
        serde_json::from_str(interventions.lines().next().expect("one intervention"))
            .expect("intervention json");
    assert_eq!(intervention.kind, InterventionKind::ServiceFailure);
}

#[test]
fn persisted_jsonl_records_repo_diff_trace_entries() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store should open");
    let input = br#"{"event_id":"evt-diff","agent":"codex","session":"s1","kind":"repo_diff","file":"src/lib.rs","line":12,"rationale":"Update parser branch."}
"#;
    let mut output = Vec::new();

    run_jsonl_with_store(&input[..], &mut output, Config::default(), &mut store)
        .expect("jsonl run should persist");

    let trace = std::fs::read_to_string(temp.path().join(".agent-monitor").join("trace.jsonl"))
        .expect("trace log");
    assert!(trace.contains("\"event_id\":\"evt-diff\""));
    assert!(trace.contains("src/lib.rs"));
    assert!(trace.contains("Update parser branch"));
}

#[test]
fn store_exposes_controlled_temp_directory() {
    let temp = tempfile::tempdir().expect("temp dir");
    let store = ProjectStore::open(temp.path()).expect("store should open");

    assert_eq!(
        store.temp_dir(),
        temp.path().join(".agent-monitor").join("tmp")
    );
}

#[test]
fn store_appends_typed_events() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store should open");

    store
        .append_event(&Event {
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some("hello".into()),
            ..Event::default()
        })
        .expect("append event");

    let events = std::fs::read_to_string(temp.path().join(".agent-monitor").join("events.jsonl"))
        .expect("events log");
    assert!(events.contains("\"agent\":\"codex\""));
}

#[test]
fn store_assigns_event_identity_before_persisting() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store should open");

    store
        .append_event(&Event {
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some("first".into()),
            ..Event::default()
        })
        .expect("append first event");
    store
        .append_event(&Event {
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some("second".into()),
            ..Event::default()
        })
        .expect("append second event");

    let events = std::fs::read_to_string(temp.path().join(".agent-monitor").join("events.jsonl"))
        .expect("events log");
    let records = events
        .lines()
        .map(|line| serde_json::from_str::<Event>(line).expect("event json"))
        .collect::<Vec<_>>();

    assert_eq!(records.len(), 2);
    assert!(
        records[0]
            .event_id
            .as_deref()
            .is_some_and(|id| id.starts_with("event-")),
        "missing generated event id: {:?}",
        records[0].event_id
    );
    assert!(
        records[1]
            .event_id
            .as_deref()
            .is_some_and(|id| id.starts_with("event-")),
        "missing generated event id: {:?}",
        records[1].event_id
    );
    assert_ne!(records[0].event_id, records[1].event_id);
    assert_eq!(records[0].seq, Some(1));
    assert_eq!(records[1].seq, Some(2));
}

#[test]
fn store_assigns_unique_gap_free_event_seq_for_concurrent_appenders() {
    let temp = tempfile::tempdir().expect("temp dir");
    ProjectStore::open(temp.path()).expect("store should open");
    let workspace = Arc::new(temp.path().to_path_buf());
    let thread_count = 32;
    let barrier = Arc::new(Barrier::new(thread_count));
    let mut handles = Vec::new();

    for index in 0..thread_count {
        let workspace = Arc::clone(&workspace);
        let barrier = Arc::clone(&barrier);
        handles.push(thread::spawn(move || {
            let mut store = ProjectStore::open(workspace.as_path()).expect("thread store");
            barrier.wait();
            store
                .append_event(&Event {
                    agent: "codex".into(),
                    kind: EventKind::ModelMessage,
                    content: Some(format!("concurrent event {index}")),
                    ..Event::default()
                })
                .expect("append concurrent event");
        }));
    }

    for handle in handles {
        handle.join().expect("thread join");
    }

    let events = std::fs::read_to_string(workspace.join(".agent-monitor").join("events.jsonl"))
        .expect("events log");
    let mut seqs = events
        .lines()
        .map(|line| {
            serde_json::from_str::<Event>(line)
                .expect("event json")
                .seq
                .expect("assigned seq")
        })
        .collect::<Vec<_>>();
    seqs.sort_unstable();

    assert_eq!(seqs.len(), thread_count);
    assert_eq!(
        seqs,
        (1..=thread_count as u64).collect::<Vec<_>>(),
        "concurrent event seq values must be unique and gap-free"
    );
}

#[test]
fn store_serializes_concurrent_side_log_appends() {
    let temp = tempfile::tempdir().expect("temp dir");
    ProjectStore::open(temp.path()).expect("store should open");
    let workspace = Arc::new(temp.path().to_path_buf());
    let thread_count = 32;
    let barrier = Arc::new(Barrier::new(thread_count));
    let mut handles = Vec::new();

    for index in 0..thread_count {
        let workspace = Arc::clone(&workspace);
        let barrier = Arc::clone(&barrier);
        handles.push(thread::spawn(move || {
            let mut store = ProjectStore::open(workspace.as_path()).expect("thread store");
            barrier.wait();
            store
                .append_design(&coding_agent_monitor::DesignEntry {
                    time: None,
                    agent: "codex".into(),
                    session: Some(format!("s{index}")),
                    content: format!("design record {index}"),
                })
                .expect("append design record");
        }));
    }

    for handle in handles {
        handle.join().expect("thread join");
    }

    let design = std::fs::read_to_string(workspace.join(".agent-monitor").join("design.jsonl"))
        .expect("design log");
    let records = design
        .lines()
        .map(|line| {
            serde_json::from_str::<coding_agent_monitor::DesignEntry>(line)
                .expect("design json line")
        })
        .collect::<Vec<_>>();

    assert_eq!(records.len(), thread_count);
    for index in 0..thread_count {
        assert!(
            records
                .iter()
                .any(|record| record.content == format!("design record {index}")),
            "missing design record {index}: {records:?}"
        );
    }
}

#[test]
fn store_stamps_monitor_owned_event_provenance_when_missing() {
    let temp = tempfile::tempdir().expect("temp dir");
    let head = init_git_repo(temp.path());
    std::fs::write(temp.path().join("tracked.txt"), "dirty\n").expect("dirty workspace");
    let mut store = ProjectStore::open(temp.path()).expect("store should open");

    store
        .append_event(&Event {
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some("working".into()),
            ..Event::default()
        })
        .expect("append event");

    let events = std::fs::read_to_string(temp.path().join(".agent-monitor").join("events.jsonl"))
        .expect("events log");
    let record: Event =
        serde_json::from_str(events.lines().next().expect("one event")).expect("event json");
    let workspace = temp.path().display().to_string();

    assert!(
        record
            .observed_at
            .as_deref()
            .is_some_and(|time| !time.is_empty())
    );
    assert_eq!(record.occurred_at, record.observed_at);
    assert_eq!(record.workspace.as_deref(), Some(workspace.as_str()));
    assert_eq!(record.cwd.as_deref(), Some(workspace.as_str()));
    assert_eq!(record.worktree.as_deref(), Some(workspace.as_str()));
    assert_eq!(record.git_head.as_deref(), Some(head.as_str()));
    assert!(
        record
            .git_branch
            .as_deref()
            .is_some_and(|branch| !branch.is_empty()),
        "missing git branch: {:?}",
        record.git_branch
    );
    assert_eq!(record.git_dirty, Some(true));
    assert_eq!(record.source_type.as_deref(), Some("monitor"));
    assert_eq!(
        record.source_path.as_deref(),
        Some("ProjectStore::append_event")
    );
    assert!(
        record
            .source_hash
            .as_deref()
            .is_some_and(|hash| hash.starts_with("fnv1a64:")),
        "missing source hash: {:?}",
        record.source_hash
    );
    assert_eq!(record.redaction_status.as_deref(), Some("clean"));
}

#[test]
fn jsonl_loop_uses_assigned_event_id_for_trace_entries() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store should open");
    let input = br#"{"agent":"codex","kind":"file_change","file":"src/lib.rs","line":7,"rationale":"Add monitored event identity."}
"#;
    let mut output = Vec::new();

    run_jsonl_with_store(&input[..], &mut output, Config::default(), &mut store)
        .expect("jsonl run should persist");

    let events = std::fs::read_to_string(temp.path().join(".agent-monitor").join("events.jsonl"))
        .expect("events log");
    let event: Event =
        serde_json::from_str(events.lines().next().expect("one event")).expect("event json");
    let trace = std::fs::read_to_string(temp.path().join(".agent-monitor").join("trace.jsonl"))
        .expect("trace log");
    let trace: TraceEntry =
        serde_json::from_str(trace.lines().next().expect("one trace")).expect("trace json");

    let event_id = event.event_id.expect("generated event id");
    assert!(event_id.starts_with("event-"));
    assert_eq!(trace.event_id.as_deref(), Some(event_id.as_str()));
}

fn init_git_repo(workspace: &Path) -> String {
    assert!(
        Command::new("git")
            .current_dir(workspace)
            .args(["init"])
            .status()
            .expect("git init")
            .success(),
        "git init failed"
    );
    std::fs::write(workspace.join("tracked.txt"), "clean\n").expect("seed file");
    assert!(
        Command::new("git")
            .current_dir(workspace)
            .args(["add", "tracked.txt"])
            .status()
            .expect("git add")
            .success(),
        "git add failed"
    );
    assert!(
        Command::new("git")
            .current_dir(workspace)
            .args([
                "-c",
                "user.email=monitor@example.invalid",
                "-c",
                "user.name=Monitor Test",
                "commit",
                "-m",
                "initial",
            ])
            .status()
            .expect("git commit")
            .success(),
        "git commit failed"
    );
    let output = Command::new("git")
        .current_dir(workspace)
        .args(["rev-parse", "HEAD"])
        .output()
        .expect("git rev-parse");
    assert!(output.status.success(), "git rev-parse failed");
    String::from_utf8(output.stdout)
        .expect("utf8 head")
        .trim()
        .to_string()
}

#[test]
fn jsonl_file_change_triggers_control_advice_packet() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store should open");
    let input = br#"{"agent":"codex","kind":"file_change","file":"src/lib.rs","rationale":"Implement streaming control loop."}
"#;
    let mut output = Vec::new();

    run_jsonl_with_store(&input[..], &mut output, Config::default(), &mut store)
        .expect("jsonl run should persist and advise");

    let root = temp.path().join(".agent-monitor");
    let advice_log = std::fs::read_to_string(root.join("advice.jsonl")).expect("advice log");
    let advice: AdviceRun =
        serde_json::from_str(advice_log.lines().next().expect("one advice")).expect("advice json");

    assert!(matches!(
        advice.final_action,
        ControlAction::ForceVerification { blocking: true, .. }
    ));
    assert_eq!(advice.packet.target_agent, "codex");
    assert!(root.join("case-files.jsonl").exists());
    assert!(root.join("dispatch.jsonl").exists());
    assert!(root.join("outbox").join("codex").join("latest.md").exists());
}

#[test]
fn jsonl_unverified_completion_claim_triggers_control_advice_packet() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store should open");
    let input = br#"{"agent":"codex","kind":"model_message","content":"Implementation complete. I did not run tests."}
"#;
    let mut output = Vec::new();

    run_jsonl_with_store(&input[..], &mut output, Config::default(), &mut store)
        .expect("jsonl run should persist and advise");

    let root = temp.path().join(".agent-monitor");
    let advice_log = std::fs::read_to_string(root.join("advice.jsonl")).expect("advice log");
    let advice: AdviceRun =
        serde_json::from_str(advice_log.lines().next().expect("one advice")).expect("advice json");

    assert!(matches!(
        advice.final_action,
        ControlAction::ForceVerification { blocking: true, .. }
    ));
    assert_eq!(advice.packet.target_agent, "codex");
    assert!(root.join("outbox").join("codex").join("latest.md").exists());
}

#[test]
fn jsonl_premature_stop_triggers_control_advice_packet() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store should open");
    let input = br#"{"agent":"codex","kind":"model_message","content":"This is a good point to stop. Should I continue?"}
"#;
    let mut output = Vec::new();

    run_jsonl_with_store(&input[..], &mut output, Config::default(), &mut store)
        .expect("jsonl run should persist and advise");

    let root = temp.path().join(".agent-monitor");
    let advice_log = std::fs::read_to_string(root.join("advice.jsonl")).expect("advice log");
    let advice: AdviceRun =
        serde_json::from_str(advice_log.lines().next().expect("one advice")).expect("advice json");

    assert_eq!(advice.packet.target_agent, "codex");
    assert!(root.join("outbox").join("codex").join("latest.md").exists());
}

#[test]
fn store_backed_premature_stop_uses_validated_packet_without_legacy_output() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store should open");
    let input = br#"{"event_id":"evt-stop","agent":"codex","kind":"model_message","content":"This is a good point to stop. Should I continue?"}
"#;
    let mut output = Vec::new();

    run_jsonl_with_store(&input[..], &mut output, Config::default(), &mut store)
        .expect("jsonl run should persist and advise");

    assert!(
        output.is_empty(),
        "store-backed control should use validated packets, not legacy stdout interventions: {}",
        String::from_utf8_lossy(&output)
    );

    let root = temp.path().join(".agent-monitor");
    let advice_log = std::fs::read_to_string(root.join("advice.jsonl")).expect("advice log");
    let advice: AdviceRun =
        serde_json::from_str(advice_log.lines().next().expect("one advice")).expect("advice json");
    assert_eq!(advice.packet.target_agent, "codex");
    assert!(root.join("outbox").join("codex").join("latest.md").exists());
    assert!(!root.join("interventions.jsonl").exists());
}

#[test]
fn store_redacts_secret_like_event_text_before_persisting() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store should open");

    store
        .append_event(&Event {
            agent: "codex".into(),
            kind: EventKind::CommandOutput,
            content: Some("OPENAI_API_KEY=sk-test-secret should not be stored".into()),
            command: Some("curl -H 'Authorization: Bearer raw-token-value'".into()),
            ..Event::default()
        })
        .expect("append event");

    let events = std::fs::read_to_string(temp.path().join(".agent-monitor").join("events.jsonl"))
        .expect("events log");
    assert!(!events.contains("sk-test-secret"));
    assert!(!events.contains("raw-token-value"));
    assert!(events.contains("[REDACTED]"));
    assert!(events.contains("\"redaction_status\":\"redacted\""));
    assert!(events.contains("storage_secret"));
}

#[test]
fn store_redaction_preserves_upstream_tainted_status() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store should open");

    store
        .append_event(&Event {
            agent: "codex".into(),
            kind: EventKind::CommandOutput,
            content: Some("token=raw-tainted-value".into()),
            redaction_status: Some("tainted".into()),
            redaction_rules: vec!["upstream_secret".into()],
            ..Event::default()
        })
        .expect("append event");

    let events = std::fs::read_to_string(temp.path().join(".agent-monitor").join("events.jsonl"))
        .expect("events log");
    assert!(!events.contains("raw-tainted-value"));
    assert!(events.contains("\"redaction_status\":\"tainted\""));
    assert!(events.contains("upstream_secret"));
    assert!(events.contains("storage_secret"));
}

#[test]
fn jsonl_loop_redacts_secret_like_trace_rationale_before_persisting() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store should open");
    let input = br#"{"event_id":"evt-secret-trace","agent":"codex","kind":"file_change","file":"src/lib.rs","line":7,"rationale":"Use token=ghp_secrettracevalue while debugging"}
"#;
    let mut output = Vec::new();

    run_jsonl_with_store(&input[..], &mut output, Config::default(), &mut store)
        .expect("jsonl run should persist");

    let trace = std::fs::read_to_string(temp.path().join(".agent-monitor").join("trace.jsonl"))
        .expect("trace log");
    let events = std::fs::read_to_string(temp.path().join(".agent-monitor").join("events.jsonl"))
        .expect("events log");
    assert!(!trace.contains("ghp_secrettracevalue"));
    assert!(!events.contains("ghp_secrettracevalue"));
    assert!(trace.contains("[REDACTED]"));
    assert!(events.contains("\"redaction_status\":\"redacted\""));
}
