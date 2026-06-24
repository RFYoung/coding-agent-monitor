use coding_agent_monitor::{
    DevHistoryAnalysisOptions, DevHistoryError, DevHistoryRawExportOptions,
    analyze_local_dev_history, export_raw_dev_history,
};
use std::fs;
use std::path::PathBuf;

#[test]
fn local_dev_history_filters_codex_by_workspace_metadata_and_analyzes_claude_project() {
    let temp = tempfile::tempdir().expect("temp dir");
    let codex_root = temp.path().join("codex-sessions");
    let claude_projects = temp.path().join("claude-projects");
    fs::create_dir_all(&codex_root).expect("codex root");
    fs::create_dir_all(claude_projects.join("F--rag-sys")).expect("claude project");

    fs::write(
        codex_root.join("matching.jsonl"),
        [
            r#"{"timestamp":"2026-06-23T10:00:00Z","type":"session_meta","payload":{"id":"s1","cwd":"F:\\rag_sys"}}"#,
            r#"{"timestamp":"2026-06-23T10:01:00Z","type":"response_item","payload":{"type":"function_call","name":"shell_command","arguments":"{\"command\":\"npm test -- --run\"}"}}"#,
            r#"{"timestamp":"2026-06-23T10:02:00Z","type":"response_item","payload":{"type":"agent_message","content":[{"type":"output_text","text":"I did not run full verification. Should I continue? api_key=SECRET_VALUE"}]}}"#,
        ]
        .join("\n"),
    )
    .expect("codex matching session");
    fs::write(
        codex_root.join("mention-only.jsonl"),
        [
            r#"{"timestamp":"2026-06-23T11:00:00Z","type":"session_meta","payload":{"id":"s2","cwd":"F:\\other"}}"#,
            r#"{"timestamp":"2026-06-23T11:01:00Z","type":"response_item","payload":{"type":"agent_message","content":[{"type":"output_text","text":"F:\\rag_sys is mentioned but this is not its session"}]}}"#,
        ]
        .join("\n"),
    )
    .expect("codex mention-only session");
    fs::write(
        claude_projects.join("F--rag-sys").join("claude.jsonl"),
        [
            r#"{"timestamp":"2026-06-22T10:00:00Z","type":"mode","mode":"default","sessionId":"c1"}"#,
            r#"{"timestamp":"2026-06-22T10:01:00Z","type":"assistant","sessionId":"c1","message":{"content":[{"type":"text","text":"Tests failed; remaining work in frontend/src/App.vue"},{"type":"tool_use","name":"Bash","input":{"command":"npm run build","file_path":"frontend/src/App.vue"}}]}}"#,
            r#"{"timestamp":"2026-06-22T10:02:00Z","type":"user","sessionId":"c1","message":{"content":[{"type":"tool_result","content":"Error: timeout while running build"}]}}"#,
        ]
        .join("\n"),
    )
    .expect("claude session");

    let report = analyze_local_dev_history(DevHistoryAnalysisOptions {
        workspace: PathBuf::from("F:/rag_sys"),
        codex_sessions_root: Some(codex_root),
        claude_projects_root: Some(claude_projects),
        top_limit: 1,
    })
    .expect("history report");

    let codex = report
        .sources
        .iter()
        .find(|source| source.source == "codex")
        .expect("codex source");
    assert_eq!(codex.files, 1);
    assert_eq!(codex.sessions, 1);
    assert!(
        codex
            .top_tools
            .iter()
            .any(|item| item.key == "codex:shell_command")
    );
    assert!(
        codex
            .top_signals
            .iter()
            .any(|item| item.key == "codex:agent-question")
    );

    let claude = report
        .sources
        .iter()
        .find(|source| source.source == "claude-code")
        .expect("claude source");
    assert_eq!(claude.files, 1);
    assert!(
        claude
            .top_tools
            .iter()
            .any(|item| item.key == "claude:Bash")
    );
    assert!(
        claude
            .top_file_refs
            .iter()
            .any(|item| item.key == "frontend/src/App.vue")
    );

    let encoded = serde_json::to_string(&report).expect("encode report");
    assert!(!encoded.contains("SECRET_VALUE"));
    assert!(
        report
            .findings
            .iter()
            .any(|finding| finding.kind == "verification_entropy")
    );
    assert!(
        report
            .findings
            .iter()
            .any(|finding| finding.kind == "user_interrupt_entropy")
    );
}

#[test]
fn local_dev_history_flags_subagent_lifecycle_fragmentation() {
    let temp = tempfile::tempdir().expect("temp dir");
    let codex_root = temp.path().join("codex-sessions");
    let claude_projects = temp.path().join("claude-projects");
    fs::create_dir_all(&codex_root).expect("codex root");
    fs::create_dir_all(claude_projects.join("F--rag-sys").join("subagents"))
        .expect("claude project");

    fs::write(
        codex_root.join("matching.jsonl"),
        [
            r#"{"timestamp":"2026-06-23T10:00:00Z","type":"session_meta","payload":{"id":"s1","cwd":"F:\\rag_sys"}}"#,
            r#"{"timestamp":"2026-06-23T10:01:00Z","type":"response_item","payload":{"type":"function_call","name":"spawn_agent","arguments":"{\"task\":\"inspect parser\"}"}}"#,
            r#"{"timestamp":"2026-06-23T10:02:00Z","type":"response_item","payload":{"type":"function_call","name":"spawn_agent","arguments":"{\"task\":\"inspect ui\"}"}}"#,
            r#"{"timestamp":"2026-06-23T10:03:00Z","type":"response_item","payload":{"type":"function_call","name":"close_agent","arguments":"{\"agent\":\"worker-1\"}"}}"#,
        ]
        .join("\n"),
    )
    .expect("codex matching session");
    fs::write(
        claude_projects
            .join("F--rag-sys")
            .join("subagents")
            .join("worker.jsonl"),
        r#"{"timestamp":"2026-06-22T10:01:00Z","type":"assistant","message":{"content":[{"type":"text","text":"subagent inspected files but no parent integration is visible"}]}}"#,
    )
    .expect("claude subagent session");

    let report = analyze_local_dev_history(DevHistoryAnalysisOptions {
        workspace: PathBuf::from("F:/rag_sys"),
        codex_sessions_root: Some(codex_root),
        claude_projects_root: Some(claude_projects),
        top_limit: 10,
    })
    .expect("history report");

    let finding = report
        .findings
        .iter()
        .find(|finding| finding.kind == "subagent_lifecycle_entropy")
        .expect("subagent lifecycle finding");
    assert_eq!(finding.severity, "warning");
    assert!(
        finding
            .evidence
            .iter()
            .any(|item| item.contains("spawn_agent=2")
                && item.contains("close_agent=1")
                && item.contains("wait_agent=0")),
        "Codex lifecycle counts should be evidence: {finding:?}"
    );
    assert!(
        finding
            .evidence
            .iter()
            .any(|item| item.contains("Claude Code subagent transcript files: 1")),
        "Claude subagent file count should be evidence: {finding:?}"
    );
    assert!(
        finding.monitor_response.iter().any(|item| {
            item.contains("joined_with_summary")
                && item.contains("cancelled_with_reason")
                && item.contains("timed_out")
        }),
        "monitor response should require terminal worker outcomes: {finding:?}"
    );
}

#[test]
fn raw_dev_history_export_copies_matched_transcripts_with_manifest() {
    let temp = tempfile::tempdir().expect("temp dir");
    let codex_root = temp.path().join("codex-sessions");
    let claude_projects = temp.path().join("claude-projects");
    let output_root = temp.path().join("exports");
    fs::create_dir_all(&codex_root).expect("codex root");
    fs::create_dir_all(claude_projects.join("F--rag-sys").join("subagents"))
        .expect("claude project");
    fs::create_dir_all(temp.path().join(".codex")).expect("codex auth dir");
    fs::write(
        temp.path().join(".codex").join("auth.json"),
        r#"{"token":"DO_NOT_COPY"}"#,
    )
    .expect("auth file");

    fs::write(
        codex_root.join("matching.jsonl"),
        [
            r#"{"timestamp":"2026-06-23T10:00:00Z","type":"session_meta","payload":{"id":"s1","cwd":"F:\\rag_sys"}}"#,
            r#"{"timestamp":"2026-06-23T10:01:00Z","type":"response_item","payload":{"type":"agent_message","content":[{"type":"output_text","text":"raw secret marker SECRET_VALUE should remain in raw export"}]}}"#,
        ]
        .join("\n"),
    )
    .expect("codex matching session");
    fs::write(
        codex_root.join("mention-only.jsonl"),
        [
            r#"{"timestamp":"2026-06-23T11:00:00Z","type":"session_meta","payload":{"id":"s2","cwd":"F:\\other"}}"#,
            r#"{"timestamp":"2026-06-23T11:01:00Z","type":"response_item","payload":{"type":"agent_message","content":[{"type":"output_text","text":"F:\\rag_sys is mentioned but this is not its session"}]}}"#,
        ]
        .join("\n"),
    )
    .expect("codex mention-only session");
    fs::write(
        claude_projects.join("F--rag-sys").join("claude.jsonl"),
        r#"{"timestamp":"2026-06-22T10:00:00Z","type":"assistant","message":{"content":[{"type":"text","text":"raw claude transcript"}]}}"#,
    )
    .expect("claude session");
    fs::write(
        claude_projects
            .join("F--rag-sys")
            .join("subagents")
            .join("worker.jsonl"),
        r#"{"timestamp":"2026-06-22T10:01:00Z","type":"assistant","message":{"content":[{"type":"text","text":"raw subagent transcript"}]}}"#,
    )
    .expect("claude subagent session");

    let report = export_raw_dev_history(DevHistoryRawExportOptions {
        workspace: PathBuf::from("F:/rag_sys"),
        codex_sessions_root: Some(codex_root.clone()),
        claude_projects_root: Some(claude_projects.clone()),
        output_root: output_root.clone(),
        package_name: Some("raw-package-test".into()),
    })
    .expect("raw export");

    assert_eq!(report.included.codex_files_matched, 1);
    assert_eq!(report.included.claude_files_matched, 2);
    assert_eq!(report.included.total_files_copied, 3);
    assert_eq!(report.copy_errors.len(), 0);
    assert!(report.warning.contains("RAW CHAT TRANSCRIPTS"));
    assert!(
        report
            .excluded
            .iter()
            .any(|item| item.contains("auth/config"))
    );

    let package_dir = PathBuf::from(&report.package_dir);
    assert!(package_dir.join("manifest.json").exists());
    assert!(package_dir.join("README.md").exists());
    assert!(
        package_dir
            .join("raw")
            .join("codex-sessions")
            .join("matching.jsonl")
            .exists()
    );
    assert!(
        !package_dir
            .join("raw")
            .join("codex-sessions")
            .join("mention-only.jsonl")
            .exists()
    );
    assert!(
        package_dir
            .join("raw")
            .join("claude-code-projects")
            .join("F--rag-sys")
            .join("subagents")
            .join("worker.jsonl")
            .exists()
    );
    assert!(!package_dir.join(".codex").join("auth.json").exists());

    let raw_codex = fs::read_to_string(
        package_dir
            .join("raw")
            .join("codex-sessions")
            .join("matching.jsonl"),
    )
    .expect("raw codex copy");
    assert!(raw_codex.contains("SECRET_VALUE"));

    let manifest: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(package_dir.join("manifest.json")).expect("manifest"),
    )
    .expect("manifest json");
    assert_eq!(
        manifest.pointer("/included/total_files_copied"),
        Some(&serde_json::json!(3))
    );
    assert!(
        manifest
            .get("files")
            .and_then(serde_json::Value::as_array)
            .expect("manifest files")
            .iter()
            .any(
                |file| file.get("package_path").and_then(serde_json::Value::as_str)
                    == Some("raw/codex-sessions/matching.jsonl")
                    && file
                        .get("digest")
                        .and_then(serde_json::Value::as_str)
                        .is_some_and(|digest| digest.starts_with("fnv1a64:"))
            )
    );
}

#[test]
fn raw_dev_history_export_rejects_existing_package_directory() {
    let temp = tempfile::tempdir().expect("temp dir");
    let output_root = temp.path().join("exports");
    let existing_package = output_root.join("raw-package-test");
    fs::create_dir_all(&existing_package).expect("existing package");
    fs::write(existing_package.join("stale.jsonl"), "old transcript").expect("stale evidence");

    let error = export_raw_dev_history(DevHistoryRawExportOptions {
        workspace: PathBuf::from("F:/rag_sys"),
        codex_sessions_root: None,
        claude_projects_root: None,
        output_root,
        package_name: Some("raw-package-test".into()),
    })
    .expect_err("existing raw transcript package must not be reused");

    match error {
        DevHistoryError::PackageExists { path } => {
            assert_eq!(path, existing_package);
        }
        other => panic!("expected package collision error, got {other}"),
    }
}

#[test]
fn raw_dev_history_export_rejects_output_root_inside_workspace_outside_monitor_store() {
    let temp = tempfile::tempdir().expect("temp dir");
    let workspace = temp.path().join("workspace");
    fs::create_dir_all(&workspace).expect("workspace");
    let output_root = workspace.join("raw-transcripts");

    let error = export_raw_dev_history(DevHistoryRawExportOptions {
        workspace: workspace.clone(),
        codex_sessions_root: None,
        claude_projects_root: None,
        output_root: output_root.clone(),
        package_name: Some("raw-package-test".into()),
    })
    .expect_err("raw transcripts must not be exported into arbitrary workspace directories");

    match error {
        DevHistoryError::UnsafeOutputRoot {
            path,
            workspace: root,
        } => {
            assert_eq!(path, output_root);
            assert_eq!(root, workspace);
        }
        other => panic!("expected unsafe output-root error, got {other}"),
    }
    assert!(!output_root.exists());
}

#[test]
fn raw_dev_history_export_allows_output_root_under_monitor_store() {
    let temp = tempfile::tempdir().expect("temp dir");
    let workspace = temp.path().join("workspace");
    fs::create_dir_all(&workspace).expect("workspace");
    let output_root = workspace.join(".agent-monitor").join("exports");

    let report = export_raw_dev_history(DevHistoryRawExportOptions {
        workspace,
        codex_sessions_root: None,
        claude_projects_root: None,
        output_root,
        package_name: Some("raw-package-test".into()),
    })
    .expect("raw export under monitor store");

    assert_eq!(report.included.total_files_copied, 0);
    assert!(
        PathBuf::from(report.package_dir)
            .join("manifest.json")
            .exists()
    );
}
