use coding_agent_monitor::{
    Action, AgentKind, AgentReviewAction, AgentReviewStatus, DashboardSnapshot, Event, EventKind,
    Intervention, InterventionKind, ProjectStore, RunningAgent, create_demo_workspace,
    judge_snapshot,
};
use std::fs;
use std::path::Path;
use std::path::PathBuf;
use std::process::Command;

#[test]
fn judge_snapshot_flags_degraded_agent_for_fresh_handoff() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_intervention(&Intervention {
            kind: InterventionKind::AgentDegraded,
            action: Action::SpawnFreshAgent,
            agent: Some("codex".into()),
            reason: "agent appears to have lost design memory".into(),
        })
        .expect("intervention");
    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");

    let report = judge_snapshot(temp.path(), &snapshot, &[]);

    assert_eq!(report.status, AgentReviewStatus::Intervene);
    assert_eq!(
        report.findings[0].recommended_action,
        AgentReviewAction::SpawnFreshAgent
    );
    assert_eq!(report.findings[0].agent.as_deref(), Some("codex"));
}

#[test]
fn judge_snapshot_warns_when_running_agent_has_no_monitor_telemetry() {
    let temp = tempfile::tempdir().expect("temp dir");
    let store = ProjectStore::open(temp.path()).expect("store");
    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let running = [RunningAgent::new(42, AgentKind::ClaudeCode, "node.exe")
        .with_cwd(Some(temp.path().to_path_buf()))];

    let report = judge_snapshot(temp.path(), &snapshot, &running);

    assert_eq!(report.status, AgentReviewStatus::Watch);
    assert_eq!(
        report.findings[0].recommended_action,
        AgentReviewAction::InstallTelemetry
    );
    assert!(report.findings[0].evidence.contains("no monitor telemetry"));
}

#[test]
fn judge_snapshot_flags_unverified_completion_claims() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some("Implementation complete. I did not run tests.".into()),
            ..Event::default()
        })
        .expect("event");
    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");

    let report = judge_snapshot(temp.path(), &snapshot, &[]);

    assert_eq!(report.status, AgentReviewStatus::Intervene);
    assert_eq!(
        report.findings[0].recommended_action,
        AgentReviewAction::ForceVerification
    );
}

#[test]
fn judge_snapshot_recommends_judge_agent_for_untraced_dirty_hunk() {
    let temp = tempfile::tempdir().expect("temp dir");
    init_git_repo(temp.path());
    write_file(temp.path().join("src/lib.rs"), "fn one() {}\nfn two() {}\n");
    git(temp.path(), ["add", "src/lib.rs"]);
    git_commit(temp.path(), "initial");
    write_file(
        temp.path().join("src/lib.rs"),
        "fn one() {}\nfn two_changed() {}\n",
    );
    let store = ProjectStore::open(temp.path()).expect("store");
    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");

    let report = judge_snapshot(temp.path(), &snapshot, &[]);

    assert_eq!(report.status, AgentReviewStatus::Intervene);
    let finding = report
        .findings
        .iter()
        .find(|finding| finding.category == "suspicious_untraced_change")
        .expect("suspicious change finding");
    assert_eq!(
        finding.recommended_action,
        AgentReviewAction::SpawnJudgeAgent
    );
    assert!(finding.evidence.contains("src/lib.rs"));
}

#[test]
fn create_demo_workspace_refuses_non_empty_existing_directory() {
    let temp = tempfile::tempdir().expect("temp dir");
    let workspace = temp.path().join("demo");
    std::fs::create_dir_all(&workspace).expect("workspace");
    std::fs::write(workspace.join("keep.txt"), "user data").expect("seed file");

    let error = create_demo_workspace(&workspace).expect_err("demo should refuse user data");

    assert!(error.to_string().contains("not empty"));
}

#[test]
fn create_demo_workspace_seeds_reviewable_monitor_logs() {
    let temp = tempfile::tempdir().expect("temp dir");
    let workspace = temp.path().join("demo");

    create_demo_workspace(&workspace).expect("create demo");

    assert!(workspace.join("README.md").exists());
    let store = ProjectStore::open(&workspace).expect("store");
    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let report = judge_snapshot(PathBuf::from(&workspace), &snapshot, &[]);

    assert!(snapshot.event_count > 0);
    assert!(!report.findings.is_empty());
}

fn init_git_repo(path: &Path) {
    git(path, ["init"]);
    git(path, ["config", "user.email", "test@example.com"]);
    git(path, ["config", "user.name", "Test User"]);
}

fn git<const N: usize>(path: &Path, args: [&str; N]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(path)
        .status()
        .expect("run git");
    assert!(status.success(), "git command failed");
}

fn git_commit(path: &Path, message: &str) {
    git(path, ["commit", "-m", message]);
}

fn write_file(path: impl AsRef<Path>, content: &str) {
    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create parent");
    }
    fs::write(path, content).expect("write file");
}
