use coding_agent_monitor::{
    BlameMatchKind, BlameQuery, BlameStatus, ProjectStore, TraceEntry, load_blame_report,
    run_jsonl_with_store,
};

#[test]
fn blame_report_ranks_exact_line_before_file_level_trace() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_trace(&TraceEntry {
            time: Some("2026-06-22T12:00:00Z".into()),
            agent: "claude-code".into(),
            session: Some("s0".into()),
            file: "src/lib.rs".into(),
            line: None,
            rationale: Some("Create monitor storage module.".into()),
            ..TraceEntry::default()
        })
        .expect("file-level trace");
    store
        .append_trace(&TraceEntry {
            time: Some("2026-06-22T12:02:00Z".into()),
            agent: "codex".into(),
            session: Some("s1".into()),
            file: "src/lib.rs".into(),
            line: Some(42),
            rationale: Some("Add stale verification guard.".into()),
            event_id: Some("evt-line".into()),
            ..TraceEntry::default()
        })
        .expect("line trace");

    let report = load_blame_report(
        temp.path(),
        BlameQuery {
            file: "src\\lib.rs".into(),
            line: Some(42),
            limit: 10,
        },
    )
    .expect("blame report");

    assert_eq!(report.status, BlameStatus::Traced);
    assert_eq!(report.trace_count, 2);
    assert_eq!(report.matches.len(), 2);
    assert_eq!(report.matches[0].match_kind, BlameMatchKind::ExactLine);
    assert_eq!(report.matches[0].trace.agent, "codex");
    assert_eq!(
        report.matches[0].trace.event_id.as_deref(),
        Some("evt-line")
    );
    assert_eq!(report.matches[1].match_kind, BlameMatchKind::File);
    assert_eq!(report.matches[1].trace.agent, "claude-code");
}

#[test]
fn blame_report_matches_trace_line_range() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_trace(&TraceEntry {
            time: Some("2026-06-22T12:00:00Z".into()),
            agent: "codex".into(),
            session: Some("s1".into()),
            file: "src/lib.rs".into(),
            line: Some(40),
            line_end: Some(45),
            rationale: Some("Rewrite parser branch across a small range.".into()),
            event_id: Some("evt-range".into()),
            ..TraceEntry::default()
        })
        .expect("range trace");

    let report = load_blame_report(
        temp.path(),
        BlameQuery {
            file: "src/lib.rs".into(),
            line: Some(42),
            limit: 10,
        },
    )
    .expect("blame report");

    assert_eq!(report.status, BlameStatus::Traced);
    assert_eq!(report.matches.len(), 1);
    assert_eq!(report.matches[0].match_kind, BlameMatchKind::ExactLine);
    assert_eq!(
        report.matches[0].trace.event_id.as_deref(),
        Some("evt-range")
    );

    let outside = load_blame_report(
        temp.path(),
        BlameQuery {
            file: "src/lib.rs".into(),
            line: Some(46),
            limit: 10,
        },
    )
    .expect("outside blame report");
    assert_eq!(outside.status, BlameStatus::Untraced);
    assert!(outside.matches.is_empty());
}

#[test]
fn blame_report_limits_newest_matches_and_reports_untraced_files() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    for index in 0..3 {
        store
            .append_trace(&TraceEntry {
                time: Some(format!("2026-06-22T12:0{index}:00Z")),
                agent: format!("agent-{index}"),
                session: Some(format!("s{index}")),
                file: "src/parser.rs".into(),
                line: None,
                rationale: Some(format!("Change parser slice {index}.")),
                ..TraceEntry::default()
            })
            .expect("trace");
    }

    let report = load_blame_report(
        temp.path(),
        BlameQuery {
            file: "./src/parser.rs".into(),
            line: None,
            limit: 2,
        },
    )
    .expect("blame report");
    assert_eq!(report.status, BlameStatus::Traced);
    assert_eq!(
        report
            .matches
            .iter()
            .map(|entry| entry.trace.agent.as_str())
            .collect::<Vec<_>>(),
        vec!["agent-2", "agent-1"]
    );

    let missing = load_blame_report(
        temp.path(),
        BlameQuery {
            file: "src/missing.rs".into(),
            line: Some(1),
            limit: 10,
        },
    )
    .expect("missing blame report");
    assert_eq!(missing.status, BlameStatus::Untraced);
    assert!(missing.matches.is_empty());
}

#[test]
fn blame_report_does_not_create_monitor_storage_for_missing_trace_log() {
    let temp = tempfile::tempdir().expect("temp dir");

    let report = load_blame_report(
        temp.path(),
        BlameQuery {
            file: "src/lib.rs".into(),
            line: Some(1),
            limit: 10,
        },
    )
    .expect("blame report");

    assert_eq!(report.status, BlameStatus::Untraced);
    assert!(!temp.path().join(".agent-monitor").exists());
}

#[test]
fn blame_report_status_remains_traced_when_limit_is_zero() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_trace(&TraceEntry {
            time: Some("2026-06-22T12:00:00Z".into()),
            agent: "codex".into(),
            session: Some("s1".into()),
            file: "src/lib.rs".into(),
            line: Some(42),
            rationale: Some("Add blame query.".into()),
            ..TraceEntry::default()
        })
        .expect("trace");

    let report = load_blame_report(
        temp.path(),
        BlameQuery {
            file: "src/lib.rs".into(),
            line: Some(42),
            limit: 0,
        },
    )
    .expect("blame report");

    assert_eq!(report.status, BlameStatus::Traced);
    assert!(report.matches.is_empty());
}

#[test]
fn blame_report_matches_absolute_workspace_paths_and_dot_segments() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_trace(&TraceEntry {
            time: Some("2026-06-22T12:00:00Z".into()),
            agent: "codex".into(),
            session: Some("s1".into()),
            file: "src/lib.rs".into(),
            line: Some(42),
            rationale: Some("Normalize blame paths.".into()),
            ..TraceEntry::default()
        })
        .expect("trace");
    let queried_path = temp
        .path()
        .join(".")
        .join("src")
        .join("..")
        .join("src")
        .join("lib.rs");

    let report = load_blame_report(
        temp.path(),
        BlameQuery {
            file: queried_path.display().to_string(),
            line: Some(42),
            limit: 10,
        },
    )
    .expect("blame report");

    assert_eq!(report.status, BlameStatus::Traced);
    assert_eq!(report.matches.len(), 1);
    assert_eq!(report.matches[0].trace.file, "src/lib.rs");
}

#[test]
fn blame_report_matches_workspace_absolute_paths_with_different_case() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_trace(&TraceEntry {
            time: Some("2026-06-22T12:00:00Z".into()),
            agent: "codex".into(),
            session: Some("s1".into()),
            file: "src/lib.rs".into(),
            line: Some(42),
            rationale: Some("Normalize absolute path case.".into()),
            ..TraceEntry::default()
        })
        .expect("trace");
    let queried_path = temp.path().join("src").join("lib.rs");
    let queried_path = queried_path.display().to_string().to_ascii_uppercase();

    let report = load_blame_report(
        temp.path(),
        BlameQuery {
            file: queried_path,
            line: Some(42),
            limit: 10,
        },
    )
    .expect("blame report");

    assert_eq!(report.status, BlameStatus::Traced);
    assert_eq!(report.matches[0].trace.agent, "codex");
}

#[test]
fn blame_report_does_not_match_root_absolute_path_outside_workspace() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_trace(&TraceEntry {
            time: Some("2026-06-22T12:00:00Z".into()),
            agent: "codex".into(),
            session: Some("s1".into()),
            file: "src/lib.rs".into(),
            line: Some(42),
            rationale: Some("Keep root absolute paths distinct.".into()),
            ..TraceEntry::default()
        })
        .expect("trace");

    let report = load_blame_report(
        temp.path(),
        BlameQuery {
            file: "\\src\\lib.rs".into(),
            line: Some(42),
            limit: 10,
        },
    )
    .expect("blame report");

    assert_eq!(report.status, BlameStatus::Untraced);
    assert!(report.matches.is_empty());
}

#[test]
fn blame_report_matches_absolute_file_when_workspace_is_relative() {
    let current_dir = std::env::current_dir().expect("current dir");
    let temp = tempfile::tempdir_in(&current_dir).expect("temp dir in current dir");
    let relative_workspace = temp
        .path()
        .strip_prefix(&current_dir)
        .expect("relative temp path")
        .to_path_buf();
    let mut store = ProjectStore::open(&relative_workspace).expect("store");
    store
        .append_trace(&TraceEntry {
            time: Some("2026-06-22T12:00:00Z".into()),
            agent: "codex".into(),
            session: Some("s1".into()),
            file: "src/lib.rs".into(),
            line: Some(42),
            rationale: Some("Resolve relative workspace for blame.".into()),
            ..TraceEntry::default()
        })
        .expect("trace");
    let absolute_file = current_dir
        .join(&relative_workspace)
        .join("src")
        .join("lib.rs");

    let report = load_blame_report(
        &relative_workspace,
        BlameQuery {
            file: absolute_file.display().to_string(),
            line: Some(42),
            limit: 10,
        },
    )
    .expect("blame report");

    assert_eq!(report.status, BlameStatus::Traced);
    assert_eq!(report.matches[0].trace.agent, "codex");
}

#[test]
fn persisted_trace_entries_keep_event_metadata_for_blame() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    let input = br#"{"time":"2026-06-22T12:04:00Z","event_id":"evt-change","agent":"codex","provider":"openai","model":"gpt-5.5","session":"s1","kind":"file_change","file":"src/lib.rs","line":42,"line_end":45,"rationale":"Add trace metadata.","related_event_ids":["evt-user","evt-test"]}
"#;
    let mut output = Vec::new();

    run_jsonl_with_store(
        &input[..],
        &mut output,
        coding_agent_monitor::Config::default(),
        &mut store,
    )
    .expect("jsonl run");

    let report = load_blame_report(
        temp.path(),
        BlameQuery {
            file: "src/lib.rs".into(),
            line: Some(42),
            limit: 10,
        },
    )
    .expect("blame report");

    assert_eq!(report.matches.len(), 1);
    let trace = &report.matches[0].trace;
    assert_eq!(trace.event_id.as_deref(), Some("evt-change"));
    assert_eq!(trace.provider.as_deref(), Some("openai"));
    assert_eq!(trace.model.as_deref(), Some("gpt-5.5"));
    assert_eq!(trace.line_end, Some(45));
    assert_eq!(trace.related_event_ids, vec!["evt-user", "evt-test"]);
}
