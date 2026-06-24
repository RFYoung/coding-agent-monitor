use coding_agent_monitor::{
    ProjectStore, RepoAuditStatus, RepoChangeKind, RepoHunkHistoryEntry, RepoHunkHistoryQuery,
    RepoTraceStatus, TraceEntry, load_repo_audit, load_repo_hunk_history,
    record_repo_audit_history,
};
use std::fs;
use std::path::Path;
use std::process::Command;

#[test]
fn repo_audit_flags_modified_hunk_without_trace() {
    let temp = tempfile::tempdir().expect("temp dir");
    init_git_repo(temp.path());
    write_file(
        temp.path().join("src/lib.rs"),
        "fn one() {}\nfn two() {}\nfn three() {}\n",
    );
    git(temp.path(), ["add", "src/lib.rs"]);
    git_commit(temp.path(), "initial");
    write_file(
        temp.path().join("src/lib.rs"),
        "fn one() {}\nfn two_changed() {}\nfn three() {}\n",
    );

    let report = load_repo_audit(temp.path()).expect("repo audit");

    assert_eq!(report.status, RepoAuditStatus::Warning);
    assert_eq!(report.untraced_count, 1);
    assert_eq!(report.changes.len(), 1);
    let change = &report.changes[0];
    assert_eq!(change.path, "src/lib.rs");
    assert_eq!(change.kind, RepoChangeKind::Modified);
    assert_eq!(change.trace_status, RepoTraceStatus::Untraced);
    assert_eq!(change.hunks[0].new_start, 2);
    assert_eq!(change.hunks[0].new_lines, 1);
    assert_eq!(change.hunks[0].trace_status, RepoTraceStatus::Untraced);
    assert_eq!(change.hunks[0].matching_trace_count, 0);
}

#[test]
fn repo_audit_marks_modified_hunk_traced_when_line_trace_has_rationale() {
    let temp = tempfile::tempdir().expect("temp dir");
    init_git_repo(temp.path());
    write_file(temp.path().join("src/lib.rs"), "fn one() {}\nfn two() {}\n");
    git(temp.path(), ["add", "src/lib.rs"]);
    git_commit(temp.path(), "initial");
    write_file(
        temp.path().join("src/lib.rs"),
        "fn one() {}\nfn two_changed() {}\n",
    );
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_trace(&TraceEntry {
            time: Some("9999-01-01T00:00:00Z".into()),
            agent: "codex".into(),
            session: Some("s1".into()),
            file: "src/lib.rs".into(),
            line: Some(2),
            rationale: Some("Rename helper for clarity.".into()),
            ..TraceEntry::default()
        })
        .expect("trace");

    let report = load_repo_audit(temp.path()).expect("repo audit");

    assert_eq!(report.status, RepoAuditStatus::Clean);
    assert_eq!(report.untraced_count, 0);
    assert_eq!(report.changes[0].trace_status, RepoTraceStatus::Traced);
    assert_eq!(
        report.changes[0].hunks[0].trace_status,
        RepoTraceStatus::Traced
    );
    assert_eq!(report.changes[0].hunks[0].matching_trace_count, 1);
    assert_eq!(report.changes[0].matching_traces[0].agent, "codex");
}

#[test]
fn repo_audit_marks_modified_hunk_traced_when_trace_range_overlaps_hunk() {
    let temp = tempfile::tempdir().expect("temp dir");
    init_git_repo(temp.path());
    write_file(
        temp.path().join("src/lib.rs"),
        "fn one() {}\nfn two() {}\nfn three() {}\nfn four() {}\n",
    );
    git(temp.path(), ["add", "src/lib.rs"]);
    git_commit(temp.path(), "initial");
    write_file(
        temp.path().join("src/lib.rs"),
        "fn one() {}\nfn two() {}\nfn three_changed() {}\nfn four() {}\n",
    );
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_trace(&TraceEntry {
            time: Some("9999-01-01T00:00:00Z".into()),
            agent: "codex".into(),
            session: Some("s1".into()),
            file: "src/lib.rs".into(),
            line: Some(2),
            line_end: Some(4),
            rationale: Some("Update helper block across the affected range.".into()),
            ..TraceEntry::default()
        })
        .expect("trace");

    let report = load_repo_audit(temp.path()).expect("repo audit");

    assert_eq!(report.status, RepoAuditStatus::Clean);
    assert_eq!(report.untraced_count, 0);
    assert_eq!(report.changes[0].trace_status, RepoTraceStatus::Traced);
    assert_eq!(report.changes[0].matching_traces[0].line, Some(2));
    assert_eq!(report.changes[0].matching_traces[0].line_end, Some(4));
}

#[test]
fn repo_audit_flags_matching_trace_without_rationale() {
    let temp = tempfile::tempdir().expect("temp dir");
    init_git_repo(temp.path());
    write_file(temp.path().join("src/lib.rs"), "fn one() {}\nfn two() {}\n");
    git(temp.path(), ["add", "src/lib.rs"]);
    git_commit(temp.path(), "initial");
    write_file(
        temp.path().join("src/lib.rs"),
        "fn one() {}\nfn two_changed() {}\n",
    );
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_trace(&TraceEntry {
            time: Some("9999-01-01T00:00:00Z".into()),
            agent: "codex".into(),
            session: Some("s1".into()),
            file: "src/lib.rs".into(),
            line: Some(2),
            rationale: None,
            ..TraceEntry::default()
        })
        .expect("trace");

    let report = load_repo_audit(temp.path()).expect("repo audit");

    assert_eq!(report.status, RepoAuditStatus::Warning);
    assert_eq!(report.unexplained_count, 1);
    assert_eq!(
        report.changes[0].trace_status,
        RepoTraceStatus::MissingRationale
    );
}

#[test]
fn repo_audit_does_not_hide_untraced_hunk_when_another_hunk_is_traced() {
    let temp = tempfile::tempdir().expect("temp dir");
    init_git_repo(temp.path());
    write_file(
        temp.path().join("src/lib.rs"),
        "fn one() {}\nfn two() {}\nfn three() {}\nfn four() {}\nfn five() {}\n",
    );
    git(temp.path(), ["add", "src/lib.rs"]);
    git_commit(temp.path(), "initial");
    write_file(
        temp.path().join("src/lib.rs"),
        "fn one() {}\nfn two_changed() {}\nfn three() {}\nfn four() {}\nfn five_changed() {}\n",
    );
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_trace(&TraceEntry {
            time: Some("9999-01-01T00:00:00Z".into()),
            agent: "codex".into(),
            session: Some("s1".into()),
            file: "src/lib.rs".into(),
            line: Some(2),
            rationale: Some("Explain the first changed hunk only.".into()),
            ..TraceEntry::default()
        })
        .expect("trace");

    let report = load_repo_audit(temp.path()).expect("repo audit");

    assert_eq!(report.status, RepoAuditStatus::Warning);
    assert_eq!(report.untraced_count, 1);
    assert_eq!(report.changes[0].hunks.len(), 2);
    assert_eq!(report.changes[0].trace_status, RepoTraceStatus::Untraced);
    assert_eq!(
        report.changes[0].hunks[0].trace_status,
        RepoTraceStatus::Traced
    );
    assert_eq!(report.changes[0].hunks[0].matching_trace_count, 1);
    assert_eq!(
        report.changes[0].hunks[1].trace_status,
        RepoTraceStatus::Untraced
    );
    assert_eq!(report.changes[0].hunks[1].matching_trace_count, 0);
}

#[test]
fn repo_audit_treats_file_level_trace_as_partial_for_multi_hunk_file() {
    let temp = tempfile::tempdir().expect("temp dir");
    init_git_repo(temp.path());
    write_file(
        temp.path().join("src/lib.rs"),
        "fn one() {}\nfn two() {}\nfn three() {}\nfn four() {}\nfn five() {}\n",
    );
    git(temp.path(), ["add", "src/lib.rs"]);
    git_commit(temp.path(), "initial");
    write_file(
        temp.path().join("src/lib.rs"),
        "fn one() {}\nfn two_changed() {}\nfn three() {}\nfn four() {}\nfn five_changed() {}\n",
    );
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_trace(&TraceEntry {
            time: Some("9999-01-01T00:00:00Z".into()),
            agent: "codex".into(),
            session: Some("s1".into()),
            file: "src/lib.rs".into(),
            line: None,
            rationale: Some("Broad file-level rationale for the edit.".into()),
            ..TraceEntry::default()
        })
        .expect("trace");

    let report = load_repo_audit(temp.path()).expect("repo audit");

    assert_eq!(report.status, RepoAuditStatus::Warning);
    assert_eq!(report.untraced_count, 0);
    assert_eq!(report.unexplained_count, 1);
    assert_eq!(report.changes[0].hunks.len(), 2);
    assert_eq!(
        report.changes[0].trace_status,
        RepoTraceStatus::MissingRationale
    );
    assert_eq!(
        report.changes[0].hunks[0].trace_status,
        RepoTraceStatus::MissingRationale
    );
    assert_eq!(report.changes[0].hunks[0].matching_trace_count, 1);
    assert_eq!(
        report.changes[0].hunks[1].trace_status,
        RepoTraceStatus::MissingRationale
    );
    assert_eq!(report.changes[0].hunks[1].matching_trace_count, 1);
}

#[test]
fn repo_audit_records_per_hunk_history_entries() {
    let temp = tempfile::tempdir().expect("temp dir");
    init_git_repo(temp.path());
    write_file(
        temp.path().join("src/lib.rs"),
        "fn one() {}\nfn two() {}\nfn three() {}\nfn four() {}\nfn five() {}\n",
    );
    git(temp.path(), ["add", "src/lib.rs"]);
    git_commit(temp.path(), "initial");
    write_file(
        temp.path().join("src/lib.rs"),
        "fn one() {}\nfn two_changed() {}\nfn three() {}\nfn four() {}\nfn five_changed() {}\n",
    );
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_trace(&TraceEntry {
            time: Some("9999-01-01T00:00:00Z".into()),
            agent: "codex".into(),
            session: Some("s1".into()),
            file: "src/lib.rs".into(),
            line: Some(2),
            rationale: Some("Explain the first changed hunk only.".into()),
            ..TraceEntry::default()
        })
        .expect("trace");

    let report = record_repo_audit_history(temp.path()).expect("repo audit");

    assert_eq!(report.changes[0].hunks.len(), 2);
    let history_path = temp.path().join(".agent-monitor").join("repo-hunks.jsonl");
    let history = fs::read_to_string(history_path).expect("hunk history");
    let entries = history
        .lines()
        .map(|line| serde_json::from_str::<RepoHunkHistoryEntry>(line).expect("history entry"))
        .collect::<Vec<_>>();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].path, "src/lib.rs");
    assert_eq!(entries[0].hunk_index, 0);
    assert_eq!(entries[0].new_start, 2);
    assert_eq!(entries[0].trace_status, RepoTraceStatus::Traced);
    assert_eq!(entries[0].matching_trace_count, 1);
    assert_eq!(entries[0].matching_trace_refs.len(), 1);
    assert_eq!(
        entries[0].matching_trace_refs[0].agent.as_deref(),
        Some("codex")
    );
    assert_eq!(
        entries[0].matching_trace_refs[0].rationale.as_deref(),
        Some("Explain the first changed hunk only.")
    );
    assert_eq!(entries[1].path, "src/lib.rs");
    assert_eq!(entries[1].hunk_index, 1);
    assert_eq!(entries[1].new_start, 5);
    assert_eq!(entries[1].trace_status, RepoTraceStatus::Untraced);
    assert_eq!(entries[1].matching_trace_count, 0);
    assert_eq!(entries[1].change_trace_status, RepoTraceStatus::Untraced);
}

#[test]
fn repo_hunk_history_query_filters_by_file_line_and_returns_newest_first() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_repo_hunk_history(&history_entry(
            "hist-old",
            "2026-06-22T12:00:00Z",
            temp.path(),
            "src/lib.rs",
            0,
            2,
            1,
            RepoTraceStatus::Traced,
            1,
        ))
        .expect("old history");
    store
        .append_repo_hunk_history(&history_entry(
            "hist-other",
            "2026-06-22T12:01:00Z",
            temp.path(),
            "tests/lib.rs",
            0,
            5,
            1,
            RepoTraceStatus::MissingRationale,
            1,
        ))
        .expect("other history");
    store
        .append_repo_hunk_history(&history_entry(
            "hist-new",
            "2026-06-22T12:02:00Z",
            temp.path(),
            "src/lib.rs",
            1,
            5,
            2,
            RepoTraceStatus::Untraced,
            0,
        ))
        .expect("new history");

    let line_report = load_repo_hunk_history(
        temp.path(),
        RepoHunkHistoryQuery {
            file: Some(
                temp.path()
                    .join(".")
                    .join("src")
                    .join("lib.rs")
                    .display()
                    .to_string(),
            ),
            line: Some(5),
            limit: 10,
        },
    )
    .expect("hunk history");

    assert_eq!(line_report.entry_count, 1);
    assert_eq!(line_report.entries.len(), 1);
    assert_eq!(line_report.entries[0].history_id, "hist-new");
    assert_eq!(
        line_report.entries[0].trace_status,
        RepoTraceStatus::Untraced
    );

    let file_report = load_repo_hunk_history(
        temp.path(),
        RepoHunkHistoryQuery {
            file: Some("src/lib.rs".into()),
            line: None,
            limit: 1,
        },
    )
    .expect("bounded hunk history");

    assert_eq!(file_report.entry_count, 2);
    assert_eq!(file_report.entries.len(), 1);
    assert_eq!(file_report.entries[0].history_id, "hist-new");
}

#[test]
fn repo_hunk_history_report_groups_matching_hunks_by_file() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_repo_hunk_history(&history_entry(
            "hist-old",
            "2026-06-22T12:00:00Z",
            temp.path(),
            "src/lib.rs",
            0,
            2,
            1,
            RepoTraceStatus::Traced,
            1,
        ))
        .expect("old history");
    store
        .append_repo_hunk_history(&history_entry(
            "hist-missing",
            "2026-06-22T12:01:00Z",
            temp.path(),
            "src/lib.rs",
            1,
            5,
            1,
            RepoTraceStatus::MissingRationale,
            1,
        ))
        .expect("missing history");
    store
        .append_repo_hunk_history(&history_entry(
            "hist-other",
            "2026-06-22T12:02:00Z",
            temp.path(),
            "tests/lib.rs",
            0,
            3,
            1,
            RepoTraceStatus::Untraced,
            0,
        ))
        .expect("other history");
    store
        .append_repo_hunk_history(&history_entry(
            "hist-new",
            "2026-06-22T12:03:00Z",
            temp.path(),
            "src/lib.rs",
            2,
            8,
            1,
            RepoTraceStatus::Untraced,
            0,
        ))
        .expect("new history");

    let report = load_repo_hunk_history(
        temp.path(),
        RepoHunkHistoryQuery {
            file: Some("src/lib.rs".into()),
            line: None,
            limit: 2,
        },
    )
    .expect("hunk history");

    assert_eq!(report.entry_count, 3);
    assert_eq!(report.entries.len(), 2);
    assert_eq!(report.file_count, 1);
    assert_eq!(report.files.len(), 1);
    let summary = &report.files[0];
    assert_eq!(summary.path, "src/lib.rs");
    assert_eq!(summary.entry_count, 3);
    assert_eq!(summary.traced_count, 1);
    assert_eq!(summary.missing_rationale_count, 1);
    assert_eq!(summary.untraced_count, 1);
    assert_eq!(summary.matching_trace_count, 2);
    assert_eq!(summary.worst_trace_status, RepoTraceStatus::Untraced);
    assert_eq!(summary.latest_trace_status, RepoTraceStatus::Untraced);
    assert_eq!(summary.latest_history_id, "hist-new");
    assert_eq!(summary.latest_observed_at, "2026-06-22T12:03:00Z");
}

#[test]
fn repo_audit_matches_deleted_line_trace_against_old_hunk_range() {
    let temp = tempfile::tempdir().expect("temp dir");
    init_git_repo(temp.path());
    write_file(
        temp.path().join("src/lib.rs"),
        "fn one() {}\nfn two() {}\nfn three() {}\n",
    );
    git(temp.path(), ["add", "src/lib.rs"]);
    git_commit(temp.path(), "initial");
    write_file(
        temp.path().join("src/lib.rs"),
        "fn one() {}\nfn three() {}\n",
    );
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_trace(&TraceEntry {
            time: Some("9999-01-01T00:00:00Z".into()),
            agent: "codex".into(),
            session: Some("s1".into()),
            file: "src/lib.rs".into(),
            line: Some(2),
            rationale: Some("Remove obsolete helper.".into()),
            ..TraceEntry::default()
        })
        .expect("trace");

    let report = load_repo_audit(temp.path()).expect("repo audit");

    assert_eq!(report.status, RepoAuditStatus::Clean);
    assert_eq!(report.untraced_count, 0);
    assert_eq!(report.changes[0].trace_status, RepoTraceStatus::Traced);
}

#[test]
fn repo_audit_ignores_stale_trace_recorded_before_dirty_edit() {
    let temp = tempfile::tempdir().expect("temp dir");
    init_git_repo(temp.path());
    write_file(temp.path().join("src/lib.rs"), "fn one() {}\nfn two() {}\n");
    git(temp.path(), ["add", "src/lib.rs"]);
    git_commit(temp.path(), "initial");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_trace(&TraceEntry {
            time: Some("1970-01-01T00:00:00Z".into()),
            agent: "codex".into(),
            session: Some("s1".into()),
            file: "src/lib.rs".into(),
            line: Some(2),
            rationale: Some("Old rationale for earlier work.".into()),
            ..TraceEntry::default()
        })
        .expect("trace");
    write_file(
        temp.path().join("src/lib.rs"),
        "fn one() {}\nfn two_changed() {}\n",
    );

    let report = load_repo_audit(temp.path()).expect("repo audit");

    assert_eq!(report.status, RepoAuditStatus::Warning);
    assert_eq!(report.untraced_count, 1);
    assert_eq!(report.changes[0].trace_status, RepoTraceStatus::Untraced);
    assert!(report.changes[0].matching_traces.is_empty());
}

#[test]
fn repo_audit_treats_untimestamped_trace_as_stale_for_dirty_file() {
    let temp = tempfile::tempdir().expect("temp dir");
    init_git_repo(temp.path());
    write_file(temp.path().join("src/lib.rs"), "fn one() {}\nfn two() {}\n");
    git(temp.path(), ["add", "src/lib.rs"]);
    git_commit(temp.path(), "initial");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_trace(&TraceEntry {
            agent: "codex".into(),
            session: Some("s1".into()),
            file: "src/lib.rs".into(),
            line: Some(2),
            rationale: Some("Legacy trace without a timestamp.".into()),
            ..TraceEntry::default()
        })
        .expect("trace");
    write_file(
        temp.path().join("src/lib.rs"),
        "fn one() {}\nfn two_changed() {}\n",
    );

    let report = load_repo_audit(temp.path()).expect("repo audit");

    assert_eq!(report.status, RepoAuditStatus::Warning);
    assert_eq!(report.untraced_count, 1);
    assert_eq!(report.changes[0].trace_status, RepoTraceStatus::Untraced);
}

#[test]
fn repo_audit_ignores_stale_trace_for_deleted_file() {
    let temp = tempfile::tempdir().expect("temp dir");
    init_git_repo(temp.path());
    write_file(temp.path().join("src/lib.rs"), "fn one() {}\nfn two() {}\n");
    git(temp.path(), ["add", "src/lib.rs"]);
    git_commit(temp.path(), "initial");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_trace(&TraceEntry {
            time: Some("1970-01-01T00:00:00Z".into()),
            agent: "codex".into(),
            session: Some("s1".into()),
            file: "src/lib.rs".into(),
            line: None,
            rationale: Some("Old file-level rationale.".into()),
            ..TraceEntry::default()
        })
        .expect("trace");
    fs::remove_file(temp.path().join("src/lib.rs")).expect("delete source");

    let report = load_repo_audit(temp.path()).expect("repo audit");

    assert_eq!(report.status, RepoAuditStatus::Warning);
    assert_eq!(report.untraced_count, 1);
    assert_eq!(report.changes[0].kind, RepoChangeKind::Deleted);
    assert_eq!(report.changes[0].trace_status, RepoTraceStatus::Untraced);
}

#[test]
fn repo_audit_ignores_post_head_trace_recorded_before_file_deletion() {
    let temp = tempfile::tempdir().expect("temp dir");
    init_git_repo(temp.path());
    write_file(temp.path().join("src/lib.rs"), "fn one() {}\nfn two() {}\n");
    git(temp.path(), ["add", "src/lib.rs"]);
    git_commit_at(temp.path(), "initial", "2000-01-01T00:00:00Z");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_trace(&TraceEntry {
            time: Some("2001-01-01T00:00:00Z".into()),
            agent: "codex".into(),
            session: Some("s1".into()),
            file: "src/lib.rs".into(),
            line: None,
            rationale: Some("Rationale after HEAD but before deletion.".into()),
            ..TraceEntry::default()
        })
        .expect("trace");
    fs::remove_file(temp.path().join("src/lib.rs")).expect("delete source");

    let report = load_repo_audit(temp.path()).expect("repo audit");

    assert_eq!(report.status, RepoAuditStatus::Warning);
    assert_eq!(report.untraced_count, 1);
    assert_eq!(report.changes[0].kind, RepoChangeKind::Deleted);
    assert_eq!(report.changes[0].trace_status, RepoTraceStatus::Untraced);
}

#[test]
fn repo_audit_ignores_post_head_trace_recorded_before_directory_deletion() {
    let temp = tempfile::tempdir().expect("temp dir");
    init_git_repo(temp.path());
    write_file(temp.path().join("src/lib.rs"), "fn one() {}\nfn two() {}\n");
    git(temp.path(), ["add", "src/lib.rs"]);
    git_commit_at(temp.path(), "initial", "2000-01-01T00:00:00Z");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_trace(&TraceEntry {
            time: Some("2001-01-01T00:00:00Z".into()),
            agent: "codex".into(),
            session: Some("s1".into()),
            file: "src/lib.rs".into(),
            line: None,
            rationale: Some("Rationale after HEAD but before directory deletion.".into()),
            ..TraceEntry::default()
        })
        .expect("trace");
    fs::remove_dir_all(temp.path().join("src")).expect("delete source dir");

    let report = load_repo_audit(temp.path()).expect("repo audit");

    assert_eq!(report.status, RepoAuditStatus::Warning);
    assert_eq!(report.untraced_count, 1);
    assert_eq!(report.changes[0].kind, RepoChangeKind::Deleted);
    assert_eq!(report.changes[0].trace_status, RepoTraceStatus::Untraced);
}

#[test]
fn repo_audit_bounds_matching_trace_entries() {
    let temp = tempfile::tempdir().expect("temp dir");
    init_git_repo(temp.path());
    write_file(temp.path().join("src/lib.rs"), "fn one() {}\nfn two() {}\n");
    git(temp.path(), ["add", "src/lib.rs"]);
    git_commit(temp.path(), "initial");
    write_file(
        temp.path().join("src/lib.rs"),
        "fn one() {}\nfn two_changed() {}\n",
    );
    let mut store = ProjectStore::open(temp.path()).expect("store");
    for index in 0..12 {
        store
            .append_trace(&TraceEntry {
                time: Some("9999-01-01T00:00:00Z".into()),
                agent: format!("codex-{index}"),
                session: Some("s1".into()),
                file: "src/lib.rs".into(),
                line: Some(2),
                rationale: Some(format!("Trace rationale {index}.")),
                ..TraceEntry::default()
            })
            .expect("trace");
    }

    let report = load_repo_audit(temp.path()).expect("repo audit");

    assert_eq!(report.status, RepoAuditStatus::Clean);
    assert!(report.changes[0].matching_traces.len() <= 5);
    assert_eq!(report.changes[0].matching_traces[0].agent, "codex-11");
}

fn init_git_repo(workspace: &Path) {
    git(workspace, ["init", "--quiet"]);
    git(workspace, ["config", "user.name", "Test User"]);
    git(workspace, ["config", "user.email", "test@example.com"]);
}

fn git<const N: usize>(workspace: &Path, args: [&str; N]) {
    let status = Command::new("git")
        .current_dir(workspace)
        .args(args)
        .status()
        .expect("git command");
    assert!(status.success(), "git command failed: {args:?}");
}

fn git_commit(workspace: &Path, message: &str) {
    git_commit_with_env(workspace, message, None);
}

fn git_commit_at(workspace: &Path, message: &str, timestamp: &str) {
    git_commit_with_env(workspace, message, Some(timestamp));
}

fn git_commit_with_env(workspace: &Path, message: &str, timestamp: Option<&str>) {
    let mut command = Command::new("git");
    command
        .current_dir(workspace)
        .args(["commit", "--quiet", "-m", message]);
    if let Some(timestamp) = timestamp {
        command.env("GIT_AUTHOR_DATE", timestamp);
        command.env("GIT_COMMITTER_DATE", timestamp);
    }
    let status = command.status().expect("git commit");
    assert!(status.success(), "git commit failed");
}

fn write_file(path: impl AsRef<Path>, content: &str) {
    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent");
    }
    fs::write(path, content).expect("write file");
}

fn history_entry(
    history_id: &str,
    observed_at: &str,
    workspace: &Path,
    path: &str,
    hunk_index: usize,
    new_start: u32,
    new_lines: u32,
    trace_status: RepoTraceStatus,
    matching_trace_count: usize,
) -> RepoHunkHistoryEntry {
    RepoHunkHistoryEntry {
        history_id: history_id.into(),
        observed_at: observed_at.into(),
        workspace: workspace.display().to_string(),
        path: path.into(),
        kind: RepoChangeKind::Modified,
        hunk_index,
        old_start: new_start,
        old_lines: new_lines,
        new_start,
        new_lines,
        trace_status,
        matching_trace_count,
        change_trace_status: trace_status,
        modified_at: Some(1),
        matching_trace_refs: Vec::new(),
    }
}
