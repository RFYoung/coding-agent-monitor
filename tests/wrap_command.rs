use coding_agent_monitor::{
    AgentKind, ProjectStore, WorktreeLockRequest, WorktreeLockResult, WrappedCommand,
    run_wrapped_command,
};

#[test]
fn wrapped_command_tees_output_and_persists_capture_events() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    let command = platform_test_command();
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();

    let result = run_wrapped_command(
        WrappedCommand {
            agent: AgentKind::Codex,
            session: Some("wrap-test".into()),
            command,
        },
        &mut store,
        &mut stdout,
        &mut stderr,
    )
    .expect("wrapped command should run");

    assert_eq!(result.exit_code, Some(7));
    assert!(
        String::from_utf8_lossy(&stdout).contains("wrapped-out"),
        "stdout was not tee'd"
    );
    assert!(
        String::from_utf8_lossy(&stderr).contains("wrapped-err"),
        "stderr was not tee'd"
    );

    let events =
        std::fs::read_to_string(temp.path().join(".agent-monitor/events.jsonl")).expect("events");
    assert!(events.contains("\"kind\":\"command_output\""));
    assert!(events.contains("\"kind\":\"command_result\""));
    assert!(events.contains("\"stream\":\"stdout\""));
    assert!(events.contains("\"stream\":\"stderr\""));
    assert!(events.contains("\"exit_code\":7"));
}

#[test]
fn wrapped_command_detects_interventions_from_captured_output() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();

    run_wrapped_command(
        WrappedCommand {
            agent: AgentKind::Codex,
            session: Some("wrap-test".into()),
            command: platform_failure_command(),
        },
        &mut store,
        &mut stdout,
        &mut stderr,
    )
    .expect("wrapped command should run");

    let interventions =
        std::fs::read_to_string(temp.path().join(".agent-monitor/interventions.jsonl"))
            .expect("interventions");
    assert!(interventions.contains("\"kind\":\"service_failure\""));
    assert!(interventions.contains("\"action\":\"retry_same_agent\""));
}

#[test]
fn wrapped_command_dispatches_advice_from_captured_completion_claim() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();

    run_wrapped_command(
        WrappedCommand {
            agent: AgentKind::Codex,
            session: Some("wrap-test".into()),
            command: platform_unverified_completion_command(),
        },
        &mut store,
        &mut stdout,
        &mut stderr,
    )
    .expect("wrapped command should run");

    let root = temp.path().join(".agent-monitor");
    let advice = std::fs::read_to_string(root.join("advice.jsonl")).expect("advice");
    assert!(advice.contains("\"force_verification\""));
    assert!(root.join("outbox").join("codex").join("latest.md").exists());
}

#[test]
fn wrapped_command_rejects_pi_without_external_sandbox() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();

    let error = run_wrapped_command(
        WrappedCommand {
            agent: AgentKind::Pi,
            session: Some("pi-wrap-test".into()),
            command: platform_test_command(),
        },
        &mut store,
        &mut stdout,
        &mut stderr,
    )
    .expect_err("Pi must require an external sandbox wrapper");

    assert!(error.to_string().contains("external sandbox"));
    assert!(!store.root().join("events.jsonl").exists());
}

#[test]
fn wrapped_command_acquires_and_releases_worktree_lock() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();

    run_wrapped_command(
        WrappedCommand {
            agent: AgentKind::Codex,
            session: Some("wrap-lock-test".into()),
            command: platform_success_command(),
        },
        &mut store,
        &mut stdout,
        &mut stderr,
    )
    .expect("wrapped command should run");

    let lock_log = std::fs::read_to_string(store.root().join("locks.jsonl"))
        .expect("wrapped command lock log");
    assert!(lock_log.contains("\"kind\":\"acquired\""));
    assert!(lock_log.contains("\"kind\":\"released\""));
    assert!(lock_log.contains("\"owner_agent\":\"codex\""));
    let reacquired = store
        .try_acquire_worktree_lock(&WorktreeLockRequest {
            worktree: temp.path().display().to_string(),
            owner_agent: "claude-code".into(),
            session: Some("after-wrap".into()),
        })
        .expect("reacquire after wrapped command");
    assert!(matches!(reacquired, WorktreeLockResult::Acquired(_)));
}

#[test]
fn wrapped_command_rejects_launch_when_worktree_is_locked_by_another_owner() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    let existing = store
        .try_acquire_worktree_lock(&WorktreeLockRequest {
            worktree: temp.path().display().to_string(),
            owner_agent: "claude-code".into(),
            session: Some("existing-owner".into()),
        })
        .expect("existing lock");
    assert!(matches!(existing, WorktreeLockResult::Acquired(_)));
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();

    let error = run_wrapped_command(
        WrappedCommand {
            agent: AgentKind::Codex,
            session: Some("wrap-lock-conflict".into()),
            command: platform_success_command(),
        },
        &mut store,
        &mut stdout,
        &mut stderr,
    )
    .expect_err("locked worktree should reject wrapped launch");

    assert!(error.to_string().contains("already locked"));
    assert!(stdout.is_empty());
    assert!(stderr.is_empty());
    assert!(!store.root().join("events.jsonl").exists());
    let lock_log =
        std::fs::read_to_string(store.root().join("locks.jsonl")).expect("lock conflict log");
    assert!(lock_log.contains("\"kind\":\"conflict\""));
    assert!(lock_log.contains("\"requested_owner\":\"codex\""));
}

#[cfg(windows)]
fn platform_test_command() -> Vec<String> {
    vec![
        "cmd".into(),
        "/C".into(),
        "echo wrapped-out& echo wrapped-err 1>&2 & exit /b 7".into(),
    ]
}

#[cfg(windows)]
fn platform_success_command() -> Vec<String> {
    vec!["cmd".into(), "/C".into(), "echo wrapped-ok".into()]
}

#[cfg(windows)]
fn platform_unverified_completion_command() -> Vec<String> {
    vec![
        "cmd".into(),
        "/C".into(),
        "echo Implementation complete. I did not run tests.".into(),
    ]
}

#[cfg(windows)]
fn platform_failure_command() -> Vec<String> {
    vec![
        "cmd".into(),
        "/C".into(),
        "echo upstream service unavailable 1>&2".into(),
    ]
}

#[cfg(not(windows))]
fn platform_test_command() -> Vec<String> {
    vec![
        "sh".into(),
        "-c".into(),
        "echo wrapped-out; echo wrapped-err >&2; exit 7".into(),
    ]
}

#[cfg(not(windows))]
fn platform_success_command() -> Vec<String> {
    vec!["sh".into(), "-c".into(), "echo wrapped-ok".into()]
}

#[cfg(not(windows))]
fn platform_unverified_completion_command() -> Vec<String> {
    vec![
        "sh".into(),
        "-c".into(),
        "echo 'Implementation complete. I did not run tests.'".into(),
    ]
}

#[cfg(not(windows))]
fn platform_failure_command() -> Vec<String> {
    vec![
        "sh".into(),
        "-c".into(),
        "echo upstream service unavailable >&2".into(),
    ]
}
