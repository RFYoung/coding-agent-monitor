use coding_agent_monitor::{
    AcceptanceCoverageStatus, ActionOutcome, AdvisorDecision, AdvisorEntropyEstimate,
    AdvisorEvidenceRef, AdvisorPacketDraft, AdvisorProviderKind, AgentKind,
    CompletionCertificateStatus, Config, ControlAction, ControlActionKind, ControlCaseFile,
    ControlPacket, DashboardSnapshot, DevHistoryFinding, DevHistoryReport, DevHistorySourceReport,
    DispatchStatus, EntropyDelta, EntropyKind, EntropyTrend, Event, EventKind, MemorySource,
    OutcomeStatus, PacketInstruction, PacketInstructionPriority, PacketPreconditions,
    PacketUrgency, ProbeSpec, ProjectConfig, ProjectStore, RedactionStatus, RepoAuditStatus,
    RepoChangeKind, RepoHunkHistoryEntry, RepoTraceStatus, RequirementGraphQuery,
    RequirementSource, RuntimeValidationSurface, TraceEntry, ValidationOutcome,
    VerificationFailureClass, VerificationRunStatus, VerificationScope, VerificationSuite,
    VerifierRun, WorktreeLock, WorktreeLockRequest, WorktreeLockResult, adapter_capabilities_for,
    adapter_capabilities_for_config, advise_workspace, build_control_case_file,
    build_control_case_file_with_config, load_decision_trails, load_requirement_graph,
    promote_memory_candidate, run_jsonl_with_store, run_probe, run_verifier,
    validate_advisor_decision, validate_control_action, validate_control_action_detailed,
};
use serde_json::json;
use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::process::Command;
use std::sync::mpsc;
use std::thread;

fn init_git_repo(workspace: &Path) -> String {
    run_git(workspace, ["init"]);
    run_git(workspace, ["config", "user.email", "monitor@example.test"]);
    run_git(workspace, ["config", "user.name", "Monitor Test"]);
    std::fs::write(workspace.join("seed.txt"), "seed\n").expect("seed file");
    run_git(workspace, ["add", "seed.txt"]);
    run_git(workspace, ["commit", "-m", "seed"]);
    git_head(workspace)
}

fn git_head(workspace: &Path) -> String {
    let output = Command::new("git")
        .current_dir(workspace)
        .args(["rev-parse", "HEAD"])
        .output()
        .expect("git rev-parse");
    assert!(
        output.status.success(),
        "git rev-parse failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("git head utf8")
        .trim()
        .to_string()
}

fn rewrite_only_worktree_lock_timestamp(store_root: &Path, acquired_at: &str) {
    let lock_dir = store_root.join("locks").join("worktrees");
    let lock_path = std::fs::read_dir(&lock_dir)
        .expect("lock dir")
        .map(|entry| entry.expect("lock entry").path())
        .find(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .expect("lock file");
    let mut lock: WorktreeLock =
        serde_json::from_str(&std::fs::read_to_string(&lock_path).expect("lock json"))
            .expect("decode lock");
    lock.acquired_at = acquired_at.into();
    std::fs::write(
        lock_path,
        format!("{}\n", serde_json::to_string(&lock).expect("encode lock")),
    )
    .expect("rewrite lock");
}

fn append_ask_user_advice_at(store: &mut ProjectStore, workspace: &Path, built_at: &str) {
    append_action_advice_at(
        store,
        workspace,
        built_at,
        ControlAction::AskUser {
            question: "Existing recent user interrupt.".into(),
        },
        "User decision required",
    );
}

fn append_action_advice_at(
    store: &mut ProjectStore,
    workspace: &Path,
    built_at: &str,
    final_action: ControlAction,
    title: &str,
) {
    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let mut case_file = build_control_case_file(workspace, &snapshot);
    case_file.case_file_id = format!("case-action-{built_at}");
    case_file.built_at = built_at.into();
    store.append_case_file(&case_file).expect("case file");
    let packet = ControlPacket {
        packet_id: format!("packet-action-{built_at}"),
        target_agent: target_agent_for_test_action(&final_action),
        urgency: PacketUrgency::Urgent,
        title: title.into(),
        summary: "Historical control action.".into(),
        instructions: vec![PacketInstruction {
            priority: PacketInstructionPriority::Must,
            text: "Historical persisted advice.".into(),
        }],
        evidence_refs: vec![],
        forbidden: vec![],
        success_criteria: vec!["Historical action was recorded.".into()],
        preconditions: PacketPreconditions::default(),
    };
    store
        .append_advice(&coding_agent_monitor::AdviceRun {
            advice_id: format!("advice-action-{built_at}"),
            case_file_id: case_file.case_file_id,
            advisor_used: false,
            advisor_error: None,
            advisor_decision: None,
            validation_outcome: ValidationOutcome::Approved(final_action.clone()),
            final_action,
            control_rationale: Default::default(),
            packet,
            dispatch_result: coding_agent_monitor::DispatchResult::default(),
            packet_path: None,
        })
        .expect("advice");
}

fn append_dispatched_action_advice_at(
    store: &mut ProjectStore,
    workspace: &Path,
    built_at: &str,
    final_action: ControlAction,
    title: &str,
) -> coding_agent_monitor::AdviceRun {
    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let mut case_file = build_control_case_file(workspace, &snapshot);
    case_file.case_file_id = format!("case-dispatched-action-{built_at}");
    case_file.built_at = built_at.into();
    store.append_case_file(&case_file).expect("case file");
    let packet = ControlPacket {
        packet_id: format!("packet-dispatched-action-{built_at}"),
        target_agent: target_agent_for_test_action(&final_action),
        urgency: PacketUrgency::Urgent,
        title: title.into(),
        summary: "Historical dispatched control action.".into(),
        instructions: vec![PacketInstruction {
            priority: PacketInstructionPriority::Must,
            text: "Historical dispatched advice.".into(),
        }],
        evidence_refs: vec![],
        forbidden: vec![],
        success_criteria: vec!["Historical dispatched action was recorded.".into()],
        preconditions: PacketPreconditions::default(),
    };
    let dispatch_result = store
        .dispatch_control_packet(&packet)
        .expect("dispatch historical packet");
    let advice = coding_agent_monitor::AdviceRun {
        advice_id: format!("advice-dispatched-action-{built_at}"),
        case_file_id: case_file.case_file_id,
        advisor_used: false,
        advisor_error: None,
        advisor_decision: None,
        validation_outcome: ValidationOutcome::Approved(final_action.clone()),
        final_action,
        control_rationale: Default::default(),
        packet,
        packet_path: dispatch_result.path.clone(),
        dispatch_result,
    };
    store.append_advice(&advice).expect("advice");
    advice
}

fn target_agent_for_test_action(action: &ControlAction) -> String {
    match action {
        ControlAction::SwitchAgent { target_agent } => target_agent.clone(),
        ControlAction::SpawnFreshAgent {
            target_agent: Some(target_agent),
        }
        | ControlAction::SpawnJudgeAgent {
            target_agent: Some(target_agent),
        }
        | ControlAction::RetryAgent {
            target_agent: Some(target_agent),
            ..
        }
        | ControlAction::SendFollowUp {
            target_agent: Some(target_agent),
        } => target_agent.clone(),
        _ => "codex".into(),
    }
}

fn run_git<const N: usize>(workspace: &Path, args: [&str; N]) {
    let output = Command::new("git")
        .current_dir(workspace)
        .args(args)
        .output()
        .expect("git command");
    assert!(
        output.status.success(),
        "git command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn file_modified_utc_timestamp(path: &Path) -> String {
    let seconds = std::fs::metadata(path)
        .expect("metadata")
        .modified()
        .expect("modified time")
        .duration_since(std::time::UNIX_EPOCH)
        .expect("mtime after epoch")
        .as_secs() as i64;
    format_test_utc_seconds(seconds)
}

fn format_test_utc_seconds(seconds: i64) -> String {
    let days = seconds.div_euclid(86_400);
    let seconds_of_day = seconds.rem_euclid(86_400);
    let (year, month, day) = test_civil_from_days(days);
    let hour = seconds_of_day / 3_600;
    let minute = seconds_of_day % 3_600 / 60;
    let second = seconds_of_day % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

fn test_civil_from_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    let year = y + if month <= 2 { 1 } else { 0 };
    (year as i32, month as u32, day as u32)
}

#[cfg(windows)]
fn passing_verifier_command() -> &'static str {
    "cmd /C exit 0"
}

#[cfg(not(windows))]
fn passing_verifier_command() -> &'static str {
    "true"
}

#[cfg(windows)]
fn failing_verifier_command() -> &'static str {
    "cmd /C exit 7"
}

#[cfg(not(windows))]
fn failing_verifier_command() -> &'static str {
    "false"
}

#[cfg(windows)]
fn compile_failure_verifier_command() -> &'static str {
    "cmd /C echo error[E0308]: mismatched types 1>&2 & exit /B 1"
}

#[cfg(not(windows))]
fn compile_failure_verifier_command() -> &'static str {
    "sh -c 'echo \"error[E0308]: mismatched types\" >&2; exit 1'"
}

#[cfg(windows)]
fn assertion_failure_verifier_command() -> &'static str {
    "cmd /C echo thread 'parser::tests::nested' panicked at assertion failed 1>&2 & exit /B 1"
}

#[cfg(not(windows))]
fn assertion_failure_verifier_command() -> &'static str {
    "sh -c 'echo \"thread parser::tests::nested panicked at assertion failed\" >&2; exit 1'"
}

#[cfg(windows)]
fn environment_failure_verifier_command() -> &'static str {
    "cmd /C echo connection refused while contacting test database 1>&2 & exit /B 1"
}

#[cfg(not(windows))]
fn environment_failure_verifier_command() -> &'static str {
    "sh -c 'echo \"connection refused while contacting test database\" >&2; exit 1'"
}

#[cfg(windows)]
fn sleeping_verifier_command() -> &'static str {
    "cmd /C ping -n 8 127.0.0.1"
}

#[cfg(not(windows))]
fn sleeping_verifier_command() -> &'static str {
    "sleep 8"
}

#[cfg(windows)]
fn background_pipe_verifier_command() -> &'static str {
    "cmd /C start /B ping -n 8 127.0.0.1"
}

#[cfg(not(windows))]
fn background_pipe_verifier_command() -> &'static str {
    "sleep 8 &"
}

fn set_test_env_var(name: &str, value: &str) {
    unsafe {
        std::env::set_var(name, value);
    }
}

fn serve_advisor_once(decision: serde_json::Value) -> (String, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind advisor server");
    let endpoint = format!(
        "http://{}/v1/chat/completions",
        listener.local_addr().expect("local addr")
    );
    let (request_tx, request_rx) = mpsc::channel();

    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept advisor request");
        let mut request = Vec::new();
        let mut buffer = [0u8; 4096];
        loop {
            let read = stream.read(&mut buffer).expect("read advisor request");
            if read == 0 {
                break;
            }
            request.extend_from_slice(&buffer[..read]);
            if http_request_complete(&request) {
                break;
            }
        }
        let request_text = String::from_utf8_lossy(&request).to_string();
        request_tx.send(request_text).expect("send advisor request");

        let content = decision.to_string();
        let body = json!({
            "choices": [
                {
                    "message": {
                        "content": content
                    }
                }
            ]
        })
        .to_string();
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        stream
            .write_all(response.as_bytes())
            .expect("write advisor response");
    });

    (endpoint, request_rx)
}

fn http_request_complete(request: &[u8]) -> bool {
    let Some(header_end) = request.windows(4).position(|window| window == b"\r\n\r\n") else {
        return false;
    };
    let headers = String::from_utf8_lossy(&request[..header_end]);
    let content_length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())
                .flatten()
        })
        .unwrap_or(0);
    request.len() >= header_end + 4 + content_length
}

fn advisor_request_case_file(request: &str) -> serde_json::Value {
    let (_, body) = request
        .split_once("\r\n\r\n")
        .expect("advisor request body");
    let payload: serde_json::Value = serde_json::from_str(body).expect("advisor json payload");
    let messages = payload
        .get("messages")
        .and_then(serde_json::Value::as_array)
        .expect("messages");
    let content = messages
        .iter()
        .find(|message| message.get("role").and_then(serde_json::Value::as_str) == Some("user"))
        .and_then(|message| message.get("content"))
        .and_then(serde_json::Value::as_str)
        .expect("user case file content");
    serde_json::from_str(content).expect("case file json")
}

fn case_file_evidence_ids(case_file: &serde_json::Value) -> Vec<&str> {
    case_file
        .get("evidence")
        .and_then(serde_json::Value::as_array)
        .expect("case file evidence")
        .iter()
        .filter_map(|item| item.get("id").and_then(serde_json::Value::as_str))
        .collect()
}

fn case_file_action_values<'a>(case_file: &'a serde_json::Value, field: &str) -> Vec<&'a str> {
    case_file
        .get(field)
        .and_then(serde_json::Value::as_array)
        .expect("case file action array")
        .iter()
        .filter_map(serde_json::Value::as_str)
        .collect()
}

fn advisor_case_file_from_request(request: &str) -> serde_json::Value {
    let (_, body) = request
        .split_once("\r\n\r\n")
        .expect("http request body separator");
    let payload: serde_json::Value = serde_json::from_str(body).expect("advisor request json");
    let content = payload
        .pointer("/messages/1/content")
        .and_then(|value| value.as_str())
        .expect("advisor case file content");
    serde_json::from_str(content).expect("advisor case file json")
}

fn advisor_visible_evidence_ids(case_file: &serde_json::Value) -> Vec<String> {
    case_file
        .get("evidence")
        .and_then(|value| value.as_array())
        .into_iter()
        .flatten()
        .filter_map(|item| item.get("id").and_then(|value| value.as_str()))
        .map(ToOwned::to_owned)
        .collect()
}

fn dev_history_evidence_id(case_file: &ControlCaseFile, finding_kind: &str) -> String {
    case_file
        .evidence
        .iter()
        .find(|item| item.kind == "DevHistoryFinding" && item.summary.contains(finding_kind))
        .map(|item| item.id.clone())
        .unwrap_or_else(|| panic!("dev-history evidence for {finding_kind}"))
}

#[path = "entropy_control/config.rs"]
mod config;

#[path = "entropy_control/evidence_provenance.rs"]
mod evidence_provenance;

#[test]
fn verifier_runner_executes_configured_command_and_persists_result() {
    let temp = tempfile::tempdir().expect("temp dir");
    let store = ProjectStore::open(temp.path()).expect("store");
    std::fs::write(
        store.root().join("config.json"),
        format!(
            r#"{{
              "verifiers": [
                {{
                  "id": "smoke",
                  "command": "{}",
                  "scope": "targeted",
                  "timeout_secs": 30,
                  "paths": []
                }}
              ]
            }}"#,
            passing_verifier_command()
        ),
    )
    .expect("config");

    let run = run_verifier(temp.path(), "smoke").expect("verifier run");

    assert_eq!(run.verifier_id.as_deref(), Some("smoke"));
    assert_eq!(run.command, passing_verifier_command());
    assert_eq!(run.status, VerificationRunStatus::Passed);
    assert_eq!(run.exit_code, Some(0));
    assert!(run.completed_at.is_some());
    assert!(run.output_digest.starts_with("fnv1a64:"));

    let log = std::fs::read_to_string(store.root().join("verifier-runs.jsonl"))
        .expect("verifier run log");
    assert!(log.contains("\"verifier_id\":\"smoke\""));
    assert!(log.contains("\"status\":\"passed\""));
}

#[test]
fn verifier_runner_times_out_configured_command_and_persists_result() {
    let temp = tempfile::tempdir().expect("temp dir");
    let store = ProjectStore::open(temp.path()).expect("store");
    std::fs::write(
        store.root().join("config.json"),
        format!(
            r#"{{
              "verifiers": [
                {{
                  "id": "sleepy",
                  "command": "{}",
                  "scope": "targeted",
                  "timeout_secs": 1,
                  "paths": []
                }}
              ]
            }}"#,
            sleeping_verifier_command()
        ),
    )
    .expect("config");

    let started = std::time::Instant::now();
    let run = run_verifier(temp.path(), "sleepy").expect("verifier run");

    assert!(
        started.elapsed() < std::time::Duration::from_secs(3),
        "verifier should be killed near the configured timeout"
    );
    assert_eq!(run.verifier_id.as_deref(), Some("sleepy"));
    assert_eq!(run.status, VerificationRunStatus::TimedOut);
    assert_eq!(run.exit_code, None);
    assert!(run.completed_at.is_some());
    assert!(run.output_digest.starts_with("fnv1a64:"));

    let log = std::fs::read_to_string(store.root().join("verifier-runs.jsonl"))
        .expect("verifier run log");
    assert!(log.contains("\"verifier_id\":\"sleepy\""));
    assert!(log.contains("\"status\":\"timed_out\""));
}

#[test]
fn verifier_runner_times_out_background_child_that_keeps_output_pipe() {
    let temp = tempfile::tempdir().expect("temp dir");
    let store = ProjectStore::open(temp.path()).expect("store");
    std::fs::write(
        store.root().join("config.json"),
        json!({
            "verifiers": [
                {
                    "id": "background",
                    "command": background_pipe_verifier_command(),
                    "scope": "targeted",
                    "timeout_secs": 1,
                    "paths": []
                }
            ]
        })
        .to_string(),
    )
    .expect("config");

    let started = std::time::Instant::now();
    let run = run_verifier(temp.path(), "background").expect("verifier run");

    assert!(
        started.elapsed() < std::time::Duration::from_secs(3),
        "background child should not keep verifier output collection blocked"
    );
    assert_eq!(run.verifier_id.as_deref(), Some("background"));
    assert_eq!(run.status, VerificationRunStatus::TimedOut);
    assert_eq!(run.exit_code, None);

    let log = std::fs::read_to_string(store.root().join("verifier-runs.jsonl"))
        .expect("verifier run log");
    assert!(log.contains("\"verifier_id\":\"background\""));
    assert!(log.contains("\"status\":\"timed_out\""));
}

#[test]
fn verifier_runner_classifies_compile_failure_and_persists_result() {
    let temp = tempfile::tempdir().expect("temp dir");
    let store = ProjectStore::open(temp.path()).expect("store");
    std::fs::write(
        store.root().join("config.json"),
        json!({
            "verifiers": [
                {
                    "id": "compile",
                    "command": compile_failure_verifier_command(),
                    "scope": "targeted",
                    "timeout_secs": 5,
                    "paths": []
                }
            ]
        })
        .to_string(),
    )
    .expect("config");

    let run = run_verifier(temp.path(), "compile").expect("verifier run");

    assert_eq!(run.status, VerificationRunStatus::Failed);
    assert_eq!(run.failure_class, Some(VerificationFailureClass::Compile));
    let log = std::fs::read_to_string(store.root().join("verifier-runs.jsonl"))
        .expect("verifier run log");
    assert!(log.contains("\"failure_class\":\"compile\""));
}

#[test]
fn verifier_runner_classifies_assertion_failure() {
    let temp = tempfile::tempdir().expect("temp dir");
    let store = ProjectStore::open(temp.path()).expect("store");
    std::fs::write(
        store.root().join("config.json"),
        json!({
            "verifiers": [
                {
                    "id": "assertion",
                    "command": assertion_failure_verifier_command(),
                    "scope": "targeted",
                    "timeout_secs": 5,
                    "paths": []
                }
            ]
        })
        .to_string(),
    )
    .expect("config");

    let run = run_verifier(temp.path(), "assertion").expect("verifier run");

    assert_eq!(run.status, VerificationRunStatus::Failed);
    assert_eq!(run.failure_class, Some(VerificationFailureClass::Assertion));
}

#[test]
fn verifier_runner_classifies_environment_failure() {
    let temp = tempfile::tempdir().expect("temp dir");
    let store = ProjectStore::open(temp.path()).expect("store");
    std::fs::write(
        store.root().join("config.json"),
        json!({
            "verifiers": [
                {
                    "id": "environment",
                    "command": environment_failure_verifier_command(),
                    "scope": "targeted",
                    "timeout_secs": 5,
                    "paths": []
                }
            ]
        })
        .to_string(),
    )
    .expect("config");

    let run = run_verifier(temp.path(), "environment").expect("verifier run");

    assert_eq!(run.status, VerificationRunStatus::Failed);
    assert_eq!(
        run.failure_class,
        Some(VerificationFailureClass::Environment)
    );
}

#[test]
fn verifier_runner_classifies_timeout_failure() {
    let temp = tempfile::tempdir().expect("temp dir");
    let store = ProjectStore::open(temp.path()).expect("store");
    std::fs::write(
        store.root().join("config.json"),
        json!({
            "verifiers": [
                {
                    "id": "timeout",
                    "command": sleeping_verifier_command(),
                    "scope": "targeted",
                    "timeout_secs": 1,
                    "paths": []
                }
            ]
        })
        .to_string(),
    )
    .expect("config");

    let run = run_verifier(temp.path(), "timeout").expect("verifier run");

    assert_eq!(run.status, VerificationRunStatus::TimedOut);
    assert_eq!(run.failure_class, Some(VerificationFailureClass::Timeout));
}

#[test]
fn case_file_scores_stale_verification_after_source_change() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:00:00Z".into()),
            event_id: Some("evt-test-pass".into()),
            agent: "codex".into(),
            kind: EventKind::CommandResult,
            command: Some("cargo test".into()),
            exit_code: Some(0),
            ..Event::default()
        })
        .expect("event");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:01:00Z".into()),
            event_id: Some("evt-write".into()),
            agent: "codex".into(),
            kind: EventKind::FileChange,
            file: Some("src/lib.rs".into()),
            rationale: Some("Add entropy control types.".into()),
            ..Event::default()
        })
        .expect("event");

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);
    let verification = case_file
        .entropy
        .score(EntropyKind::Verification)
        .expect("verification score");

    assert!(verification.score >= 75, "{verification:?}");
    assert!(verification.top_causes.iter().any(|cause| {
        cause.contains("source changes") && cause.contains("passing verification")
    }));
    assert!(
        case_file
            .allowed_actions
            .contains(&ControlActionKind::ForceVerification)
    );
    assert!(case_file.evidence.iter().any(|item| item.id == "evt-write"));
}

#[test]
fn case_file_respects_policy_disabling_verification_after_source_change() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:00:00Z".into()),
            event_id: Some("evt-test-pass".into()),
            agent: "codex".into(),
            kind: EventKind::CommandResult,
            command: Some("cargo test".into()),
            exit_code: Some(0),
            ..Event::default()
        })
        .expect("event");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:01:00Z".into()),
            event_id: Some("evt-write".into()),
            agent: "codex".into(),
            kind: EventKind::FileChange,
            file: Some("src/lib.rs".into()),
            rationale: Some("Update source under a no-stale-verification policy.".into()),
            ..Event::default()
        })
        .expect("event");

    let mut config = ProjectConfig::default();
    config.policy.require_verification_after_source_change = false;
    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file_with_config(temp.path(), &snapshot, &config);

    assert_eq!(
        case_file.verification.status,
        coding_agent_monitor::VerificationStatus::Passed
    );
    let verification = case_file
        .entropy
        .score(EntropyKind::Verification)
        .expect("verification score");
    assert!(
        verification.score < 75,
        "policy should suppress stale-verification entropy: {verification:?}"
    );
    assert!(
        !verification
            .top_causes
            .iter()
            .any(|cause| cause.contains("source changes"))
    );
}

#[test]
fn case_file_raises_verification_entropy_for_completion_claim_without_verifier_evidence() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:00:00Z".into()),
            event_id: Some("evt-completion-claim".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some("Implementation complete. Ready for review.".into()),
            ..Event::default()
        })
        .expect("event");

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);
    let verification = case_file
        .entropy
        .score(EntropyKind::Verification)
        .expect("verification score");

    assert!(verification.score >= 80, "{verification:?}");
    assert!(
        verification
            .top_causes
            .iter()
            .any(|cause| cause.contains("completion") && cause.contains("verification")),
        "{verification:?}"
    );
    assert!(
        verification
            .evidence_ids
            .contains(&"evt-completion-claim".to_string())
    );
    assert!(
        case_file.belief_state.hypotheses.iter().any(|belief| {
            belief.kind == coding_agent_monitor::FailureHypothesisKind::StaleVerification
                && belief.evidence_ids.contains(&"evt-completion-claim".into())
                && belief.estimated_probability >= 80
        }),
        "belief state should translate completion-without-verifier evidence into a stale-verification hypothesis: {:?}",
        case_file.belief_state
    );
}

#[test]
fn case_file_clears_completion_claim_entropy_after_later_passing_verifier() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:00:00Z".into()),
            event_id: Some("evt-completion-before-verifier".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some("Task complete. Ready for review.".into()),
            ..Event::default()
        })
        .expect("completion event");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:01:00Z".into()),
            event_id: Some("evt-verifier-after-completion".into()),
            agent: "codex".into(),
            kind: EventKind::CommandResult,
            command: Some("cargo test".into()),
            exit_code: Some(0),
            ..Event::default()
        })
        .expect("verifier event");

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);
    let verification = case_file
        .entropy
        .score(EntropyKind::Verification)
        .expect("verification score");

    assert!(
        verification.score < 80,
        "later passing verifier should clear completion-claim entropy: {verification:?}"
    );
    assert!(
        !verification
            .top_causes
            .iter()
            .any(|cause| cause.contains("completion") && cause.contains("verification")),
        "{verification:?}"
    );
}

#[test]
fn case_file_completion_certificate_blocks_unresolved_requirement_closure() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:00:00Z".into()),
            event_id: Some("evt-user-acceptance-certificate".into()),
            agent: "user".into(),
            kind: EventKind::UserInstruction,
            content: Some(
                "Build the monitor.\nAcceptance criteria:\n- parser handles nested calls\n- export csv report"
                    .into(),
            ),
            ..Event::default()
        })
        .expect("user event");
    store
        .append_verifier_run(&coding_agent_monitor::VerifierRun {
            verifier_run_id: "verifier-run-parser-certificate".into(),
            verifier_id: Some("parser_targeted".into()),
            command: "cargo test parser::nested".into(),
            status: VerificationRunStatus::Passed,
            started_at: "2026-06-22T12:01:00Z".into(),
            completed_at: Some("2026-06-22T12:02:00Z".into()),
            exit_code: Some(0),
            output_digest: "ok".into(),
            failure_class: None,
        })
        .expect("verifier run");

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let config = ProjectConfig {
        verifiers: vec![coding_agent_monitor::VerifierConfig {
            id: "parser_targeted".into(),
            command: "cargo test parser::nested".into(),
            scope: VerificationScope::Targeted,
            timeout_secs: 120,
            paths: vec!["src/parser.rs".into()],
            acceptance_patterns: vec!["parser handles nested calls".into()],
        }],
        ..ProjectConfig::default()
    };
    let case_file = build_control_case_file_with_config(temp.path(), &snapshot, &config);
    let certificate = &case_file.completion_certificate;

    assert_eq!(certificate.status, CompletionCertificateStatus::Blocked);
    assert_eq!(certificate.scoped_requirement_ids.len(), 2);
    assert_eq!(
        certificate.closed_requirement_ids,
        vec!["req-parser-handles-nested-calls".to_string()]
    );
    assert_eq!(
        certificate.unresolved_requirement_ids,
        vec!["req-export-csv-report".to_string()]
    );
    assert!(
        certificate
            .unresolved_incidents
            .iter()
            .any(|incident| incident.summary.contains("requirement closure")),
        "{certificate:?}"
    );
}

#[test]
fn case_file_completion_certificate_allows_closed_requirements_with_fresh_verification() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:00:00Z".into()),
            event_id: Some("evt-user-acceptance-closed".into()),
            agent: "user".into(),
            kind: EventKind::UserInstruction,
            content: Some(
                "Build the monitor.\nAcceptance criteria:\n- parser handles nested calls".into(),
            ),
            ..Event::default()
        })
        .expect("user event");
    store
        .append_verifier_run(&coding_agent_monitor::VerifierRun {
            verifier_run_id: "verifier-run-parser-closed".into(),
            verifier_id: Some("parser_targeted".into()),
            command: "cargo test parser::nested".into(),
            status: VerificationRunStatus::Passed,
            started_at: "2026-06-22T12:01:00Z".into(),
            completed_at: Some("2026-06-22T12:02:00Z".into()),
            exit_code: Some(0),
            output_digest: "ok".into(),
            failure_class: None,
        })
        .expect("verifier run");

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let config = ProjectConfig {
        verifiers: vec![coding_agent_monitor::VerifierConfig {
            id: "parser_targeted".into(),
            command: "cargo test parser::nested".into(),
            scope: VerificationScope::Targeted,
            timeout_secs: 120,
            paths: vec!["src/parser.rs".into()],
            acceptance_patterns: vec!["parser handles nested calls".into()],
        }],
        ..ProjectConfig::default()
    };
    let case_file = build_control_case_file_with_config(temp.path(), &snapshot, &config);
    let certificate = &case_file.completion_certificate;

    assert_eq!(certificate.status, CompletionCertificateStatus::Eligible);
    assert_eq!(
        certificate.closed_requirement_ids,
        vec!["req-parser-handles-nested-calls".to_string()]
    );
    assert!(certificate.unresolved_requirement_ids.is_empty());
    assert_eq!(
        certificate.verification_status,
        coding_agent_monitor::VerificationStatus::Passed
    );
    assert!(
        certificate.unresolved_incidents.is_empty(),
        "{certificate:?}"
    );
}

#[test]
fn case_file_scopes_project_contract_requirements_from_agents_md() {
    let temp = tempfile::tempdir().expect("temp dir");
    std::fs::write(
        temp.path().join("AGENTS.md"),
        "# Project\n\n## Non-Negotiable Invariants\n\n- The monitor is outside the coding agent loop.\n- Do not continue after source changes when verification is stale.\n\n## Current Working Commands\n\n- cargo test\n",
    )
    .expect("AGENTS.md");
    let store = ProjectStore::open(temp.path()).expect("store");
    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");

    let case_file = build_control_case_file(temp.path(), &snapshot);

    let contract_requirements = case_file
        .requirements
        .iter()
        .filter(|requirement| requirement.source == RequirementSource::ProjectContract)
        .collect::<Vec<_>>();
    assert_eq!(contract_requirements.len(), 2);
    assert!(
        contract_requirements.iter().any(|requirement| requirement
            .text
            .contains("monitor is outside the coding agent")),
        "{contract_requirements:?}"
    );
    assert!(
        contract_requirements
            .iter()
            .all(|requirement| requirement.status == AcceptanceCoverageStatus::Unmapped),
        "{contract_requirements:?}"
    );
    assert!(
        case_file
            .evidence
            .iter()
            .any(|evidence| evidence.kind == "ProjectContract"
                && evidence.source_path.as_deref() == Some("AGENTS.md")),
        "{:?}",
        case_file.evidence
    );
    assert_eq!(
        case_file.completion_certificate.status,
        CompletionCertificateStatus::Blocked
    );
    assert!(
        !case_file
            .completion_certificate
            .unresolved_incidents
            .iter()
            .any(|incident| incident.kind == "requirement_scope"),
        "{:?}",
        case_file.completion_certificate
    );
}

#[test]
fn case_file_completion_certificate_reports_worker_and_test_oracle_gaps() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:00:00Z".into()),
            event_id: Some("evt-certificate-worker-spawn".into()),
            agent: "codex".into(),
            kind: EventKind::ToolCall,
            command: Some("spawn_agent inspect parser".into()),
            content: Some("tool command: spawn_agent inspect parser".into()),
            ..Event::default()
        })
        .expect("spawn event");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:01:00Z".into()),
            event_id: Some("evt-certificate-oracle-change".into()),
            agent: "codex".into(),
            kind: EventKind::FileChange,
            file: Some("tests/parser.rs".into()),
            content: Some("Updated expected value for nested parser result.".into()),
            rationale: Some("match current output".into()),
            ..Event::default()
        })
        .expect("oracle event");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:02:00Z".into()),
            event_id: Some("evt-certificate-completion".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some("Task complete. Ready for review.".into()),
            ..Event::default()
        })
        .expect("completion event");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:03:00Z".into()),
            event_id: Some("evt-certificate-green-tests".into()),
            agent: "codex".into(),
            kind: EventKind::CommandResult,
            command: Some("cargo test".into()),
            exit_code: Some(0),
            ..Event::default()
        })
        .expect("verifier event");

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);
    let certificate = &case_file.completion_certificate;

    assert_eq!(certificate.status, CompletionCertificateStatus::Blocked);
    assert_eq!(certificate.unresolved_workers.len(), 1, "{certificate:?}");
    assert_eq!(certificate.unresolved_workers[0].agent, "codex");
    assert_eq!(certificate.unresolved_workers[0].count, 1);
    assert_eq!(
        certificate.unresolved_workers[0].evidence_id.as_deref(),
        Some("evt-certificate-worker-spawn")
    );
    assert_eq!(certificate.test_oracle_changes.len(), 1, "{certificate:?}");
    assert_eq!(certificate.test_oracle_changes[0].file, "tests/parser.rs");
    assert!(!certificate.test_oracle_changes[0].authorized);
    assert!(
        certificate
            .unresolved_incidents
            .iter()
            .any(|incident| incident.summary.contains("test oracle authority")),
        "{certificate:?}"
    );
    assert!(
        case_file.belief_state.hypotheses.iter().any(|belief| {
            belief.kind == coding_agent_monitor::FailureHypothesisKind::SubagentLifecycleGap
                && belief
                    .evidence_ids
                    .contains(&"evt-certificate-worker-spawn".into())
        }),
        "belief state should surface unresolved worker lifecycle gaps: {:?}",
        case_file.belief_state
    );
    assert!(
        case_file.belief_state.hypotheses.iter().any(|belief| {
            belief.kind == coding_agent_monitor::FailureHypothesisKind::WeakTestOracle
                && belief
                    .evidence_ids
                    .contains(&"evt-certificate-oracle-change".into())
                && belief.missing_evidence.iter().any(|missing| {
                    missing.contains("spec authority")
                        && missing.contains("independent behavior evidence")
                })
        }),
        "belief state should surface test-oracle authority gaps: {:?}",
        case_file.belief_state
    );
}

#[test]
fn case_file_keeps_completion_blocked_when_spawned_subagent_is_unresolved() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:00:00Z".into()),
            event_id: Some("evt-subagent-spawn".into()),
            agent: "codex".into(),
            kind: EventKind::ToolCall,
            command: Some("spawn_agent inspect parser".into()),
            content: Some("tool command: spawn_agent inspect parser".into()),
            ..Event::default()
        })
        .expect("spawn event");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:01:00Z".into()),
            event_id: Some("evt-completion-with-unjoined-worker".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some("Task complete. Ready for review.".into()),
            ..Event::default()
        })
        .expect("completion event");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:02:00Z".into()),
            event_id: Some("evt-verifier-after-unjoined-worker".into()),
            agent: "codex".into(),
            kind: EventKind::CommandResult,
            command: Some("cargo test".into()),
            exit_code: Some(0),
            ..Event::default()
        })
        .expect("verifier event");

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);
    let verification = case_file
        .entropy
        .score(EntropyKind::Verification)
        .expect("verification score");

    assert!(
        verification.score >= 80,
        "passing verifier alone should not close unresolved spawned workers: {verification:?}"
    );
    assert!(
        verification
            .top_causes
            .iter()
            .any(|cause| { cause.contains("completion") && cause.contains("spawned worker") }),
        "verification cause should name the unresolved worker lifecycle: {verification:?}"
    );
    assert!(
        verification
            .evidence_ids
            .contains(&"evt-subagent-spawn".into())
    );
    assert!(
        verification
            .missing_evidence
            .iter()
            .any(|missing| missing.contains("joined_with_summary")
                && missing.contains("cancelled_with_reason")
                && missing.contains("timed_out")),
        "missing evidence should require terminal worker outcomes: {verification:?}"
    );
}

#[test]
fn case_file_allows_completion_after_spawned_subagent_terminal_outcome_and_verifier() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:00:00Z".into()),
            event_id: Some("evt-subagent-spawn-joined".into()),
            agent: "codex".into(),
            kind: EventKind::ToolCall,
            command: Some("spawn_agent inspect parser".into()),
            content: Some("tool command: spawn_agent inspect parser".into()),
            ..Event::default()
        })
        .expect("spawn event");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:01:00Z".into()),
            event_id: Some("evt-subagent-join".into()),
            agent: "codex".into(),
            kind: EventKind::ToolResult,
            command: Some("wait_agent worker-1".into()),
            content: Some("joined_with_summary: inspected parser".into()),
            ..Event::default()
        })
        .expect("join event");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:02:00Z".into()),
            event_id: Some("evt-completion-after-worker-join".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some("Task complete. Ready for review.".into()),
            ..Event::default()
        })
        .expect("completion event");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:03:00Z".into()),
            event_id: Some("evt-verifier-after-worker-join".into()),
            agent: "codex".into(),
            kind: EventKind::CommandResult,
            command: Some("cargo test".into()),
            exit_code: Some(0),
            ..Event::default()
        })
        .expect("verifier event");

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);
    let verification = case_file
        .entropy
        .score(EntropyKind::Verification)
        .expect("verification score");

    assert!(
        verification.score < 80,
        "terminal worker outcome plus verifier should avoid completion block: {verification:?}"
    );
    assert!(
        !verification
            .top_causes
            .iter()
            .any(|cause| cause.contains("spawned worker")),
        "{verification:?}"
    );
}

#[test]
fn case_file_flags_overlapping_subagent_path_ownership() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:00:00Z".into()),
            event_id: Some("evt-overlap-worker-1".into()),
            agent: "codex".into(),
            kind: EventKind::ToolCall,
            command: Some("spawn_agent inspect parser path=src/parser.rs".into()),
            content: Some("worker started for src/parser.rs".into()),
            ..Event::default()
        })
        .expect("first worker");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:01:00Z".into()),
            event_id: Some("evt-overlap-worker-2".into()),
            agent: "codex".into(),
            kind: EventKind::ToolCall,
            command: Some("spawn_agent repair parser --file src/parser.rs".into()),
            content: Some("worker started for src/parser.rs".into()),
            ..Event::default()
        })
        .expect("second worker");

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);
    let plan = case_file
        .entropy
        .score(EntropyKind::Plan)
        .expect("plan score");
    let health = case_file
        .entropy
        .score(EntropyKind::AgentHealth)
        .expect("agent-health score");

    assert!(
        plan.score >= 72,
        "overlapping worker paths should raise plan entropy: {plan:?}"
    );
    assert!(
        health.score >= 70,
        "overlapping worker paths should raise agent-health entropy: {health:?}"
    );
    assert!(
        plan.top_causes
            .iter()
            .any(|cause| cause.contains("overlapping subagent path ownership")),
        "{plan:?}"
    );
    assert!(
        plan.evidence_ids.contains(&"evt-overlap-worker-2".into()),
        "{plan:?}"
    );
    assert!(
        plan.missing_evidence
            .iter()
            .any(|missing| missing.contains("disjoint worker path ownership")),
        "{plan:?}"
    );
}

#[test]
fn follow_up_packet_names_overlapping_subagent_path_ownership() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:00:00Z".into()),
            event_id: Some("evt-overlap-packet-worker-1".into()),
            agent: "codex".into(),
            kind: EventKind::ToolCall,
            command: Some("spawn_agent inspect parser path=src/parser.rs".into()),
            content: Some("worker started for src/parser.rs".into()),
            ..Event::default()
        })
        .expect("first worker");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:01:00Z".into()),
            event_id: Some("evt-overlap-packet-worker-2".into()),
            agent: "codex".into(),
            kind: EventKind::ToolCall,
            command: Some("spawn_agent repair parser --file src/parser.rs".into()),
            content: Some("worker started for src/parser.rs".into()),
            ..Event::default()
        })
        .expect("second worker");

    let advice = advise_workspace(temp.path()).expect("advice");

    assert_eq!(
        advice.final_action,
        ControlAction::SendFollowUp { target_agent: None }
    );
    assert!(
        advice.packet.instructions.iter().any(|instruction| {
            instruction
                .text
                .contains("overlapping subagent path ownership")
        }),
        "packet should name overlapping ownership: {:?}",
        advice.packet
    );
    assert!(
        advice
            .packet
            .instructions
            .iter()
            .any(|instruction| instruction.text.contains("disjoint worker paths")),
        "packet should require disjoint paths: {:?}",
        advice.packet
    );
}

#[test]
fn force_verification_packet_names_unresolved_spawned_workers() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:00:00Z".into()),
            event_id: Some("evt-packet-subagent-spawn".into()),
            agent: "codex".into(),
            kind: EventKind::ToolCall,
            command: Some("spawn_agent inspect parser".into()),
            content: Some("tool command: spawn_agent inspect parser".into()),
            ..Event::default()
        })
        .expect("spawn event");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:01:00Z".into()),
            event_id: Some("evt-packet-subagent-completion".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some("Task complete. Ready for review.".into()),
            ..Event::default()
        })
        .expect("completion event");

    let advice = advise_workspace(temp.path()).expect("advice");

    assert_eq!(
        advice.final_action,
        ControlAction::ForceVerification {
            suite: VerificationSuite::Full,
            blocking: true,
        }
    );
    assert!(
        advice.packet.instructions.iter().any(|instruction| {
            instruction.text.contains("spawned workers")
                && instruction.text.contains("joined_with_summary")
        }),
        "packet should name worker lifecycle closure: {:?}",
        advice.packet
    );
}

#[test]
fn case_file_flags_subagent_wip_cap_before_completion_claim() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    for index in 1..=4 {
        store
            .append_event(&Event {
                time: Some(format!("2026-06-22T12:0{index}:00Z")),
                event_id: Some(format!("evt-wip-subagent-{index}")),
                agent: "codex".into(),
                kind: EventKind::ToolCall,
                command: Some(format!("spawn_agent worker-{index}")),
                content: Some(format!("tool command: spawn_agent worker-{index}")),
                ..Event::default()
            })
            .expect("spawn event");
    }

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);
    let plan = case_file
        .entropy
        .score(EntropyKind::Plan)
        .expect("plan score");
    let health = case_file
        .entropy
        .score(EntropyKind::AgentHealth)
        .expect("agent health score");

    assert!(
        plan.score >= 70,
        "unresolved worker fan-out should raise plan entropy: {plan:?}"
    );
    assert!(
        health.score >= 70,
        "unresolved worker fan-out should raise agent-health entropy: {health:?}"
    );
    assert!(
        plan.top_causes
            .iter()
            .any(|cause| cause.contains("subagent WIP cap")),
        "{plan:?}"
    );
    assert!(plan.evidence_ids.contains(&"evt-wip-subagent-4".into()));
    assert!(
        plan.missing_evidence
            .iter()
            .any(|missing| missing.contains("join or cancel spawned workers")),
        "{plan:?}"
    );
}

#[test]
fn follow_up_packet_names_subagent_wip_cap() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    for index in 1..=4 {
        store
            .append_event(&Event {
                time: Some(format!("2026-06-22T12:0{index}:00Z")),
                event_id: Some(format!("evt-packet-wip-subagent-{index}")),
                agent: "codex".into(),
                kind: EventKind::ToolCall,
                command: Some(format!("spawn_agent worker-{index}")),
                content: Some(format!("tool command: spawn_agent worker-{index}")),
                ..Event::default()
            })
            .expect("spawn event");
    }

    let advice = advise_workspace(temp.path()).expect("advice");

    assert_eq!(
        advice.final_action,
        ControlAction::SendFollowUp { target_agent: None }
    );
    assert!(
        advice
            .packet
            .instructions
            .iter()
            .any(|instruction| instruction.text.contains("subagent WIP cap")),
        "packet should name subagent WIP cap: {:?}",
        advice.packet.instructions
    );
    assert!(
        advice
            .packet
            .instructions
            .iter()
            .any(|instruction| instruction.text.contains("Join or cancel")),
        "packet should name subagent WIP cap: {:?}",
        advice.packet.instructions
    );
}

#[test]
fn advise_workspace_forces_verification_after_unverified_completion_claim() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:00:00Z".into()),
            event_id: Some("evt-unverified-completion-claim".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some("Implementation complete. Ready for review.".into()),
            ..Event::default()
        })
        .expect("event");

    let advice = advise_workspace(temp.path()).expect("advice");

    assert_eq!(
        advice.final_action,
        ControlAction::ForceVerification {
            suite: VerificationSuite::Full,
            blocking: true,
        }
    );
    assert!(
        advice.packet.summary.contains("completion claim")
            || advice.packet.instructions.iter().any(|instruction| {
                instruction.text.contains("completion")
                    || instruction.text.contains("claiming completion")
            }),
        "packet should explain completion verification: {:?}",
        advice.packet
    );
    assert!(
        advice
            .control_rationale
            .evidence_ids
            .contains(&"evt-unverified-completion-claim".to_string())
    );
}

#[test]
fn case_file_allows_docs_only_change_without_stale_verification_by_default() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:00:00Z".into()),
            event_id: Some("evt-test-pass".into()),
            agent: "codex".into(),
            kind: EventKind::CommandResult,
            command: Some("cargo test".into()),
            exit_code: Some(0),
            ..Event::default()
        })
        .expect("event");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:01:00Z".into()),
            event_id: Some("evt-docs-write".into()),
            agent: "codex".into(),
            kind: EventKind::FileChange,
            file: Some("docs/reference.yaml".into()),
            rationale: Some("Document monitor configuration.".into()),
            ..Event::default()
        })
        .expect("event");

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);

    assert_eq!(
        case_file.verification.status,
        coding_agent_monitor::VerificationStatus::Passed
    );
    assert!(case_file.verification.changed_source_files.is_empty());
    let verification = case_file
        .entropy
        .score(EntropyKind::Verification)
        .expect("verification score");
    assert!(
        verification.score < 75,
        "docs-only changes should not force verification by default: {verification:?}"
    );
}

#[test]
fn case_file_requires_verification_for_docs_only_change_when_policy_disallows_exemption() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:00:00Z".into()),
            event_id: Some("evt-test-pass".into()),
            agent: "codex".into(),
            kind: EventKind::CommandResult,
            command: Some("cargo test".into()),
            exit_code: Some(0),
            ..Event::default()
        })
        .expect("event");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:01:00Z".into()),
            event_id: Some("evt-docs-write".into()),
            agent: "codex".into(),
            kind: EventKind::FileChange,
            file: Some("docs/operator-guide.md".into()),
            rationale: Some("Document monitor operation.".into()),
            ..Event::default()
        })
        .expect("event");

    let mut config = ProjectConfig::default();
    config.policy.allow_docs_only_continue_without_tests = false;
    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file_with_config(temp.path(), &snapshot, &config);
    let verification = case_file
        .entropy
        .score(EntropyKind::Verification)
        .expect("verification score");

    assert_eq!(
        case_file.verification.status,
        coding_agent_monitor::VerificationStatus::Stale
    );
    assert!(
        verification.score >= 75,
        "policy should require verification for docs-only changes: {verification:?}"
    );
    assert_eq!(
        case_file.verification.changed_source_files,
        vec!["docs/operator-guide.md".to_string()]
    );
}

#[test]
fn case_file_requires_browser_validation_for_ui_change_even_after_build_passes() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:00:00Z".into()),
            event_id: Some("evt-ui-write".into()),
            agent: "codex".into(),
            kind: EventKind::FileChange,
            file: Some("frontend/src/App.vue".into()),
            rationale: Some("Update route interaction.".into()),
            ..Event::default()
        })
        .expect("ui change");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:01:00Z".into()),
            event_id: Some("evt-ui-build".into()),
            agent: "codex".into(),
            kind: EventKind::CommandResult,
            command: Some("npm run build".into()),
            exit_code: Some(0),
            ..Event::default()
        })
        .expect("build");

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);
    let verification = case_file
        .entropy
        .score(EntropyKind::Verification)
        .expect("verification score");

    assert!(
        verification.score >= 80,
        "build pass should not close intended-environment validation obligation: {verification:?}"
    );
    assert!(
        verification
            .top_causes
            .iter()
            .any(|cause| cause.contains("web UI change")
                && cause.contains("intended-environment validation")),
        "verification cause should name web intended-environment validation: {verification:?}"
    );
    assert!(
        verification
            .missing_evidence
            .iter()
            .any(|missing| missing.contains("browser or Playwright validation")),
        "missing evidence should name web validation executor family: {verification:?}"
    );
    assert!(verification.evidence_ids.contains(&"evt-ui-write".into()));
}

#[test]
fn case_file_accepts_browser_validation_after_ui_change() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:00:00Z".into()),
            event_id: Some("evt-ui-write-validated".into()),
            agent: "codex".into(),
            kind: EventKind::FileChange,
            file: Some("frontend/src/App.vue".into()),
            rationale: Some("Update route interaction.".into()),
            ..Event::default()
        })
        .expect("ui change");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:01:00Z".into()),
            event_id: Some("evt-ui-browser-pass".into()),
            agent: "codex".into(),
            kind: EventKind::CommandResult,
            command: Some("npx playwright test frontend/app.spec.ts".into()),
            exit_code: Some(0),
            ..Event::default()
        })
        .expect("browser validation");

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);
    let verification = case_file
        .entropy
        .score(EntropyKind::Verification)
        .expect("verification score");

    assert!(
        verification.score < 80,
        "browser validation after the UI change should satisfy the UI obligation: {verification:?}"
    );
    assert!(
        !verification
            .top_causes
            .iter()
            .any(|cause| cause.contains("web UI change")
                && cause.contains("intended-environment validation")),
        "{verification:?}"
    );
}

#[test]
fn force_verification_packet_names_browser_validation_for_ui_change() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:00:00Z".into()),
            event_id: Some("evt-ui-packet-write".into()),
            agent: "codex".into(),
            kind: EventKind::FileChange,
            file: Some("frontend/src/App.vue".into()),
            rationale: Some("Update route interaction.".into()),
            ..Event::default()
        })
        .expect("ui change");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:01:00Z".into()),
            event_id: Some("evt-ui-packet-build".into()),
            agent: "codex".into(),
            kind: EventKind::CommandResult,
            command: Some("npm run build".into()),
            exit_code: Some(0),
            ..Event::default()
        })
        .expect("build");

    let advice = advise_workspace(temp.path()).expect("advice");

    assert_eq!(
        advice.final_action,
        ControlAction::ForceVerification {
            suite: VerificationSuite::Full,
            blocking: true,
        }
    );
    assert!(
        advice.packet.instructions.iter().any(|instruction| {
            instruction.text.contains("intended-environment validation")
                && instruction.text.contains("browser/Playwright")
                && instruction.text.contains("web UI")
                && !instruction.text.contains("simulator/device")
                && !instruction.text.contains("ML eval")
        }),
        "packet should name web intended-environment validation without unrelated surfaces: {:?}",
        advice.packet
    );
}

#[test]
fn force_verification_packet_names_mobile_validation_for_mobile_change() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:00:00Z".into()),
            event_id: Some("evt-mobile-packet-write".into()),
            agent: "codex".into(),
            kind: EventKind::FileChange,
            file: Some("mobile/app/src/MainActivity.kt".into()),
            rationale: Some("Update mobile runtime behavior.".into()),
            ..Event::default()
        })
        .expect("mobile change");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:01:00Z".into()),
            event_id: Some("evt-mobile-packet-build".into()),
            agent: "codex".into(),
            kind: EventKind::CommandResult,
            command: Some("gradle build".into()),
            exit_code: Some(0),
            ..Event::default()
        })
        .expect("build");

    let advice = advise_workspace(temp.path()).expect("advice");

    assert_eq!(
        advice.final_action,
        ControlAction::ForceVerification {
            suite: VerificationSuite::Full,
            blocking: true,
        }
    );
    assert!(
        advice.packet.instructions.iter().any(|instruction| {
            instruction.text.contains("intended-environment validation")
                && instruction.text.contains("simulator/device")
                && instruction.text.contains("mobile app")
                && !instruction.text.contains("browser/Playwright")
                && !instruction.text.contains("ML eval")
        }),
        "packet should name mobile intended-environment validation without unrelated surfaces: {:?}",
        advice.packet
    );
}

#[test]
fn force_verification_packet_names_non_browser_runtime_validation_surfaces() {
    let cases = [
        (
            "src-tauri/src/main.rs",
            "cargo build",
            "native GUI",
            "native GUI smoke, e2e, or screenshot evidence",
        ),
        (
            "system/daemon.rs",
            "cargo build",
            "system component",
            "service, integration, healthcheck, or daemon smoke evidence",
        ),
        (
            "ml/inference.py",
            "pytest",
            "ML system",
            "model evaluation, benchmark, golden-data, or inference smoke evidence",
        ),
    ];

    for (path, command, expected_surface, expected_evidence) in cases {
        let temp = tempfile::tempdir().expect("temp dir");
        let mut store = ProjectStore::open(temp.path()).expect("store");
        store
            .append_event(&Event {
                time: Some("2026-06-22T12:00:00Z".into()),
                event_id: Some(
                    format!("evt-packet-runtime-write-{path}").replace(['/', '\\'], "-"),
                ),
                agent: "codex".into(),
                kind: EventKind::FileChange,
                file: Some(path.into()),
                rationale: Some("Update runtime-facing behavior.".into()),
                ..Event::default()
            })
            .expect("runtime change");
        store
            .append_event(&Event {
                time: Some("2026-06-22T12:01:00Z".into()),
                event_id: Some(
                    format!("evt-packet-runtime-build-{path}").replace(['/', '\\'], "-"),
                ),
                agent: "codex".into(),
                kind: EventKind::CommandResult,
                command: Some(command.into()),
                exit_code: Some(0),
                ..Event::default()
            })
            .expect("build or test");

        let advice = advise_workspace(temp.path()).expect("advice");

        assert_eq!(
            advice.final_action,
            ControlAction::ForceVerification {
                suite: VerificationSuite::Full,
                blocking: true,
            }
        );
        assert!(
            advice.packet.instructions.iter().any(|instruction| {
                instruction.text.contains("intended-environment validation")
                    && instruction.text.contains(expected_surface)
                    && instruction.text.contains(expected_evidence)
                    && !instruction.text.contains("browser/Playwright")
                    && !instruction.text.contains("simulator/device")
            }),
            "packet should name {expected_surface} validation without browser/mobile bias: {:?}",
            advice.packet
        );
    }
}

#[test]
fn case_file_requires_intended_environment_validation_for_non_web_domains_after_build_or_tests() {
    let cases = [
        (
            "mobile/app/src/MainActivity.kt",
            "gradle build",
            "mobile app change",
            "simulator/device validation",
        ),
        (
            "src-tauri/src/main.rs",
            "cargo build",
            "native GUI change",
            "native GUI smoke/e2e validation",
        ),
        (
            "system/daemon.rs",
            "cargo build",
            "system component change",
            "service or integration validation",
        ),
        (
            "ml/inference.py",
            "pytest",
            "ML system change",
            "model evaluation or benchmark validation",
        ),
    ];

    for (path, command, expected_cause, expected_missing) in cases {
        let temp = tempfile::tempdir().expect("temp dir");
        let mut store = ProjectStore::open(temp.path()).expect("store");
        store
            .append_event(&Event {
                time: Some("2026-06-22T12:00:00Z".into()),
                event_id: Some(format!("evt-domain-write-{path}").replace(['/', '\\'], "-")),
                agent: "codex".into(),
                kind: EventKind::FileChange,
                file: Some(path.into()),
                rationale: Some("Update runtime behavior.".into()),
                ..Event::default()
            })
            .expect("domain change");
        store
            .append_event(&Event {
                time: Some("2026-06-22T12:01:00Z".into()),
                event_id: Some(format!("evt-domain-build-{path}").replace(['/', '\\'], "-")),
                agent: "codex".into(),
                kind: EventKind::CommandResult,
                command: Some(command.into()),
                exit_code: Some(0),
                ..Event::default()
            })
            .expect("build or test");

        let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
        let case_file = build_control_case_file(temp.path(), &snapshot);
        let verification = case_file
            .entropy
            .score(EntropyKind::Verification)
            .expect("verification score");

        assert!(
            verification.score >= 80,
            "build/test pass should not close intended-environment validation for {path}: {verification:?}"
        );
        assert!(
            verification
                .top_causes
                .iter()
                .any(|cause| cause.contains(expected_cause)
                    && cause.contains("intended-environment validation")),
            "verification cause should name the domain validation gap for {path}: {verification:?}"
        );
        assert!(
            verification
                .missing_evidence
                .iter()
                .any(|missing| missing.contains(expected_missing)),
            "missing evidence should name the right validation executor family for {path}: {verification:?}"
        );
    }
}

#[test]
fn case_file_accepts_mobile_simulator_validation_after_mobile_change() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:00:00Z".into()),
            event_id: Some("evt-mobile-write-validated".into()),
            agent: "codex".into(),
            kind: EventKind::FileChange,
            file: Some("mobile/app/src/MainActivity.kt".into()),
            rationale: Some("Update mobile runtime behavior.".into()),
            ..Event::default()
        })
        .expect("mobile change");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:01:00Z".into()),
            event_id: Some("evt-mobile-simulator-pass".into()),
            agent: "codex".into(),
            kind: EventKind::CommandResult,
            command: Some("maestro test flows/login.yaml --device emulator".into()),
            exit_code: Some(0),
            ..Event::default()
        })
        .expect("mobile validation");

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);
    let verification = case_file
        .entropy
        .score(EntropyKind::Verification)
        .expect("verification score");

    assert!(
        !verification.top_causes.iter().any(|cause| {
            cause.contains("mobile app change") && cause.contains("intended-environment validation")
        }),
        "mobile simulator validation after the mobile change should satisfy the validation obligation: {verification:?}"
    );
}

#[test]
fn case_file_accepts_non_web_intended_environment_validation_after_matching_change() {
    let cases = [
        (
            "src-tauri/src/main.rs",
            "cargo test desktop gui smoke",
            "native GUI change",
        ),
        (
            "system/daemon.rs",
            "docker compose run service healthcheck smoke",
            "system component change",
        ),
        (
            "ml/inference.py",
            "python -m eval benchmark --golden",
            "ML system change",
        ),
    ];

    for (path, command, expected_cause) in cases {
        let temp = tempfile::tempdir().expect("temp dir");
        let mut store = ProjectStore::open(temp.path()).expect("store");
        store
            .append_event(&Event {
                time: Some("2026-06-22T12:00:00Z".into()),
                event_id: Some(
                    format!("evt-domain-write-validated-{path}").replace(['/', '\\'], "-"),
                ),
                agent: "codex".into(),
                kind: EventKind::FileChange,
                file: Some(path.into()),
                rationale: Some("Update runtime-facing behavior.".into()),
                ..Event::default()
            })
            .expect("domain change");
        store
            .append_event(&Event {
                time: Some("2026-06-22T12:01:00Z".into()),
                event_id: Some(format!("evt-domain-validation-{path}").replace(['/', '\\'], "-")),
                agent: "codex".into(),
                kind: EventKind::CommandResult,
                command: Some(command.into()),
                exit_code: Some(0),
                ..Event::default()
            })
            .expect("domain validation");

        let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
        let case_file = build_control_case_file(temp.path(), &snapshot);
        let verification = case_file
            .entropy
            .score(EntropyKind::Verification)
            .expect("verification score");

        assert!(
            !verification.top_causes.iter().any(|cause| {
                cause.contains(expected_cause) && cause.contains("intended-environment validation")
            }),
            "matching domain validation should satisfy {path}: {verification:?}"
        );
    }
}

#[test]
fn case_file_requires_authority_for_test_oracle_change_even_after_tests_pass() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:00:00Z".into()),
            event_id: Some("evt-oracle-change".into()),
            agent: "codex".into(),
            kind: EventKind::FileChange,
            file: Some("tests/parser_tests.rs".into()),
            rationale: Some("Update expected value to match implementation output.".into()),
            ..Event::default()
        })
        .expect("oracle change");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:01:00Z".into()),
            event_id: Some("evt-oracle-green-tests".into()),
            agent: "codex".into(),
            kind: EventKind::CommandResult,
            command: Some("cargo test".into()),
            exit_code: Some(0),
            ..Event::default()
        })
        .expect("test pass");

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);
    let verification = case_file
        .entropy
        .score(EntropyKind::Verification)
        .expect("verification score");

    assert!(
        verification.score >= 80,
        "green tests should not close an unauthorised oracle change: {verification:?}"
    );
    assert!(
        verification
            .top_causes
            .iter()
            .any(|cause| cause.contains("test oracle") && cause.contains("authority")),
        "verification cause should name test-oracle authority: {verification:?}"
    );
    assert!(
        verification
            .evidence_ids
            .contains(&"evt-oracle-change".into())
    );
}

#[test]
fn case_file_accepts_test_oracle_change_with_authority_and_passing_tests() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:00:00Z".into()),
            event_id: Some("evt-authorized-oracle-change".into()),
            agent: "codex".into(),
            kind: EventKind::FileChange,
            file: Some("tests/parser_tests.rs".into()),
            rationale: Some(
                "Update expected value for user-authorized requirement: parser now normalizes IDs."
                    .into(),
            ),
            ..Event::default()
        })
        .expect("oracle change");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:01:00Z".into()),
            event_id: Some("evt-authorized-oracle-green-tests".into()),
            agent: "codex".into(),
            kind: EventKind::CommandResult,
            command: Some("cargo test".into()),
            exit_code: Some(0),
            ..Event::default()
        })
        .expect("test pass");

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);
    let verification = case_file
        .entropy
        .score(EntropyKind::Verification)
        .expect("verification score");

    assert!(
        verification.score < 80,
        "authorized oracle change with fresh tests should avoid oracle block: {verification:?}"
    );
    assert!(
        !verification
            .top_causes
            .iter()
            .any(|cause| cause.contains("test oracle") && cause.contains("authority")),
        "{verification:?}"
    );
}

#[test]
fn force_verification_packet_names_test_oracle_authority_gap() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:00:00Z".into()),
            event_id: Some("evt-oracle-packet-change".into()),
            agent: "codex".into(),
            kind: EventKind::FileChange,
            file: Some("tests/parser_tests.rs".into()),
            rationale: Some("Refresh snapshot to match current output.".into()),
            ..Event::default()
        })
        .expect("oracle change");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:01:00Z".into()),
            event_id: Some("evt-oracle-packet-green-tests".into()),
            agent: "codex".into(),
            kind: EventKind::CommandResult,
            command: Some("cargo test".into()),
            exit_code: Some(0),
            ..Event::default()
        })
        .expect("test pass");

    let advice = advise_workspace(temp.path()).expect("advice");

    assert_eq!(
        advice.final_action,
        ControlAction::ForceVerification {
            suite: VerificationSuite::Full,
            blocking: true,
        }
    );
    assert!(
        advice.packet.instructions.iter().any(|instruction| {
            instruction.text.contains("test oracle")
                && instruction.text.contains("authority")
                && instruction.text.contains("independent")
        }),
        "packet should name test-oracle authority gap: {:?}",
        advice.packet
    );
}

#[test]
fn case_file_marks_untimestamped_source_change_as_stale_in_status_and_entropy() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:00:00Z".into()),
            event_id: Some("evt-test-pass".into()),
            agent: "codex".into(),
            kind: EventKind::CommandResult,
            command: Some("cargo test".into()),
            exit_code: Some(0),
            ..Event::default()
        })
        .expect("event");
    store
        .append_event(&Event {
            event_id: Some("evt-untimestamped-write".into()),
            agent: "codex".into(),
            kind: EventKind::FileChange,
            file: Some("src/lib.rs".into()),
            rationale: Some("Captured by a legacy adapter without event time.".into()),
            ..Event::default()
        })
        .expect("event");

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);
    let verification = case_file
        .entropy
        .score(EntropyKind::Verification)
        .expect("verification score");

    assert_eq!(
        case_file.verification.status,
        coding_agent_monitor::VerificationStatus::Stale
    );
    assert!(verification.score >= 75, "{verification:?}");
    assert!(
        verification
            .evidence_ids
            .contains(&"evt-untimestamped-write".to_string())
    );
}

#[test]
fn case_file_does_not_mask_later_untimestamped_source_change_after_fresh_verification() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:00:00Z".into()),
            event_id: Some("evt-timestamped-write".into()),
            agent: "codex".into(),
            kind: EventKind::FileChange,
            file: Some("src/lib.rs".into()),
            rationale: Some("Initial source edit.".into()),
            ..Event::default()
        })
        .expect("event");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:01:00Z".into()),
            event_id: Some("evt-test-pass".into()),
            agent: "codex".into(),
            kind: EventKind::CommandResult,
            command: Some("cargo test".into()),
            exit_code: Some(0),
            ..Event::default()
        })
        .expect("event");
    store
        .append_event(&Event {
            event_id: Some("evt-later-untimestamped-write".into()),
            agent: "codex".into(),
            kind: EventKind::FileChange,
            file: Some("src/lib.rs".into()),
            rationale: Some("Legacy adapter write after verification.".into()),
            ..Event::default()
        })
        .expect("event");

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);
    let verification = case_file
        .entropy
        .score(EntropyKind::Verification)
        .expect("verification score");

    assert_eq!(
        case_file.verification.status,
        coding_agent_monitor::VerificationStatus::Stale
    );
    assert!(verification.score >= 75, "{verification:?}");
    assert!(
        verification
            .evidence_ids
            .contains(&"evt-later-untimestamped-write".to_string())
    );
}

#[test]
fn case_file_raises_repo_blame_entropy_for_untraced_dirty_hunk() {
    let temp = tempfile::tempdir().expect("temp dir");
    init_git_repo(temp.path());
    std::fs::create_dir_all(temp.path().join("src")).expect("src dir");
    std::fs::write(temp.path().join("src/lib.rs"), "fn one() {}\nfn two() {}\n").expect("source");
    run_git(temp.path(), ["add", "src/lib.rs"]);
    run_git(temp.path(), ["commit", "-m", "add source"]);
    std::fs::write(
        temp.path().join("src/lib.rs"),
        "fn one() {}\nfn two_changed() {}\n",
    )
    .expect("changed source");
    let store = ProjectStore::open(temp.path()).expect("store");
    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");

    let case_file = build_control_case_file(temp.path(), &snapshot);
    let repo_blame = case_file
        .entropy
        .score(EntropyKind::RepoBlame)
        .expect("repo blame score");
    let verification = case_file
        .entropy
        .score(EntropyKind::Verification)
        .expect("verification score");

    let repo_audit = case_file.repo_audit.as_ref().expect("repo audit");
    assert_eq!(repo_audit.status, RepoAuditStatus::Warning);
    assert_eq!(repo_audit.untraced_count, 1);
    assert!(repo_blame.score >= 85, "{repo_blame:?}");
    assert!(
        verification.score >= 75,
        "dirty source hunk should require verification even without file_change event: {verification:?}"
    );
    assert!(
        verification
            .top_causes
            .iter()
            .any(|cause| cause.contains("dirty source/test git hunks"))
    );
    assert!(
        repo_blame
            .top_causes
            .iter()
            .any(|cause| cause.contains("dirty git hunks lack trace evidence"))
    );
    assert!(
        case_file
            .evidence
            .iter()
            .any(|item| item.kind == "repo_audit" && item.summary.contains("src/lib.rs"))
    );
}

#[test]
fn repo_diff_without_rationale_raises_repo_blame_entropy() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            event_id: Some("evt-repo-diff".into()),
            agent: "codex".into(),
            kind: EventKind::RepoDiff,
            file: Some("src/lib.rs".into()),
            ..Event::default()
        })
        .expect("event");

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);
    let repo_blame = case_file
        .entropy
        .score(EntropyKind::RepoBlame)
        .expect("repo blame score");

    assert!(repo_blame.score >= 75, "{repo_blame:?}");
    assert!(repo_blame.evidence_ids.contains(&"evt-repo-diff".into()));
    assert!(
        repo_blame
            .top_causes
            .iter()
            .any(|cause| cause.contains("file change lacks rationale"))
    );
}

#[test]
fn policy_validator_replaces_progress_with_trace_and_verification_block() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            event_id: Some("evt-untraced-doc-diff".into()),
            agent: "codex".into(),
            kind: EventKind::RepoDiff,
            file: Some("docs/notes.md".into()),
            ..Event::default()
        })
        .expect("event");
    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let mut case_file = build_control_case_file(temp.path(), &snapshot);
    for score in &mut case_file.entropy.scores {
        if score.kind == EntropyKind::Verification {
            score.score = 0;
            score.top_causes.clear();
            score.evidence_ids.clear();
        }
    }

    let outcome = validate_control_action_detailed(
        ControlAction::SendFollowUp { target_agent: None },
        &case_file,
    );

    match outcome {
        ValidationOutcome::Modified {
            original,
            replacement,
            reason,
        } => {
            assert_eq!(original, ControlAction::SendFollowUp { target_agent: None });
            assert!(matches!(
                replacement,
                ControlAction::BlockProgressUntilTraceAndVerification { .. }
            ));
            assert!(reason.contains("trace/repo-blame entropy"));
        }
        other => panic!("expected trace/verification block, got {other:?}"),
    }
}

#[test]
fn advise_workspace_blocks_progress_for_unrationalized_doc_diff() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            event_id: Some("evt-doc-diff-without-rationale".into()),
            agent: "codex".into(),
            kind: EventKind::RepoDiff,
            file: Some("docs/notes.md".into()),
            ..Event::default()
        })
        .expect("event");
    std::fs::write(
        store.root().join("config.json"),
        json!({
            "adapters": {
                "codex": { "supports_readonly_mode": false },
                "claude_code": { "supports_readonly_mode": false },
                "opencode": { "supports_readonly_mode": false }
            }
        })
        .to_string(),
    )
    .expect("config");

    let advice = advise_workspace(temp.path()).expect("advice");

    assert!(matches!(
        advice.final_action,
        ControlAction::BlockProgressUntilTraceAndVerification { .. }
    ));
    assert_eq!(
        advice.control_rationale.selected_action,
        ControlActionKind::BlockProgressUntilTraceAndVerification
    );
    assert!(
        advice
            .packet
            .summary
            .contains("trace rationale and verification evidence")
    );
    assert!(
        advice
            .packet
            .instructions
            .iter()
            .any(|instruction| instruction.text.contains("trace rationale"))
    );
    assert!(
        advice
            .packet
            .instructions
            .iter()
            .any(|instruction| instruction.text.contains("verifier"))
    );

    let trails = load_decision_trails(store.root()).expect("decision trails");
    let outcome = trails
        .first()
        .and_then(|trail| trail.outcomes.first())
        .expect("block progress outcome");
    assert_eq!(
        outcome.action,
        ControlActionKind::BlockProgressUntilTraceAndVerification
    );
    assert_eq!(outcome.status, OutcomeStatus::Succeeded);
}

#[test]
fn advise_workspace_forces_verification_for_untraced_source_repo_audit() {
    let temp = tempfile::tempdir().expect("temp dir");
    init_git_repo(temp.path());
    std::fs::create_dir_all(temp.path().join("src")).expect("src dir");
    std::fs::write(temp.path().join("src/lib.rs"), "fn one() {}\nfn two() {}\n").expect("source");
    run_git(temp.path(), ["add", "src/lib.rs"]);
    run_git(temp.path(), ["commit", "-m", "add source"]);
    std::fs::write(
        temp.path().join("src/lib.rs"),
        "fn one() {}\nfn two_changed() {}\n",
    )
    .expect("changed source");
    ProjectStore::open(temp.path()).expect("store");

    let advice = advise_workspace(temp.path()).expect("advice");

    assert_eq!(
        advice.final_action,
        ControlAction::ForceVerification {
            suite: VerificationSuite::Full,
            blocking: true,
        }
    );
    assert!(
        advice
            .packet
            .instructions
            .iter()
            .any(|instruction| instruction.text.contains("trace rationale"))
    );
}

#[test]
fn advise_workspace_spawns_judge_agent_when_dirty_source_hunk_has_fresh_verifier() {
    let temp = tempfile::tempdir().expect("temp dir");
    init_git_repo(temp.path());
    std::fs::create_dir_all(temp.path().join("src")).expect("src dir");
    std::fs::write(temp.path().join("src/lib.rs"), "fn one() {}\nfn two() {}\n").expect("source");
    run_git(temp.path(), ["add", "src/lib.rs"]);
    run_git(temp.path(), ["commit", "-m", "add source"]);
    std::fs::write(
        temp.path().join("src/lib.rs"),
        "fn one() {}\nfn two_changed() {}\n",
    )
    .expect("changed source");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_verifier_run(&coding_agent_monitor::VerifierRun {
            verifier_run_id: "verifier-run-after-dirty-source".into(),
            verifier_id: Some("rust_full".into()),
            command: "cargo test".into(),
            status: VerificationRunStatus::Passed,
            started_at: "9999-01-01T00:00:00Z".into(),
            completed_at: Some("9999-01-01T00:00:01Z".into()),
            exit_code: Some(0),
            output_digest: "fnv1a64:cbf29ce484222325".into(),
            failure_class: None,
        })
        .expect("verifier run");

    let advice = advise_workspace(temp.path()).expect("advice");

    assert_eq!(
        advice.final_action,
        ControlAction::SpawnJudgeAgent {
            target_agent: Some("claude-code".into())
        }
    );
    assert!(
        advice
            .packet
            .instructions
            .iter()
            .any(|instruction| instruction.text.contains("read-only judge"))
    );
    assert!(
        advice
            .packet
            .forbidden
            .iter()
            .any(|forbidden| forbidden.contains("Do not edit files"))
    );
}

#[test]
fn same_second_repo_audit_write_and_verifier_is_conservatively_stale() {
    let temp = tempfile::tempdir().expect("temp dir");
    init_git_repo(temp.path());
    std::fs::create_dir_all(temp.path().join("src")).expect("src dir");
    let source_path = temp.path().join("src/lib.rs");
    std::fs::write(&source_path, "fn one() {}\nfn two() {}\n").expect("source");
    run_git(temp.path(), ["add", "src/lib.rs"]);
    run_git(temp.path(), ["commit", "-m", "add source"]);
    std::fs::write(&source_path, "fn one() {}\nfn two_changed() {}\n").expect("changed source");
    let same_second = file_modified_utc_timestamp(&source_path);
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_verifier_run(&coding_agent_monitor::VerifierRun {
            verifier_run_id: "verifier-run-same-second-dirty-source".into(),
            verifier_id: Some("rust_full".into()),
            command: "cargo test".into(),
            status: VerificationRunStatus::Passed,
            started_at: same_second.clone(),
            completed_at: Some(same_second),
            exit_code: Some(0),
            output_digest: "fnv1a64:cbf29ce484222325".into(),
            failure_class: None,
        })
        .expect("verifier run");

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);
    let verification = case_file
        .entropy
        .score(EntropyKind::Verification)
        .expect("verification score");

    assert_eq!(
        case_file.verification.status,
        coding_agent_monitor::VerificationStatus::Stale
    );
    assert!(
        verification.score >= 75,
        "same-second dirty git hunk and verifier should fail closed: {verification:?}"
    );
    assert!(
        verification
            .evidence_ids
            .contains(&"repo-audit-src-lib-rs".to_string())
    );
}

#[test]
fn case_file_treats_passing_verifier_run_as_fresh_verification() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:00:00Z".into()),
            event_id: Some("evt-write".into()),
            agent: "codex".into(),
            kind: EventKind::FileChange,
            file: Some("src/lib.rs".into()),
            rationale: Some("Implement verifier integration.".into()),
            ..Event::default()
        })
        .expect("event");
    store
        .append_verifier_run(&coding_agent_monitor::VerifierRun {
            verifier_run_id: "verifier-run-pass".into(),
            verifier_id: Some("rust_full".into()),
            command: "cargo test".into(),
            status: VerificationRunStatus::Passed,
            started_at: "2026-06-22T12:01:00Z".into(),
            completed_at: Some("2026-06-22T12:01:10Z".into()),
            exit_code: Some(0),
            output_digest: "fnv1a64:cbf29ce484222325".into(),
            failure_class: None,
        })
        .expect("verifier run");

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);

    assert_eq!(
        case_file.verification.status,
        coding_agent_monitor::VerificationStatus::Passed
    );
    assert_eq!(
        case_file.verification.latest_passing_command.as_deref(),
        Some("cargo test")
    );
    let verification = case_file
        .entropy
        .score(EntropyKind::Verification)
        .expect("verification score");
    assert!(
        verification.score < 75,
        "fresh verifier run should avoid force-verification entropy: {verification:?}"
    );
}

#[test]
fn case_file_treats_timed_out_verifier_run_as_failed_verification() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_verifier_run(&coding_agent_monitor::VerifierRun {
            verifier_run_id: "verifier-run-timeout".into(),
            verifier_id: Some("sleepy".into()),
            command: "cargo test".into(),
            status: VerificationRunStatus::TimedOut,
            started_at: "2026-06-22T12:01:00Z".into(),
            completed_at: Some("2026-06-22T12:01:01Z".into()),
            exit_code: None,
            output_digest: "fnv1a64:cbf29ce484222325".into(),
            failure_class: None,
        })
        .expect("verifier run");

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);

    assert_eq!(
        case_file.verification.status,
        coding_agent_monitor::VerificationStatus::Failed
    );
    assert_eq!(
        case_file.verification.latest_failing_command.as_deref(),
        Some("cargo test")
    );
    let verification = case_file
        .entropy
        .score(EntropyKind::Verification)
        .expect("verification score");
    assert!(
        verification.score >= 75,
        "timed-out verifier run should raise verification entropy: {verification:?}"
    );
}

#[test]
fn force_verification_packet_includes_latest_failure_class() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_verifier_run(&coding_agent_monitor::VerifierRun {
            verifier_run_id: "verifier-run-compile-failed".into(),
            verifier_id: Some("rust_full".into()),
            command: "cargo test".into(),
            status: VerificationRunStatus::Failed,
            started_at: "2026-06-22T12:01:00Z".into(),
            completed_at: Some("2026-06-22T12:01:10Z".into()),
            exit_code: Some(101),
            output_digest: "fnv1a64:cbf29ce484222325".into(),
            failure_class: Some(VerificationFailureClass::Compile),
        })
        .expect("verifier run");

    let advice = advise_workspace(temp.path()).expect("advice");

    assert_eq!(
        advice.final_action,
        ControlAction::ForceVerification {
            suite: VerificationSuite::Full,
            blocking: true,
        }
    );
    assert!(
        advice
            .packet
            .instructions
            .iter()
            .any(|instruction| instruction.text.contains("compile failure")),
        "packet instructions should include latest failure class: {:?}",
        advice.packet.instructions
    );
}

#[test]
fn advisor_case_file_prunes_tainted_evidence_before_endpoint_request() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            event_id: Some("evt-tainted-secret".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some("secret-bearing evidence was already tainted upstream".into()),
            redaction_status: Some("tainted".into()),
            redaction_rules: vec!["token_like".into()],
            ..Event::default()
        })
        .expect("tainted event");
    store
        .append_event(&Event {
            event_id: Some("evt-clean-plan".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some("This is a good point to stop while work remains.".into()),
            ..Event::default()
        })
        .expect("clean event");
    let decision = json!({
        "diagnosis_id": "diagnosis-clean-plan",
        "dominant_entropy": "plan",
        "entropy_scores": {
            "plan": { "score": 75, "confidence": 80 }
        },
        "top_evidence": [
            {
                "event_id": "evt-clean-plan",
                "why_it_matters": "Clean evidence is enough to propose a follow-up."
            }
        ],
        "cited_evidence_ids": ["evt-clean-plan"],
        "missing_evidence": [],
        "proposed_action": { "type": "send_follow_up", "target_agent": null },
        "expected_entropy_delta": [
            { "kind": "plan", "delta": -20 }
        ],
        "packet_intent": "continue bounded work",
        "packet_draft": {
            "urgency": "follow_up",
            "summary": "Continue with the next bounded step.",
            "instructions": ["Take one concrete next step."],
            "evidence_refs": ["evt-clean-plan"]
        },
        "ask_user": null,
        "confidence": 0.78
    });
    let (endpoint, request_rx) = serve_advisor_once(decision);
    set_test_env_var("CAM_TAINTED_PRUNE_KEY", "test-key");
    std::fs::write(
        store.root().join("config.json"),
        json!({
            "advisor": {
                "enabled": true,
                "provider": {
                    "endpoint": endpoint,
                    "model": "test-advisor",
                    "api_key_env": "CAM_TAINTED_PRUNE_KEY",
                    "timeout_secs": 5
                }
            }
        })
        .to_string(),
    )
    .expect("config");

    let advice = advise_workspace(temp.path()).expect("advice");
    let request = request_rx.recv().expect("advisor request");
    let case_file = advisor_request_case_file(&request);
    let evidence_ids = case_file_evidence_ids(&case_file);

    assert!(advice.advisor_used);
    assert!(evidence_ids.contains(&"evt-clean-plan"));
    assert!(!evidence_ids.contains(&"evt-tainted-secret"));
    assert!(!request.contains("evt-tainted-secret"));
    assert!(!request.contains("token_like"));
}

#[test]
fn advisor_case_file_includes_sanitized_task_summary_and_source_refs() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:00:00Z".into()),
            event_id: Some("evt-task-source".into()),
            agent: "user".into(),
            kind: EventKind::UserInstruction,
            content: Some(
                "Implement dedicated advisor config. api_key=dedicated-secret\nAcceptance criteria:\n- coding-plan profile is linked by reference"
                    .into(),
            ),
            ..Event::default()
        })
        .expect("event");
    let decision = json!({
        "diagnosis_id": "diagnosis-task-context",
        "dominant_entropy": "verification",
        "entropy_scores": {
            "verification": { "score": 82, "confidence": 80 }
        },
        "top_evidence": [
            {
                "event_id": "evt-task-source",
                "why_it_matters": "The task summary gives the advisor source-grounded context."
            }
        ],
        "cited_evidence_ids": ["evt-task-source"],
        "missing_evidence": ["passing verifier result"],
        "proposed_action": {
            "type": "force_verification",
            "suite": "full",
            "blocking": true
        },
        "expected_entropy_delta": [
            { "kind": "verification", "delta": -55 }
        ],
        "packet_intent": "require verification",
        "packet_draft": {
            "urgency": "urgent",
            "summary": "Verify before continuing.",
            "instructions": ["Run the full verifier."],
            "evidence_refs": ["evt-task-source"]
        },
        "ask_user": null,
        "confidence": 0.8
    });
    let (endpoint, request_rx) = serve_advisor_once(decision);
    set_test_env_var("CAM_TASK_CONTEXT_KEY", "test-key");
    std::fs::write(
        store.root().join("config.json"),
        json!({
            "advisor": {
                "enabled": true,
                "provider": {
                    "endpoint": endpoint,
                    "model": "test-advisor",
                    "api_key_env": "CAM_TASK_CONTEXT_KEY",
                    "timeout_secs": 5
                }
            }
        })
        .to_string(),
    )
    .expect("config");

    let advice = advise_workspace(temp.path()).expect("advice");
    let request = request_rx.recv().expect("advisor request");
    let case_file = advisor_request_case_file(&request);
    let task = case_file.get("task").expect("task summary");

    assert!(advice.advisor_used);
    assert!(!request.contains("dedicated-secret"));
    assert_eq!(
        task.pointer("/user_goal_event_id")
            .and_then(serde_json::Value::as_str),
        Some("evt-task-source")
    );
    assert!(
        task.pointer("/user_goal")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|goal| goal.contains("api_key=[REDACTED]"))
    );
    assert_eq!(
        task.pointer("/acceptance_criteria/0/text")
            .and_then(serde_json::Value::as_str),
        Some("coding-plan profile is linked by reference")
    );
    assert_eq!(
        task.pointer("/acceptance_criteria/0/source_event_id")
            .and_then(serde_json::Value::as_str),
        Some("evt-task-source")
    );
}

#[test]
fn test_result_event_feeds_failed_verification_entropy() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:00:00Z".into()),
            event_id: Some("evt-test-result-failed".into()),
            agent: "codex".into(),
            kind: EventKind::TestResult,
            command: Some("cargo test parser::tests::handles_nested".into()),
            exit_code: Some(1),
            ..Event::default()
        })
        .expect("event");

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);
    let verification = case_file
        .entropy
        .score(EntropyKind::Verification)
        .expect("verification score");

    assert!(verification.score >= 75, "{verification:?}");
    assert!(
        verification
            .evidence_ids
            .contains(&"evt-test-result-failed".into())
    );
    assert!(
        verification
            .top_causes
            .iter()
            .any(|cause| cause.contains("verification command failed"))
    );
}

#[test]
fn later_passing_verifier_run_clears_earlier_verifier_failure_entropy() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:00:00Z".into()),
            event_id: Some("evt-write".into()),
            agent: "codex".into(),
            kind: EventKind::FileChange,
            file: Some("src/lib.rs".into()),
            rationale: Some("Implement verifier integration.".into()),
            ..Event::default()
        })
        .expect("event");
    store
        .append_verifier_run(&coding_agent_monitor::VerifierRun {
            verifier_run_id: "verifier-run-timeout".into(),
            verifier_id: Some("rust_full".into()),
            command: "cargo test".into(),
            status: VerificationRunStatus::TimedOut,
            started_at: "2026-06-22T12:01:00Z".into(),
            completed_at: Some("2026-06-22T12:01:10Z".into()),
            exit_code: None,
            output_digest: "fnv1a64:cbf29ce484222325".into(),
            failure_class: None,
        })
        .expect("failed verifier run");
    store
        .append_verifier_run(&coding_agent_monitor::VerifierRun {
            verifier_run_id: "verifier-run-pass".into(),
            verifier_id: Some("rust_full".into()),
            command: "cargo test".into(),
            status: VerificationRunStatus::Passed,
            started_at: "2026-06-22T12:02:00Z".into(),
            completed_at: Some("2026-06-22T12:02:10Z".into()),
            exit_code: Some(0),
            output_digest: "fnv1a64:cbf29ce484222325".into(),
            failure_class: None,
        })
        .expect("passing verifier run");

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);
    let verification = case_file
        .entropy
        .score(EntropyKind::Verification)
        .expect("verification score");

    assert_eq!(
        case_file.verification.status,
        coding_agent_monitor::VerificationStatus::Passed
    );
    assert!(
        verification.score < 75,
        "later passing verifier run should clear prior failure entropy: {verification:?}"
    );
}

#[test]
fn same_second_later_passing_verifier_run_clears_prior_failure_by_append_order() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:00:00Z".into()),
            event_id: Some("evt-write".into()),
            agent: "codex".into(),
            kind: EventKind::FileChange,
            file: Some("src/lib.rs".into()),
            rationale: Some("Implement verifier integration.".into()),
            ..Event::default()
        })
        .expect("event");
    store
        .append_verifier_run(&coding_agent_monitor::VerifierRun {
            verifier_run_id: "verifier-run-timeout".into(),
            verifier_id: Some("rust_full".into()),
            command: "cargo test".into(),
            status: VerificationRunStatus::TimedOut,
            started_at: "2026-06-22T12:01:00Z".into(),
            completed_at: Some("2026-06-22T12:01:10Z".into()),
            exit_code: None,
            output_digest: "fnv1a64:cbf29ce484222325".into(),
            failure_class: None,
        })
        .expect("failed verifier run");
    store
        .append_verifier_run(&coding_agent_monitor::VerifierRun {
            verifier_run_id: "verifier-run-pass".into(),
            verifier_id: Some("rust_full".into()),
            command: "cargo test".into(),
            status: VerificationRunStatus::Passed,
            started_at: "2026-06-22T12:01:10Z".into(),
            completed_at: Some("2026-06-22T12:01:10Z".into()),
            exit_code: Some(0),
            output_digest: "fnv1a64:cbf29ce484222325".into(),
            failure_class: None,
        })
        .expect("passing verifier run");

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);
    let verification = case_file
        .entropy
        .score(EntropyKind::Verification)
        .expect("verification score");

    assert_eq!(
        case_file.verification.status,
        coding_agent_monitor::VerificationStatus::Passed
    );
    assert!(
        verification.score < 75,
        "same-second later pass should use verifier append order: {verification:?}"
    );
}

#[test]
fn case_file_recommends_configured_targeted_verifier_for_changed_path() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:01:00Z".into()),
            event_id: Some("evt-parser-write".into()),
            agent: "codex".into(),
            kind: EventKind::FileChange,
            file: Some("src/parser.rs".into()),
            rationale: Some("Implement nested parser behavior.".into()),
            ..Event::default()
        })
        .expect("event");
    let config = ProjectConfig {
        verifiers: vec![coding_agent_monitor::VerifierConfig {
            id: "parser_targeted".into(),
            command: "cargo test parser::tests::handles_nested".into(),
            scope: VerificationScope::Targeted,
            timeout_secs: 120,
            paths: vec!["src/parser.rs".into(), "tests/parser.rs".into()],
            acceptance_patterns: Vec::new(),
        }],
        ..ProjectConfig::default()
    };

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file_with_config(temp.path(), &snapshot, &config);

    assert_eq!(
        case_file.verification.recommended_commands,
        vec!["cargo test parser::tests::handles_nested"]
    );
    assert_eq!(
        case_file.verification.status,
        coding_agent_monitor::VerificationStatus::NotRun
    );
}

#[test]
fn case_file_recommends_verifier_for_matching_acceptance_criterion() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:00:00Z".into()),
            event_id: Some("evt-user-acceptance".into()),
            agent: "user".into(),
            kind: EventKind::UserInstruction,
            content: Some("Acceptance: nested parser behavior passes.".into()),
            ..Event::default()
        })
        .expect("event");
    let config = ProjectConfig {
        verifiers: vec![coding_agent_monitor::VerifierConfig {
            id: "parser_regression".into(),
            command: "cargo test regression::smoke".into(),
            scope: VerificationScope::Targeted,
            timeout_secs: 120,
            paths: vec!["src/parser.rs".into()],
            acceptance_patterns: vec!["nested parser".into()],
        }],
        ..ProjectConfig::default()
    };

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file_with_config(temp.path(), &snapshot, &config);
    let verification = case_file
        .entropy
        .score(EntropyKind::Verification)
        .expect("verification score");

    assert_eq!(
        case_file.verification.acceptance_criteria,
        vec!["nested parser behavior passes."]
    );
    assert_eq!(
        case_file.verification.recommended_commands,
        vec!["cargo test regression::smoke"]
    );
    assert!(
        case_file
            .verification
            .uncovered_acceptance_criteria
            .is_empty()
    );
    assert!(
        verification.score >= 75,
        "unverified acceptance criterion should keep verification entropy high: {verification:?}"
    );
}

#[test]
fn case_file_task_summary_extracts_latest_goal_and_acceptance_sources() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:00:00Z".into()),
            event_id: Some("evt-initial-goal".into()),
            agent: "user".into(),
            kind: EventKind::UserInstruction,
            content: Some(
                "Build the monitor supervisor.\nAcceptance criteria:\n- parser handles nested calls\n- advisor packets cite evidence"
                    .into(),
            ),
            ..Event::default()
        })
        .expect("event");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:03:00Z".into()),
            event_id: Some("evt-latest-goal".into()),
            agent: "user".into(),
            kind: EventKind::UserInstruction,
            content: Some("Continue with the dedicated coding-plan advisor endpoint.".into()),
            ..Event::default()
        })
        .expect("event");

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    assert_eq!(snapshot.recent_events.len(), 2);
    assert_eq!(
        snapshot.recent_events[0].content.as_deref(),
        Some(
            "Build the monitor supervisor.\nAcceptance criteria:\n- parser handles nested calls\n- advisor packets cite evidence"
        )
    );
    let case_file = build_control_case_file(temp.path(), &snapshot);

    assert_eq!(
        case_file.task.user_goal.as_deref(),
        Some("Continue with the dedicated coding-plan advisor endpoint.")
    );
    assert_eq!(
        case_file.task.user_goal_event_id.as_deref(),
        Some("evt-latest-goal")
    );
    assert_eq!(
        case_file.verification.acceptance_criteria,
        vec![
            "parser handles nested calls".to_string(),
            "advisor packets cite evidence".to_string()
        ]
    );
    assert_eq!(case_file.task.acceptance_criteria.len(), 2);
    assert_eq!(
        case_file.task.acceptance_criteria[0].text,
        "parser handles nested calls"
    );
    assert_eq!(
        case_file.task.acceptance_criteria[0]
            .source_event_id
            .as_deref(),
        Some("evt-initial-goal")
    );
    assert!(case_file.task.acceptance_criteria[0].confidence >= 80);
}

#[test]
fn case_file_task_summary_recovers_goal_and_acceptance_after_long_run() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:00:00Z".into()),
            event_id: Some("evt-original-long-run-goal".into()),
            agent: "user".into(),
            kind: EventKind::UserInstruction,
            content: Some(
                "Build the monitored control plane.\nAcceptance criteria:\n- stale verification is blocked\n- packets cite evidence"
                    .into(),
            ),
            ..Event::default()
        })
        .expect("event");
    for index in 0..600 {
        store
            .append_event(&Event {
                time: Some(format!("2026-06-22T12:{:02}:00Z", index % 60)),
                event_id: Some(format!("evt-tool-chatter-{index}")),
                agent: "codex".into(),
                kind: EventKind::ToolResult,
                content: Some(format!("tool chatter {index}")),
                ..Event::default()
            })
            .expect("chatter event");
    }

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    assert!(
        snapshot
            .recent_events
            .iter()
            .all(|event| event.event_id.as_deref() != Some("evt-original-long-run-goal"))
    );

    let case_file = build_control_case_file(temp.path(), &snapshot);
    let goal = case_file
        .entropy
        .score(EntropyKind::Goal)
        .expect("goal score");

    assert_eq!(
        case_file.task.user_goal.as_deref(),
        Some(
            "Build the monitored control plane.\nAcceptance criteria:\n- stale verification is blocked\n- packets cite evidence"
        )
    );
    assert_eq!(
        case_file.task.user_goal_event_id.as_deref(),
        Some("evt-original-long-run-goal")
    );
    assert_eq!(
        case_file.task.acceptance_criteria[0]
            .source_event_id
            .as_deref(),
        Some("evt-original-long-run-goal")
    );
    assert!(
        case_file
            .verification
            .acceptance_criteria
            .contains(&"stale verification is blocked".into())
    );
    assert!(
        goal.score < 60,
        "recovered goal should avoid missing-goal entropy: {goal:?}"
    );
}

#[test]
fn goal_entropy_rises_when_task_summary_has_ambiguity_marker() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:00:00Z".into()),
            event_id: Some("evt-ambiguous-goal".into()),
            agent: "user".into(),
            kind: EventKind::UserInstruction,
            content: Some("Goal: implement the endpoint. Which provider should be default?".into()),
            ..Event::default()
        })
        .expect("event");

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);
    let goal = case_file
        .entropy
        .score(EntropyKind::Goal)
        .expect("goal score");

    assert!(
        goal.score >= 80,
        "ambiguous user goal should raise goal entropy: {goal:?}"
    );
    assert!(
        goal.top_causes
            .iter()
            .any(|cause| cause.contains("unresolved ambiguity")),
        "goal entropy should explain ambiguity: {goal:?}"
    );
    assert!(goal.evidence_ids.contains(&"evt-ambiguous-goal".into()));
}

#[test]
fn goal_entropy_rises_when_no_user_goal_is_captured() {
    let temp = tempfile::tempdir().expect("temp dir");
    let store = ProjectStore::open(temp.path()).expect("store");

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);
    let goal = case_file
        .entropy
        .score(EntropyKind::Goal)
        .expect("goal score");

    assert!(
        goal.score >= 60,
        "missing user goal should raise bounded goal entropy: {goal:?}"
    );
    assert!(
        goal.missing_evidence
            .contains(&"current user goal or acceptance criteria".into()),
        "missing goal should name the missing evidence: {goal:?}"
    );
}

#[test]
fn case_file_flags_rejected_alternative_resurrection_from_user_instruction() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:00:00Z".into()),
            event_id: Some("evt-reject-public-dataset".into()),
            agent: "user".into(),
            kind: EventKind::UserInstruction,
            content: Some("Do not create a public dataset concept.".into()),
            ..Event::default()
        })
        .expect("user event");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:01:00Z".into()),
            event_id: Some("evt-resurrect-public-dataset".into()),
            agent: "codex".into(),
            kind: EventKind::DesignThought,
            content: Some("Plan: add a public dataset concept for compatibility.".into()),
            ..Event::default()
        })
        .expect("agent event");

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);
    let context = case_file
        .entropy
        .score(EntropyKind::Context)
        .expect("context score");
    let plan = case_file
        .entropy
        .score(EntropyKind::Plan)
        .expect("plan score");

    assert!(
        context.score >= 80,
        "rejected alternative recurrence should raise context entropy: {context:?}"
    );
    assert!(
        plan.score >= 70,
        "rejected alternative recurrence should raise plan entropy: {plan:?}"
    );
    assert!(
        context
            .top_causes
            .iter()
            .any(|cause| cause.contains("rejected alternative")),
        "{context:?}"
    );
    assert!(
        context
            .evidence_ids
            .contains(&"evt-resurrect-public-dataset".into())
    );
    assert!(
        context
            .missing_evidence
            .iter()
            .any(|missing| missing.contains("user authorization")),
        "{context:?}"
    );
}

#[test]
fn case_file_flags_rejected_alternative_resurrection_from_durable_memory() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_memory(&coding_agent_monitor::MemoryCandidate {
            memory_id: "mem-no-public-dataset".into(),
            scope: coding_agent_monitor::MemoryScope::Project,
            claim: "Do not create a public dataset concept.".into(),
            status: coding_agent_monitor::MemoryStatus::Active,
            source: MemorySource::User,
            evidence_ids: vec!["evt-memory-no-public-dataset".into()],
            confidence: 92,
        })
        .expect("memory");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:01:00Z".into()),
            event_id: Some("evt-durable-resurrect-public-dataset".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some("I added the public dataset concept as a compatibility layer.".into()),
            ..Event::default()
        })
        .expect("agent event");

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);
    let context = case_file
        .entropy
        .score(EntropyKind::Context)
        .expect("context score");

    assert!(
        context.score >= 80,
        "durable rejected alternative memory should be enforced: {context:?}"
    );
    assert!(
        context
            .evidence_ids
            .contains(&"evt-durable-resurrect-public-dataset".into())
    );
}

#[test]
fn follow_up_packet_names_rejected_alternative_resurrection() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:00:00Z".into()),
            event_id: Some("evt-packet-reject-public-dataset".into()),
            agent: "user".into(),
            kind: EventKind::UserInstruction,
            content: Some("Do not create a public dataset concept.".into()),
            ..Event::default()
        })
        .expect("user event");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:01:00Z".into()),
            event_id: Some("evt-packet-resurrect-public-dataset".into()),
            agent: "codex".into(),
            kind: EventKind::DesignThought,
            content: Some("Plan: add a public dataset concept for compatibility.".into()),
            ..Event::default()
        })
        .expect("agent event");

    let advice = advise_workspace(temp.path()).expect("advice");

    assert_eq!(
        advice.final_action,
        ControlAction::SendFollowUp { target_agent: None }
    );
    assert!(
        advice
            .packet
            .instructions
            .iter()
            .any(
                |instruction| instruction.text.contains("rejected alternative")
                    && instruction.text.contains("Do not implement")
            ),
        "packet should name rejected alternative recurrence: {:?}",
        advice.packet.instructions
    );
}

#[test]
fn case_file_flags_routine_agent_question_before_local_probe() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:00:00Z".into()),
            event_id: Some("evt-routine-question-goal".into()),
            agent: "user".into(),
            kind: EventKind::UserInstruction,
            content: Some(
                "Fix the failing dashboard flow without asking routine sequencing questions."
                    .into(),
            ),
            ..Event::default()
        })
        .expect("user event");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:01:00Z".into()),
            event_id: Some("evt-routine-question".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some(
                "Should I run the browser probe first or inspect the backend files next?".into(),
            ),
            ..Event::default()
        })
        .expect("agent event");

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);
    let plan = case_file
        .entropy
        .score(EntropyKind::Plan)
        .expect("plan score");

    assert!(
        plan.score >= 70,
        "routine question should raise plan entropy: {plan:?}"
    );
    assert!(
        plan.top_causes
            .iter()
            .any(|cause| cause.contains("routine user question")),
        "{plan:?}"
    );
    assert!(plan.evidence_ids.contains(&"evt-routine-question".into()));
    assert!(
        plan.missing_evidence
            .iter()
            .any(|missing| missing.contains("local probe")),
        "{plan:?}"
    );
    assert!(
        case_file
            .entropy
            .score(EntropyKind::UserDecision)
            .is_none_or(|score| score.score < 80),
        "routine question must not authorize AskUser: {:?}",
        case_file.entropy.score(EntropyKind::UserDecision)
    );
}

#[test]
fn follow_up_packet_names_routine_question_probe_gate() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:00:00Z".into()),
            event_id: Some("evt-routine-question-packet-goal".into()),
            agent: "user".into(),
            kind: EventKind::UserInstruction,
            content: Some("Fix the failing dashboard flow autonomously.".into()),
            ..Event::default()
        })
        .expect("user event");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:01:00Z".into()),
            event_id: Some("evt-routine-question-packet".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some("Which file should I inspect next before continuing?".into()),
            ..Event::default()
        })
        .expect("agent event");

    let advice = advise_workspace(temp.path()).expect("advice");

    assert_eq!(
        advice.final_action,
        ControlAction::RunProbe {
            probe: coding_agent_monitor::ProbeSpec::LocalEvidence {
                target: Some("routine_next_step".into())
            }
        }
    );
    assert!(
        advice
            .packet
            .instructions
            .iter()
            .any(|instruction| instruction.text.contains("Do not ask the user"))
            && advice
                .packet
                .instructions
                .iter()
                .any(
                    |instruction| instruction.text.contains("The monitor will run")
                        && instruction.text.contains("local evidence probe")
                ),
        "packet should gate routine questions behind local probes: {:?}",
        advice.packet.instructions
    );
}

#[test]
fn agent_facing_run_probe_packet_avoids_internal_controller_jargon() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:00:00Z".into()),
            event_id: Some("evt-probe-jargon-goal".into()),
            agent: "user".into(),
            kind: EventKind::UserInstruction,
            content: Some("Fix the failing dashboard flow autonomously.".into()),
            ..Event::default()
        })
        .expect("user event");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:01:00Z".into()),
            event_id: Some("evt-probe-jargon-question".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some("Which file should I inspect next before continuing?".into()),
            ..Event::default()
        })
        .expect("agent event");

    let advice = advise_workspace(temp.path()).expect("advice");
    let packet_text = packet_text_for_assertion(&advice.packet);

    assert_eq!(
        advice.final_action,
        ControlAction::RunProbe {
            probe: ProbeSpec::LocalEvidence {
                target: Some("routine_next_step".into())
            }
        }
    );
    assert!(
        packet_text.contains("monitor-owned local evidence probe"),
        "packet should name the evidence source in agent-operational terms: {packet_text}"
    );
    assert!(
        packet_text.contains("Wait for the recorded probe result"),
        "packet should make the next checkpoint explicit: {packet_text}"
    );
    for low_signal in ["entropy", "uncertainty", "cheap local", "selected"] {
        assert!(
            !packet_text.contains(low_signal),
            "packet should avoid low-signal controller jargon `{low_signal}`: {packet_text}"
        );
    }
}

#[test]
fn agent_facing_handoff_packet_names_takeover_checkpoint_without_entropy_jargon() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:00:00Z".into()),
            event_id: Some("evt-context-jargon".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some("I lost design memory and need the project context again.".into()),
            ..Event::default()
        })
        .expect("event");

    let advice = advise_workspace(temp.path()).expect("advice");
    let packet_text = packet_text_for_assertion(&advice.packet);

    assert!(matches!(
        advice.final_action,
        ControlAction::SpawnFreshAgent { .. }
    ));
    assert!(
        packet_text.contains("Take over from the bounded case file"),
        "handoff packet should start with the concrete takeover source: {packet_text}"
    );
    assert!(
        packet_text.contains(
            "current goal, active memory constraints, recent trace, and verification state"
        ),
        "handoff packet should state the minimum context checklist: {packet_text}"
    );
    for low_signal in [
        "entropy",
        "selected an agent switch",
        "selected a fresh agent",
    ] {
        assert!(
            !packet_text.contains(low_signal),
            "handoff packet should avoid low-signal controller jargon `{low_signal}`: {packet_text}"
        );
    }
}

fn packet_text_for_assertion(packet: &ControlPacket) -> String {
    let mut text = format!("{} {}\n", packet.title, packet.summary);
    for instruction in &packet.instructions {
        text.push_str(&instruction.text);
        text.push('\n');
    }
    for item in &packet.forbidden {
        text.push_str(item);
        text.push('\n');
    }
    for item in &packet.success_criteria {
        text.push_str(item);
        text.push('\n');
    }
    text
}

#[test]
fn advise_workspace_executes_monitor_owned_local_probe_immediately() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    std::fs::write(
        store.root().join("config.json"),
        json!({
            "verifiers": [
                {
                    "id": "should-not-run",
                    "command": passing_verifier_command(),
                    "scope": "targeted",
                    "timeout_secs": 5,
                    "paths": ["src/lib.rs"]
                }
            ]
        })
        .to_string(),
    )
    .expect("config");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:00:00Z".into()),
            event_id: Some("evt-auto-probe-goal".into()),
            agent: "user".into(),
            kind: EventKind::UserInstruction,
            content: Some("Fix the failing dashboard flow autonomously.".into()),
            ..Event::default()
        })
        .expect("user event");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:01:00Z".into()),
            event_id: Some("evt-auto-probe-question".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some("Which file should I inspect next before continuing?".into()),
            ..Event::default()
        })
        .expect("question event");
    drop(store);

    let advice = advise_workspace(temp.path()).expect("advice");
    let store = ProjectStore::open(temp.path()).expect("store");
    let probe_log =
        std::fs::read_to_string(store.root().join("probe-runs.jsonl")).expect("probe log");
    let runs = probe_log
        .lines()
        .map(|line| {
            serde_json::from_str::<coding_agent_monitor::ProbeRun>(line).expect("probe run json")
        })
        .collect::<Vec<_>>();
    let trails = load_decision_trails(store.root()).expect("trails");

    assert_eq!(
        advice.final_action,
        ControlAction::RunProbe {
            probe: ProbeSpec::LocalEvidence {
                target: Some("routine_next_step".into())
            }
        }
    );
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].advice_id, advice.advice_id);
    assert_eq!(runs[0].status, OutcomeStatus::Succeeded);
    assert!(runs[0].summary.contains("local evidence probe"));
    assert!(
        runs[0]
            .evidence_ids
            .contains(&"evt-auto-probe-question".into())
    );
    assert!(
        !store.root().join("verifier-runs.jsonl").exists(),
        "automatic LocalEvidence probe must not execute configured verifiers"
    );
    assert!(
        advice
            .packet
            .instructions
            .iter()
            .any(|instruction| instruction.text.contains("The monitor will run")),
        "packet should not ask the coding agent to perform the monitor-owned probe: {:?}",
        advice.packet.instructions
    );
    let trail = trails
        .iter()
        .find(|trail| trail.advice.advice_id == advice.advice_id)
        .expect("probe trail");
    let outcome = trail.outcomes.first().expect("probe outcome");
    assert_eq!(outcome.action, ControlActionKind::RunProbe);
    assert_eq!(outcome.status, OutcomeStatus::Succeeded);
    assert!(outcome.evidence_ids.contains(&runs[0].probe_run_id));
}

#[test]
fn policy_validator_rejects_run_probe_without_probe_worthy_entropy() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:00:00Z".into()),
            event_id: Some("evt-no-probe-entropy".into()),
            agent: "user".into(),
            kind: EventKind::UserInstruction,
            content: Some("Keep going on the documented implementation plan.".into()),
            ..Event::default()
        })
        .expect("event");
    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);

    let outcome = validate_control_action_detailed(
        ControlAction::RunProbe {
            probe: coding_agent_monitor::ProbeSpec::LocalEvidence { target: None },
        },
        &case_file,
    );

    match outcome {
        ValidationOutcome::Modified {
            original,
            replacement,
            reason,
        } => {
            assert_eq!(
                original,
                ControlAction::RunProbe {
                    probe: coding_agent_monitor::ProbeSpec::LocalEvidence { target: None },
                }
            );
            assert_eq!(replacement, ControlAction::ContinueWorking);
            assert!(reason.contains("probe-worthy entropy"));
        }
        other => panic!("expected low-entropy run_probe rewrite, got {other:?}"),
    }
}

#[test]
fn advisor_request_allows_run_probe_for_probe_worthy_plan_entropy() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:00:00Z".into()),
            event_id: Some("evt-probe-action-goal".into()),
            agent: "user".into(),
            kind: EventKind::UserInstruction,
            content: Some("Fix the failing dashboard flow autonomously.".into()),
            ..Event::default()
        })
        .expect("user event");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:01:00Z".into()),
            event_id: Some("evt-probe-action-question".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some("Which file should I inspect next before continuing?".into()),
            ..Event::default()
        })
        .expect("agent event");

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);

    assert!(
        case_file
            .allowed_actions
            .contains(&ControlActionKind::RunProbe),
        "probe-worthy plan entropy should expose run_probe to advisors: {:?}",
        case_file.allowed_actions
    );
}

#[test]
fn routine_question_gate_does_not_downgrade_credential_decisions() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:00:00Z".into()),
            event_id: Some("evt-credential-question".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some(
                "Should I continue after you provide credentials? I need your API key to verify the production integration.".into(),
            ),
            ..Event::default()
        })
        .expect("agent event");

    let advice = advise_workspace(temp.path()).expect("advice");

    assert!(
        matches!(advice.final_action, ControlAction::AskUser { .. }),
        "credential blockers should still reach AskUser: {:?}",
        advice.final_action
    );
}

#[test]
fn case_file_records_requirement_level_acceptance_coverage() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:00:00Z".into()),
            event_id: Some("evt-user-acceptance".into()),
            agent: "user".into(),
            kind: EventKind::UserInstruction,
            content: Some(
                "Acceptance criteria:\n- nested parser behavior passes.\n- export CSV report."
                    .into(),
            ),
            ..Event::default()
        })
        .expect("event");
    store
        .append_verifier_run(&coding_agent_monitor::VerifierRun {
            verifier_run_id: "verifier-run-parser".into(),
            verifier_id: Some("parser_regression".into()),
            command: "cargo test regression::smoke".into(),
            status: VerificationRunStatus::Passed,
            started_at: "2026-06-22T12:01:00Z".into(),
            completed_at: Some("2026-06-22T12:01:03Z".into()),
            exit_code: Some(0),
            output_digest: "fnv1a64:cbf29ce484222325".into(),
            failure_class: None,
        })
        .expect("verifier run");
    let config = ProjectConfig {
        verifiers: vec![coding_agent_monitor::VerifierConfig {
            id: "parser_regression".into(),
            command: "cargo test regression::smoke".into(),
            scope: VerificationScope::Targeted,
            timeout_secs: 120,
            paths: vec!["src/parser.rs".into()],
            acceptance_patterns: vec!["nested parser".into()],
        }],
        ..ProjectConfig::default()
    };

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file_with_config(temp.path(), &snapshot, &config);

    assert_eq!(
        case_file.verification.acceptance_criteria,
        vec![
            "nested parser behavior passes.".to_string(),
            "export CSV report.".to_string()
        ]
    );
    assert_eq!(case_file.verification.acceptance_coverage.len(), 2);
    let parser = &case_file.verification.acceptance_coverage[0];
    assert_eq!(parser.criterion, "nested parser behavior passes.");
    assert_eq!(
        parser.status,
        coding_agent_monitor::AcceptanceCoverageStatus::Covered
    );
    assert_eq!(parser.verifier_ids, vec!["parser_regression"]);
    assert_eq!(
        parser.verifier_commands,
        vec!["cargo test regression::smoke"]
    );
    assert_eq!(
        parser.latest_status,
        Some(coding_agent_monitor::VerificationStatus::Passed)
    );

    let csv = &case_file.verification.acceptance_coverage[1];
    assert_eq!(csv.criterion, "export CSV report.");
    assert_eq!(
        csv.status,
        coding_agent_monitor::AcceptanceCoverageStatus::Unmapped
    );
    assert!(csv.verifier_ids.is_empty());
    assert_eq!(csv.latest_status, None);

    assert_eq!(case_file.requirements.len(), 2);
    let parser_req = &case_file.requirements[0];
    assert_eq!(
        parser_req.requirement_id,
        "req-nested-parser-behavior-passes"
    );
    assert_eq!(parser_req.text, "nested parser behavior passes.");
    assert_eq!(
        parser_req.source_event_id.as_deref(),
        Some("evt-user-acceptance")
    );
    assert_eq!(
        parser_req.evidence_ids,
        vec!["evt-user-acceptance", "verifier-run-parser"]
    );
    assert!(parser_req.evidence_refs.iter().any(|evidence| {
        evidence.evidence_id == "evt-user-acceptance"
            && evidence.role == coding_agent_monitor::RequirementEvidenceRole::RequirementSource
            && evidence.necessity == coding_agent_monitor::RequirementEvidenceNecessity::Necessary
    }));
    assert!(parser_req.evidence_refs.iter().any(|evidence| {
        evidence.evidence_id == "verifier-run-parser"
            && evidence.role == coding_agent_monitor::RequirementEvidenceRole::VerificationResult
            && evidence.necessity == coding_agent_monitor::RequirementEvidenceNecessity::Necessary
    }));
    assert_eq!(
        parser_req.latest_verification_evidence_id.as_deref(),
        Some("verifier-run-parser")
    );
    assert_eq!(parser_req.verifier_ids, vec!["parser_regression"]);
    assert_eq!(
        parser_req.verifier_commands,
        vec!["cargo test regression::smoke"]
    );
    assert_eq!(
        parser_req.status,
        coding_agent_monitor::AcceptanceCoverageStatus::Covered
    );
    assert_eq!(
        parser_req.latest_status,
        Some(coding_agent_monitor::VerificationStatus::Passed)
    );

    let csv_req = &case_file.requirements[1];
    assert_eq!(csv_req.requirement_id, "req-export-csv-report");
    assert_eq!(csv_req.text, "export CSV report.");
    assert_eq!(
        csv_req.source_event_id.as_deref(),
        Some("evt-user-acceptance")
    );
    assert_eq!(csv_req.evidence_ids, vec!["evt-user-acceptance"]);
    assert_eq!(csv_req.evidence_refs.len(), 1);
    assert_eq!(
        csv_req.evidence_refs[0].role,
        coding_agent_monitor::RequirementEvidenceRole::RequirementSource
    );
    assert_eq!(
        csv_req.evidence_refs[0].necessity,
        coding_agent_monitor::RequirementEvidenceNecessity::Necessary
    );
    assert_eq!(csv_req.evidence_refs[0].evidence_id, "evt-user-acceptance");
    assert_eq!(csv_req.latest_verification_evidence_id, None);
    assert_eq!(
        csv_req.status,
        coding_agent_monitor::AcceptanceCoverageStatus::Unmapped
    );
    assert!(csv_req.verifier_ids.is_empty());
}

#[test]
fn case_file_surfaces_uncovered_acceptance_criterion() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:00:00Z".into()),
            event_id: Some("evt-user-acceptance".into()),
            agent: "user".into(),
            kind: EventKind::UserInstruction,
            content: Some("Acceptance: export CSV report.".into()),
            ..Event::default()
        })
        .expect("event");
    let config = ProjectConfig {
        verifiers: vec![coding_agent_monitor::VerifierConfig {
            id: "parser_nested".into(),
            command: "cargo test parser::tests::handles_nested".into(),
            scope: VerificationScope::Targeted,
            timeout_secs: 120,
            paths: vec!["src/parser.rs".into(), "tests/parser.rs".into()],
            acceptance_patterns: Vec::new(),
        }],
        ..ProjectConfig::default()
    };

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file_with_config(temp.path(), &snapshot, &config);
    let verification = case_file
        .entropy
        .score(EntropyKind::Verification)
        .expect("verification score");

    assert_eq!(
        case_file.verification.uncovered_acceptance_criteria,
        vec!["export CSV report."]
    );
    assert!(case_file.verification.recommended_commands.is_empty());
    assert!(
        verification
            .missing_evidence
            .contains(&"mapped verifier for acceptance criterion".into())
    );
    assert!(
        verification
            .evidence_ids
            .contains(&"evt-user-acceptance".into()),
        "verification entropy should cite the user instruction that introduced the acceptance gap: {verification:?}"
    );
}

#[test]
fn force_verification_packet_includes_acceptance_coverage_gap() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:00:00Z".into()),
            event_id: Some("evt-user-acceptance".into()),
            agent: "user".into(),
            kind: EventKind::UserInstruction,
            content: Some("Acceptance: export CSV report.".into()),
            ..Event::default()
        })
        .expect("event");

    let advice = advise_workspace(temp.path()).expect("advice");

    assert!(matches!(
        advice.final_action,
        ControlAction::ForceVerification { .. }
    ));
    assert!(
        advice.packet.instructions.iter().any(|instruction| {
            instruction.text.contains("Unmapped acceptance criterion")
                && instruction.text.contains("export CSV report")
        }),
        "packet should name the uncovered acceptance criterion: {:?}",
        advice.packet.instructions
    );
}

#[test]
fn case_file_keeps_stale_when_only_unrelated_targeted_verifier_passes() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:00:00Z".into()),
            event_id: Some("evt-parser-write".into()),
            agent: "codex".into(),
            kind: EventKind::FileChange,
            file: Some("src/parser.rs".into()),
            rationale: Some("Implement nested parser behavior.".into()),
            ..Event::default()
        })
        .expect("event");
    store
        .append_verifier_run(&coding_agent_monitor::VerifierRun {
            verifier_run_id: "verifier-run-ui-pass".into(),
            verifier_id: Some("ui_targeted".into()),
            command: "cargo test ui::smoke".into(),
            status: VerificationRunStatus::Passed,
            started_at: "2026-06-22T12:01:00Z".into(),
            completed_at: Some("2026-06-22T12:01:10Z".into()),
            exit_code: Some(0),
            output_digest: "fnv1a64:cbf29ce484222325".into(),
            failure_class: None,
        })
        .expect("verifier run");
    let config = ProjectConfig {
        verifiers: vec![
            coding_agent_monitor::VerifierConfig {
                id: "parser_targeted".into(),
                command: "cargo test parser::tests::handles_nested".into(),
                scope: VerificationScope::Targeted,
                timeout_secs: 120,
                paths: vec!["src/parser.rs".into(), "tests/parser.rs".into()],
                acceptance_patterns: Vec::new(),
            },
            coding_agent_monitor::VerifierConfig {
                id: "ui_targeted".into(),
                command: "cargo test ui::smoke".into(),
                scope: VerificationScope::Targeted,
                timeout_secs: 120,
                paths: vec!["src/ui.rs".into()],
                acceptance_patterns: Vec::new(),
            },
        ],
        ..ProjectConfig::default()
    };

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file_with_config(temp.path(), &snapshot, &config);
    let verification = case_file
        .entropy
        .score(EntropyKind::Verification)
        .expect("verification score");

    assert_eq!(
        case_file.verification.status,
        coding_agent_monitor::VerificationStatus::Stale
    );
    assert_eq!(
        case_file.verification.recommended_commands,
        vec!["cargo test parser::tests::handles_nested"]
    );
    assert!(
        verification.score >= 75,
        "unrelated targeted pass must not clear parser verification: {verification:?}"
    );
}

#[test]
fn case_file_clears_stale_when_matching_targeted_verifier_passes() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:00:00Z".into()),
            event_id: Some("evt-parser-write".into()),
            agent: "codex".into(),
            kind: EventKind::FileChange,
            file: Some("src/parser.rs".into()),
            rationale: Some("Implement nested parser behavior.".into()),
            ..Event::default()
        })
        .expect("event");
    store
        .append_verifier_run(&coding_agent_monitor::VerifierRun {
            verifier_run_id: "verifier-run-ui-pass".into(),
            verifier_id: Some("ui_targeted".into()),
            command: "cargo test ui::smoke".into(),
            status: VerificationRunStatus::Passed,
            started_at: "2026-06-22T12:01:00Z".into(),
            completed_at: Some("2026-06-22T12:01:10Z".into()),
            exit_code: Some(0),
            output_digest: "fnv1a64:cbf29ce484222325".into(),
            failure_class: None,
        })
        .expect("ui verifier run");
    store
        .append_verifier_run(&coding_agent_monitor::VerifierRun {
            verifier_run_id: "verifier-run-parser-pass".into(),
            verifier_id: Some("parser_targeted".into()),
            command: "cargo test parser::tests::handles_nested".into(),
            status: VerificationRunStatus::Passed,
            started_at: "2026-06-22T12:02:00Z".into(),
            completed_at: Some("2026-06-22T12:02:10Z".into()),
            exit_code: Some(0),
            output_digest: "fnv1a64:cbf29ce484222325".into(),
            failure_class: None,
        })
        .expect("parser verifier run");
    let config = ProjectConfig {
        verifiers: vec![
            coding_agent_monitor::VerifierConfig {
                id: "parser_targeted".into(),
                command: "cargo test parser::tests::handles_nested".into(),
                scope: VerificationScope::Targeted,
                timeout_secs: 120,
                paths: vec!["src/parser.rs".into(), "tests/parser.rs".into()],
                acceptance_patterns: Vec::new(),
            },
            coding_agent_monitor::VerifierConfig {
                id: "ui_targeted".into(),
                command: "cargo test ui::smoke".into(),
                scope: VerificationScope::Targeted,
                timeout_secs: 120,
                paths: vec!["src/ui.rs".into()],
                acceptance_patterns: Vec::new(),
            },
        ],
        ..ProjectConfig::default()
    };

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file_with_config(temp.path(), &snapshot, &config);
    let verification = case_file
        .entropy
        .score(EntropyKind::Verification)
        .expect("verification score");

    assert_eq!(
        case_file.verification.status,
        coding_agent_monitor::VerificationStatus::Passed
    );
    assert!(
        verification.score < 75,
        "matching targeted pass should clear parser verification: {verification:?}"
    );
}

#[test]
fn case_file_recommends_targeted_verifier_for_repo_audit_changed_path() {
    let temp = tempfile::tempdir().expect("temp dir");
    init_git_repo(temp.path());
    std::fs::create_dir_all(temp.path().join("src")).expect("src dir");
    std::fs::write(temp.path().join("src/parser.rs"), "fn parse() {}\n").expect("source");
    run_git(temp.path(), ["add", "src/parser.rs"]);
    run_git(temp.path(), ["commit", "-m", "add parser"]);
    std::fs::write(temp.path().join("src/parser.rs"), "fn parse_nested() {}\n")
        .expect("changed source");
    let store = ProjectStore::open(temp.path()).expect("store");
    let config = ProjectConfig {
        verifiers: vec![coding_agent_monitor::VerifierConfig {
            id: "parser_targeted".into(),
            command: "cargo test parser::tests::handles_nested".into(),
            scope: VerificationScope::Targeted,
            timeout_secs: 120,
            paths: vec!["src/parser.rs".into(), "tests/parser.rs".into()],
            acceptance_patterns: Vec::new(),
        }],
        ..ProjectConfig::default()
    };

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file_with_config(temp.path(), &snapshot, &config);

    assert_eq!(
        case_file.verification.recommended_commands,
        vec!["cargo test parser::tests::handles_nested"]
    );
    assert_eq!(
        case_file.verification.changed_source_files,
        vec!["src/parser.rs"]
    );
}

#[test]
fn case_file_treats_design_thoughts_as_unverified_memory_candidates() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:01:00Z".into()),
            event_id: Some("evt-design".into()),
            agent: "codex".into(),
            kind: EventKind::DesignThought,
            content: Some("Parser must preserve comments for formatter roundtrip.".into()),
            ..Event::default()
        })
        .expect("event");

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);

    assert_eq!(case_file.memory_candidates.len(), 1);
    assert_eq!(
        case_file.memory_candidates[0].claim,
        "Parser must preserve comments for formatter roundtrip."
    );
    assert_eq!(
        case_file.memory_candidates[0].status,
        coding_agent_monitor::MemoryStatus::Unverified
    );
    assert_eq!(
        case_file.memory_candidates[0].evidence_ids,
        vec!["evt-design"]
    );
}

#[test]
fn case_file_treats_durable_user_instructions_as_memory_candidates() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:01:00Z".into()),
            event_id: Some("evt-user-memory".into()),
            agent: "user".into(),
            kind: EventKind::UserInstruction,
            content: Some(
                "Remember: Do not copy local Codex or Claude CLI credentials into project config."
                    .into(),
            ),
            ..Event::default()
        })
        .expect("event");

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);

    assert_eq!(case_file.memory_candidates.len(), 1);
    assert_eq!(
        case_file.memory_candidates[0].claim,
        "Do not copy local Codex or Claude CLI credentials into project config."
    );
    assert_eq!(
        case_file.memory_candidates[0].source,
        coding_agent_monitor::MemorySource::User
    );
    assert_eq!(
        case_file.memory_candidates[0].status,
        coding_agent_monitor::MemoryStatus::Unverified
    );
    assert_eq!(
        case_file.memory_candidates[0].evidence_ids,
        vec!["evt-user-memory"]
    );
}

#[test]
fn case_file_ignores_generic_user_instructions_as_memory_candidates() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:01:00Z".into()),
            event_id: Some("evt-generic-user-request".into()),
            agent: "user".into(),
            kind: EventKind::UserInstruction,
            content: Some("Please run the tests and tell me what failed.".into()),
            ..Event::default()
        })
        .expect("event");

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);

    assert!(case_file.memory_candidates.is_empty());
}

#[test]
fn promote_memory_candidate_persists_manual_review_memory_as_durable() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:01:00Z".into()),
            event_id: Some("evt-design-promote".into()),
            agent: "codex".into(),
            kind: EventKind::DesignThought,
            content: Some("Adapters must support Codex, Claude Code, Pi, and OpenCode.".into()),
            ..Event::default()
        })
        .expect("event");

    let promoted = promote_memory_candidate(
        temp.path(),
        "mem-evt-design-promote",
        coding_agent_monitor::MemorySource::ManualReview,
    )
    .expect("promote memory");
    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);

    assert_eq!(promoted.memory_id, "mem-evt-design-promote");
    assert_eq!(promoted.status, coding_agent_monitor::MemoryStatus::Active);
    assert_eq!(
        promoted.source,
        coding_agent_monitor::MemorySource::ManualReview
    );
    assert_eq!(promoted.evidence_ids, vec!["evt-design-promote"]);
    assert!(promoted.confidence >= 90);
    assert_eq!(case_file.durable_memory.len(), 1);
    assert_eq!(
        case_file.durable_memory[0].claim,
        "Adapters must support Codex, Claude Code, Pi, and OpenCode."
    );
}

#[test]
fn promote_memory_candidate_rejects_agent_claim_as_trusted_source() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            event_id: Some("evt-design-agent-source".into()),
            agent: "codex".into(),
            kind: EventKind::DesignThought,
            content: Some("This needs trusted review before durable persistence.".into()),
            ..Event::default()
        })
        .expect("event");

    let error = promote_memory_candidate(
        temp.path(),
        "mem-evt-design-agent-source",
        coding_agent_monitor::MemorySource::AgentClaim,
    )
    .expect_err("agent claim cannot be a durable promotion source");

    assert!(error.to_string().contains("trusted source"));
    assert!(!store.root().join("memories.jsonl").exists());
}

#[test]
fn promote_memory_candidate_rejects_secret_like_claims_before_persisting() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            event_id: Some("evt-design-secret".into()),
            agent: "codex".into(),
            kind: EventKind::DesignThought,
            content: Some("Use api_key=super-secret-token during integration tests.".into()),
            ..Event::default()
        })
        .expect("event");

    let error = promote_memory_candidate(
        temp.path(),
        "mem-evt-design-secret",
        coding_agent_monitor::MemorySource::ManualReview,
    )
    .expect_err("secret-like memory should not persist");

    assert!(error.to_string().contains("tainted"));
    assert!(!store.root().join("memories.jsonl").exists());
}

#[test]
fn promote_memory_candidate_rejects_broader_token_like_claims() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            event_id: Some("evt-design-token".into()),
            agent: "codex".into(),
            kind: EventKind::DesignThought,
            content: Some("Use token=super-secret-token during integration tests.".into()),
            ..Event::default()
        })
        .expect("event");

    let error = promote_memory_candidate(
        temp.path(),
        "mem-evt-design-token",
        coding_agent_monitor::MemorySource::ManualReview,
    )
    .expect_err("token-like memory should not persist");

    assert!(error.to_string().contains("tainted"));
    assert!(!store.root().join("memories.jsonl").exists());
}

#[test]
fn promote_memory_candidate_rejects_spaced_token_like_claims() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            event_id: Some("evt-design-spaced-token".into()),
            agent: "codex".into(),
            kind: EventKind::DesignThought,
            content: Some("Use token = super-secret-token during integration tests.".into()),
            ..Event::default()
        })
        .expect("event");

    let error = promote_memory_candidate(
        temp.path(),
        "mem-evt-design-spaced-token",
        coding_agent_monitor::MemorySource::ManualReview,
    )
    .expect_err("spaced token-like memory should not persist");

    assert!(error.to_string().contains("tainted"));
    assert!(!store.root().join("memories.jsonl").exists());
}

#[test]
fn promote_memory_candidate_rejects_secret_like_evidence_ids() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            event_id: Some("evt-token=super-secret".into()),
            agent: "codex".into(),
            kind: EventKind::DesignThought,
            content: Some("This clean-looking claim has a tainted evidence id.".into()),
            ..Event::default()
        })
        .expect("event");

    let error = promote_memory_candidate(
        temp.path(),
        "mem-evt-token-super-secret",
        coding_agent_monitor::MemorySource::ManualReview,
    )
    .expect_err("tainted evidence id should not persist");

    assert!(error.to_string().contains("tainted"));
    assert!(!store.root().join("memories.jsonl").exists());
}

#[test]
fn promote_memory_candidate_rejects_already_governed_memory_id() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            event_id: Some("evt-design-retracted".into()),
            agent: "codex".into(),
            kind: EventKind::DesignThought,
            content: Some("Do not resurrect this old constraint by accident.".into()),
            ..Event::default()
        })
        .expect("event");
    store
        .append_memory(&coding_agent_monitor::MemoryCandidate {
            memory_id: "mem-evt-design-retracted".into(),
            scope: coding_agent_monitor::MemoryScope::Project,
            claim: "The old constraint was deprecated.".into(),
            status: coding_agent_monitor::MemoryStatus::Deprecated,
            source: coding_agent_monitor::MemorySource::ManualReview,
            evidence_ids: vec!["review-retract".into()],
            confidence: 95,
        })
        .expect("append deprecated memory");

    let error = promote_memory_candidate(
        temp.path(),
        "mem-evt-design-retracted",
        coding_agent_monitor::MemorySource::ManualReview,
    )
    .expect_err("governed memory id should not be resurrected");

    assert!(error.to_string().contains("already governed"));
    let memory_log =
        std::fs::read_to_string(store.root().join("memories.jsonl")).expect("memory log");
    assert_eq!(memory_log.lines().count(), 1);
    assert!(!memory_log.contains("\"status\":\"active\""));
}

#[test]
fn promote_memory_candidate_rejects_missing_current_candidate() {
    let temp = tempfile::tempdir().expect("temp dir");
    let store = ProjectStore::open(temp.path()).expect("store");

    let error = promote_memory_candidate(
        temp.path(),
        "mem-missing",
        coding_agent_monitor::MemorySource::ManualReview,
    )
    .expect_err("missing candidate should not be promoted");

    assert!(error.to_string().contains("memory candidate not found"));
    assert!(!store.root().join("memories.jsonl").exists());
}

#[test]
fn case_file_loads_active_verified_memory_separately_from_unverified_candidates() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_memory(&coding_agent_monitor::MemoryCandidate {
            memory_id: "mem-verified-roundtrip".into(),
            scope: coding_agent_monitor::MemoryScope::Project,
            claim: "Parser must preserve comments for formatter roundtrip.".into(),
            status: coding_agent_monitor::MemoryStatus::Active,
            source: coding_agent_monitor::MemorySource::VerifiedResult,
            evidence_ids: vec!["verifier-roundtrip".into()],
            confidence: 92,
        })
        .expect("append memory");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:01:00Z".into()),
            event_id: Some("evt-design".into()),
            agent: "codex".into(),
            kind: EventKind::DesignThought,
            content: Some("Temporary implementation note from current agent.".into()),
            ..Event::default()
        })
        .expect("event");

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);

    assert_eq!(case_file.durable_memory.len(), 1);
    assert_eq!(
        case_file.durable_memory[0].claim,
        "Parser must preserve comments for formatter roundtrip."
    );
    assert_eq!(
        case_file.durable_memory[0].source,
        coding_agent_monitor::MemorySource::VerifiedResult
    );
    assert_eq!(case_file.memory_candidates.len(), 1);
    assert_eq!(
        case_file.memory_candidates[0].status,
        coding_agent_monitor::MemoryStatus::Unverified
    );
}

#[test]
fn case_file_mirrors_active_durable_memory_into_requirement_nodes() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_memory(&coding_agent_monitor::MemoryCandidate {
            memory_id: "mem-adapter-constraint".into(),
            scope: coding_agent_monitor::MemoryScope::Project,
            claim: "Adapters must support Codex, Claude Code, Pi, and OpenCode.".into(),
            status: coding_agent_monitor::MemoryStatus::Active,
            source: coding_agent_monitor::MemorySource::ManualReview,
            evidence_ids: vec!["review-adapter-constraint".into()],
            confidence: 93,
        })
        .expect("append memory");

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);

    let memory_requirement = case_file
        .requirements
        .iter()
        .find(|requirement| requirement.source == RequirementSource::DurableMemory)
        .expect("durable memory requirement");
    assert!(
        memory_requirement
            .requirement_id
            .starts_with("req-memory-adapters-must-support-codex")
    );
    assert_eq!(memory_requirement.source, RequirementSource::DurableMemory);
    assert_eq!(
        memory_requirement.text,
        "Adapters must support Codex, Claude Code, Pi, and OpenCode."
    );
    assert_eq!(
        memory_requirement.evidence_ids,
        vec!["review-adapter-constraint"]
    );
    assert_eq!(
        memory_requirement.source_event_id.as_deref(),
        Some("review-adapter-constraint")
    );
    assert_eq!(
        memory_requirement.status,
        coding_agent_monitor::AcceptanceCoverageStatus::Covered
    );
    assert!(memory_requirement.verifier_ids.is_empty());
}

#[test]
fn case_file_does_not_promote_agent_claim_memory_as_durable() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_memory(&coding_agent_monitor::MemoryCandidate {
            memory_id: "mem-agent-claim".into(),
            scope: coding_agent_monitor::MemoryScope::Project,
            claim: "Agent claimed this should be durable.".into(),
            status: coding_agent_monitor::MemoryStatus::Active,
            source: coding_agent_monitor::MemorySource::AgentClaim,
            evidence_ids: vec!["evt-agent-claim".into()],
            confidence: 95,
        })
        .expect("append memory");

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);

    assert!(case_file.durable_memory.is_empty());
}

#[test]
fn case_file_filters_secret_like_durable_memory_before_packets() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_memory(&coding_agent_monitor::MemoryCandidate {
            memory_id: "mem-secret".into(),
            scope: coding_agent_monitor::MemoryScope::Project,
            claim: "Never expose api_key=memory-secret-value".into(),
            status: coding_agent_monitor::MemoryStatus::Active,
            source: coding_agent_monitor::MemorySource::User,
            evidence_ids: vec!["evt-user".into()],
            confidence: 99,
        })
        .expect("append memory");

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);

    assert!(case_file.durable_memory.is_empty());
}

#[test]
fn case_file_keeps_valid_durable_memory_when_one_memory_line_is_malformed() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_memory(&coding_agent_monitor::MemoryCandidate {
            memory_id: "mem-valid-before".into(),
            scope: coding_agent_monitor::MemoryScope::Project,
            claim: "Keep the validated parser roundtrip constraint.".into(),
            status: coding_agent_monitor::MemoryStatus::Active,
            source: coding_agent_monitor::MemorySource::VerifiedResult,
            evidence_ids: vec!["verifier-roundtrip".into()],
            confidence: 92,
        })
        .expect("append first memory");
    std::fs::OpenOptions::new()
        .append(true)
        .open(temp.path().join(".agent-monitor/memories.jsonl"))
        .expect("open memory log")
        .write_all(br#"{"memory_id":"broken"}"#)
        .expect("write malformed memory");
    std::fs::OpenOptions::new()
        .append(true)
        .open(temp.path().join(".agent-monitor/memories.jsonl"))
        .expect("open memory log")
        .write_all(b"\n")
        .expect("terminate malformed memory");
    store
        .append_memory(&coding_agent_monitor::MemoryCandidate {
            memory_id: "mem-valid-after".into(),
            scope: coding_agent_monitor::MemoryScope::Project,
            claim: "Keep the manually reviewed adapter constraint.".into(),
            status: coding_agent_monitor::MemoryStatus::Active,
            source: coding_agent_monitor::MemorySource::ManualReview,
            evidence_ids: vec!["review-adapter".into()],
            confidence: 90,
        })
        .expect("append second memory");

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);

    let claims = case_file
        .durable_memory
        .iter()
        .map(|memory| memory.claim.as_str())
        .collect::<Vec<_>>();
    assert_eq!(claims.len(), 2);
    assert!(claims.contains(&"Keep the validated parser roundtrip constraint."));
    assert!(claims.contains(&"Keep the manually reviewed adapter constraint."));
    assert!(case_file.evidence.iter().any(|evidence| {
        evidence.kind == "memory_load_warning" && evidence.summary.contains("line 2")
    }));
}

#[test]
fn case_file_uses_latest_memory_record_status_for_append_only_governance() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_memory(&coding_agent_monitor::MemoryCandidate {
            memory_id: "mem-retracted".into(),
            scope: coding_agent_monitor::MemoryScope::Project,
            claim: "This older active memory was later retracted.".into(),
            status: coding_agent_monitor::MemoryStatus::Active,
            source: coding_agent_monitor::MemorySource::User,
            evidence_ids: vec!["evt-user-old".into()],
            confidence: 90,
        })
        .expect("append active memory");
    store
        .append_memory(&coding_agent_monitor::MemoryCandidate {
            memory_id: "mem-still-active".into(),
            scope: coding_agent_monitor::MemoryScope::Project,
            claim: "This newer active memory should remain.".into(),
            status: coding_agent_monitor::MemoryStatus::Active,
            source: coding_agent_monitor::MemorySource::User,
            evidence_ids: vec!["evt-user-new".into()],
            confidence: 91,
        })
        .expect("append active memory");
    store
        .append_memory(&coding_agent_monitor::MemoryCandidate {
            memory_id: "mem-retracted".into(),
            scope: coding_agent_monitor::MemoryScope::Project,
            claim: "The old parser constraint is deprecated.".into(),
            status: coding_agent_monitor::MemoryStatus::Deprecated,
            source: coding_agent_monitor::MemorySource::ManualReview,
            evidence_ids: vec!["review-retract".into()],
            confidence: 95,
        })
        .expect("append deprecated memory");

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);

    assert_eq!(case_file.durable_memory.len(), 1);
    assert_eq!(
        case_file.durable_memory[0].claim,
        "This newer active memory should remain."
    );
}

#[test]
fn case_file_quarantines_conflicting_active_memory_across_ids() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_memory(&coding_agent_monitor::MemoryCandidate {
            memory_id: "mem-no-electron".into(),
            scope: coding_agent_monitor::MemoryScope::Project,
            claim: "Do not use Electron for the dashboard.".into(),
            status: coding_agent_monitor::MemoryStatus::Active,
            source: coding_agent_monitor::MemorySource::User,
            evidence_ids: vec!["evt-no-electron".into()],
            confidence: 95,
        })
        .expect("append deny memory");
    store
        .append_memory(&coding_agent_monitor::MemoryCandidate {
            memory_id: "mem-use-electron".into(),
            scope: coding_agent_monitor::MemoryScope::Project,
            claim: "Use Electron for the dashboard.".into(),
            status: coding_agent_monitor::MemoryStatus::Active,
            source: coding_agent_monitor::MemorySource::ManualReview,
            evidence_ids: vec!["review-use-electron".into()],
            confidence: 90,
        })
        .expect("append allow memory");
    store
        .append_memory(&coding_agent_monitor::MemoryCandidate {
            memory_id: "mem-adapters".into(),
            scope: coding_agent_monitor::MemoryScope::Project,
            claim: "Adapters must support Codex, Claude Code, Pi, and OpenCode.".into(),
            status: coding_agent_monitor::MemoryStatus::Active,
            source: coding_agent_monitor::MemorySource::ManualReview,
            evidence_ids: vec!["review-adapters".into()],
            confidence: 93,
        })
        .expect("append unrelated memory");

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);
    let claims = case_file
        .durable_memory
        .iter()
        .map(|memory| memory.claim.as_str())
        .collect::<Vec<_>>();

    assert_eq!(
        claims,
        vec!["Adapters must support Codex, Claude Code, Pi, and OpenCode."]
    );
    assert!(case_file.evidence.iter().any(|evidence| {
        evidence.kind == "memory_conflict"
            && evidence.summary.contains("mem-no-electron")
            && evidence.summary.contains("mem-use-electron")
    }));
}

#[test]
fn promote_memory_candidate_rejects_cross_id_conflicting_durable_claim() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_memory(&coding_agent_monitor::MemoryCandidate {
            memory_id: "mem-no-electron".into(),
            scope: coding_agent_monitor::MemoryScope::Project,
            claim: "Do not use Electron for the dashboard.".into(),
            status: coding_agent_monitor::MemoryStatus::Active,
            source: coding_agent_monitor::MemorySource::User,
            evidence_ids: vec!["evt-no-electron".into()],
            confidence: 95,
        })
        .expect("append existing memory");
    store
        .append_event(&Event {
            event_id: Some("evt-use-electron".into()),
            agent: "user".into(),
            kind: EventKind::UserInstruction,
            content: Some("Remember: Use Electron for the dashboard.".into()),
            ..Event::default()
        })
        .expect("event");

    let error = promote_memory_candidate(
        temp.path(),
        "mem-evt-use-electron",
        coding_agent_monitor::MemorySource::User,
    )
    .expect_err("conflicting durable claim should require governance");

    assert!(error.to_string().contains("conflict"));
    let memory_log =
        std::fs::read_to_string(store.root().join("memories.jsonl")).expect("memory log");
    assert_eq!(memory_log.lines().count(), 1);
    assert!(!memory_log.contains("mem-evt-use-electron"));
}

#[test]
fn case_file_keeps_newest_durable_memories_when_memory_log_exceeds_packet_limit() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    for index in 0..25 {
        store
            .append_memory(&coding_agent_monitor::MemoryCandidate {
                memory_id: format!("mem-{index:02}"),
                scope: coding_agent_monitor::MemoryScope::Project,
                claim: format!("Durable memory {index:02}"),
                status: coding_agent_monitor::MemoryStatus::Active,
                source: coding_agent_monitor::MemorySource::User,
                evidence_ids: vec![format!("evt-user-{index:02}")],
                confidence: 80,
            })
            .expect("append memory");
    }

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);

    assert_eq!(case_file.durable_memory.len(), 20);
    assert_eq!(case_file.durable_memory[0].claim, "Durable memory 24");
    assert_eq!(case_file.durable_memory[19].claim, "Durable memory 05");
    assert!(
        !case_file
            .durable_memory
            .iter()
            .any(|memory| memory.claim == "Durable memory 00")
    );
}

#[test]
fn case_file_redacts_secret_like_evidence_before_advisor_use() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            event_id: Some("evt-secret".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some("request failed with Authorization: Bearer sk-test-secret-token".into()),
            ..Event::default()
        })
        .expect("event");

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);
    let evidence = case_file
        .evidence
        .iter()
        .find(|item| item.id == "evt-secret")
        .expect("secret evidence");

    assert_eq!(evidence.redaction_status, RedactionStatus::Redacted);
    assert!(!evidence.summary.contains("sk-test-secret-token"));
    assert!(evidence.summary.contains("[REDACTED]"));
}

#[test]
fn case_file_includes_persisted_dev_history_findings_as_bounded_evidence() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_dev_history_report(&DevHistoryReport {
            workspace: temp.path().display().to_string(),
            generated_at: "2026-06-24T02:17:50Z".into(),
            sources: vec![DevHistorySourceReport {
                source: "claude-code".into(),
                history_root: "C:/Users/yys/.claude/projects".into(),
                files: 432,
                bytes: 369_301_667,
                lines: 77_843,
                parsed: 77_842,
                sessions: 4,
                first_time: Some("2026-06-13T01:48:55Z".into()),
                last_time: Some("2026-06-22T10:35:26Z".into()),
                subagent_files: Some(428),
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
                summary: "History shows verification-heavy work with stale verifier risk.".into(),
                evidence: vec!["14136 verification or unverified-stop signals".into()],
                monitor_response: vec!["Track verifier freshness before continue.".into()],
            }],
        })
        .expect("dev history");

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    assert_eq!(snapshot.dev_history_count, 1);
    let case_file = build_control_case_file(temp.path(), &snapshot);
    let evidence = case_file
        .evidence
        .iter()
        .find(|item| {
            item.kind == "DevHistoryFinding" && item.summary.contains("verification_entropy")
        })
        .expect("dev-history evidence");

    assert!(evidence.id.starts_with("dev-history-verification_entropy-"));
    assert_eq!(evidence.kind, "DevHistoryFinding");
    assert_eq!(evidence.source_type.as_deref(), Some("dev_history"));
    assert_eq!(evidence.source_path.as_deref(), Some("dev-history.jsonl"));
    assert!(evidence.summary.contains("verification-heavy"));
    assert!(evidence.summary.contains("14136 verification"));
    assert!(!evidence.summary.contains("Track verifier freshness"));
    assert_eq!(evidence.redaction_status, RedactionStatus::Clean);
}

#[test]
fn persisted_dev_history_finding_evidence_id_is_stable_across_snapshot_limits() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_dev_history_report(&DevHistoryReport {
            workspace: temp.path().display().to_string(),
            generated_at: "2026-06-24T01:00:00Z".into(),
            sources: Vec::new(),
            findings: vec![DevHistoryFinding {
                kind: "verification_entropy".into(),
                severity: "warning".into(),
                summary: "Older history report.".into(),
                evidence: vec!["1 verification signal".into()],
                monitor_response: vec!["Track verifier freshness.".into()],
            }],
        })
        .expect("older dev history");
    store
        .append_dev_history_report(&DevHistoryReport {
            workspace: temp.path().display().to_string(),
            generated_at: "2026-06-24T02:00:00Z".into(),
            sources: Vec::new(),
            findings: vec![DevHistoryFinding {
                kind: "verification_entropy".into(),
                severity: "critical".into(),
                summary: "Newer stable-id target history report.".into(),
                evidence: vec!["2 verification signals".into()],
                monitor_response: vec!["Track verifier freshness.".into()],
            }],
        })
        .expect("newer dev history");

    let snapshot_limit_1 = DashboardSnapshot::load(store.root(), 1).expect("snapshot limit 1");
    let case_limit_1 = build_control_case_file(temp.path(), &snapshot_limit_1);
    let id_limit_1 = case_limit_1
        .evidence
        .iter()
        .find(|item| item.summary.contains("Newer stable-id target"))
        .map(|item| item.id.clone())
        .expect("newer dev-history evidence with limit 1");

    let snapshot_limit_2 = DashboardSnapshot::load(store.root(), 2).expect("snapshot limit 2");
    let case_limit_2 = build_control_case_file(temp.path(), &snapshot_limit_2);
    let id_limit_2 = case_limit_2
        .evidence
        .iter()
        .find(|item| item.summary.contains("Newer stable-id target"))
        .map(|item| item.id.clone())
        .expect("newer dev-history evidence with limit 2");

    assert_eq!(
        id_limit_1, id_limit_2,
        "same persisted dev-history finding should keep its evidence id across bounded snapshot windows"
    );
}

#[test]
fn persisted_dev_history_findings_raise_only_conservative_entropy_priors() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_dev_history_report(&DevHistoryReport {
            workspace: temp.path().display().to_string(),
            generated_at: "2026-06-24T02:17:50Z".into(),
            sources: vec![DevHistorySourceReport {
                source: "claude-code".into(),
                history_root: "C:/Users/yys/.claude/projects".into(),
                files: 432,
                bytes: 369_301_667,
                lines: 77_843,
                parsed: 77_842,
                sessions: 4,
                first_time: Some("2026-06-13T01:48:55Z".into()),
                last_time: Some("2026-06-22T10:35:26Z".into()),
                subagent_files: Some(428),
                top_types: Vec::new(),
                top_payload_types: Vec::new(),
                top_content_types: Vec::new(),
                top_tools: Vec::new(),
                top_command_heads: Vec::new(),
                top_signals: Vec::new(),
                top_file_refs: Vec::new(),
            }],
            findings: vec![
                DevHistoryFinding {
                    kind: "verification_entropy".into(),
                    severity: "critical".into(),
                    summary: "History shows verification-heavy work with stale verifier risk."
                        .into(),
                    evidence: vec!["14136 verification or unverified-stop signals".into()],
                    monitor_response: vec!["Track verifier freshness before continue.".into()],
                },
                DevHistoryFinding {
                    kind: "agent_health_entropy".into(),
                    severity: "warning".into(),
                    summary: "History contains service and context-instability signals.".into(),
                    evidence: vec!["86 instability signals".into()],
                    monitor_response: vec![
                        "Retry transient failures before switching agents.".into(),
                    ],
                },
                DevHistoryFinding {
                    kind: "user_interrupt_entropy".into(),
                    severity: "warning".into(),
                    summary: "History includes avoidable agent questions.".into(),
                    evidence: vec!["34 agent-question signals".into()],
                    monitor_response: vec!["Gate AskUser behind local evidence probes.".into()],
                },
                DevHistoryFinding {
                    kind: "subagent_lifecycle_entropy".into(),
                    severity: "warning".into(),
                    summary: "History shows subagent fan-out without enough observable joins."
                        .into(),
                    evidence: vec![
                        "Codex lifecycle tool counts: spawn_agent=69, close_agent=29, wait_agent=4"
                            .into(),
                    ],
                    monitor_response: vec![
                        "Require terminal worker outcomes before more fan-out.".into(),
                    ],
                },
                DevHistoryFinding {
                    kind: "blame_hotspots".into(),
                    severity: "info".into(),
                    summary: "History identifies repeated blame targets.".into(),
                    evidence: vec!["src/lib.rs (42)".into()],
                    monitor_response: vec!["Link imported history to hunk evidence ids.".into()],
                },
            ],
        })
        .expect("dev history");

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);
    let verification_evidence_id = dev_history_evidence_id(&case_file, "verification_entropy");
    let agent_health_evidence_id = dev_history_evidence_id(&case_file, "agent_health_entropy");
    let user_interrupt_evidence_id = dev_history_evidence_id(&case_file, "user_interrupt_entropy");
    let subagent_lifecycle_evidence_id =
        dev_history_evidence_id(&case_file, "subagent_lifecycle_entropy");
    let blame_hotspots_evidence_id = dev_history_evidence_id(&case_file, "blame_hotspots");
    let verification = case_file
        .entropy
        .score(EntropyKind::Verification)
        .expect("verification entropy");
    assert!(
        (50..75).contains(&verification.score),
        "history-only verification prior should stay below force-verification threshold: {verification:?}"
    );
    assert!(
        verification
            .evidence_ids
            .contains(&verification_evidence_id),
        "history prior should cite bounded dev-history evidence: {verification:?}"
    );
    assert!(
        verification
            .top_causes
            .iter()
            .any(|cause| cause.contains("local dev-history") && cause.contains("verification")),
        "history prior should explain that it is historical evidence: {verification:?}"
    );

    let agent_health = case_file
        .entropy
        .score(EntropyKind::AgentHealth)
        .expect("agent health entropy");
    assert!(
        (45..75).contains(&agent_health.score),
        "history-only agent-health prior should stay below retry/switch threshold: {agent_health:?}"
    );
    assert!(
        agent_health
            .evidence_ids
            .contains(&agent_health_evidence_id)
    );

    let plan = case_file
        .entropy
        .score(EntropyKind::Plan)
        .expect("plan entropy");
    assert!(
        (45..60).contains(&plan.score),
        "history-only subagent lifecycle prior should stay below follow-up threshold: {plan:?}"
    );
    assert!(
        plan.evidence_ids.contains(&subagent_lifecycle_evidence_id),
        "subagent lifecycle prior should cite bounded dev-history evidence: {plan:?}"
    );
    assert!(
        case_file.belief_state.hypotheses.iter().any(|belief| {
            belief.kind == coding_agent_monitor::FailureHypothesisKind::ProcessConformanceGap
                && belief
                    .evidence_ids
                    .contains(&subagent_lifecycle_evidence_id)
        }),
        "belief state should include history-derived process conformance risk: {:?}",
        case_file.belief_state
    );

    let user_decision = case_file
        .entropy
        .score(EntropyKind::UserDecision)
        .expect("user decision entropy");
    assert_eq!(
        user_decision.score, 0,
        "history-only user-interrupt evidence must not raise AskUser entropy: {user_decision:?}"
    );
    assert!(
        !user_decision
            .evidence_ids
            .contains(&user_interrupt_evidence_id)
    );

    let repo_blame = case_file
        .entropy
        .score(EntropyKind::RepoBlame)
        .expect("repo/blame entropy");
    assert_eq!(
        repo_blame.score, 0,
        "history-only blame hotspots without current overlap should not raise repo/blame entropy: {repo_blame:?}"
    );
    assert!(
        !repo_blame
            .evidence_ids
            .contains(&blame_hotspots_evidence_id)
    );

    let ask_outcome = validate_control_action_detailed(
        ControlAction::AskUser {
            question: "Should I continue?".into(),
        },
        &case_file,
    );
    assert!(
        matches!(ask_outcome, ValidationOutcome::Modified { .. }),
        "history-only prior must not approve AskUser: {ask_outcome:?}"
    );

    let handoff_outcome = validate_control_action_detailed(
        ControlAction::SwitchAgent {
            target_agent: "claude-code".into(),
        },
        &case_file,
    );
    assert!(
        matches!(handoff_outcome, ValidationOutcome::Modified { .. }),
        "history-only prior must not approve switching agents: {handoff_outcome:?}"
    );

    let evidence_ids = case_file
        .evidence
        .iter()
        .map(|evidence| evidence.id.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    for score in &case_file.entropy.scores {
        for evidence_id in &score.evidence_ids {
            assert!(
                evidence_ids.contains(evidence_id.as_str()),
                "entropy evidence ref should resolve in case-file evidence: {evidence_id}"
            );
        }
    }
}

#[test]
fn dev_history_blame_hotspot_prior_requires_current_changed_file_overlap() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            event_id: Some("evt-current-hotspot".into()),
            time: Some("2026-06-24T02:20:00Z".into()),
            agent: "codex".into(),
            kind: EventKind::FileChange,
            file: Some("src/lib.rs".into()),
            rationale: Some("Update monitor entropy scoring.".into()),
            ..Event::default()
        })
        .expect("event");
    store
        .append_dev_history_report(&DevHistoryReport {
            workspace: temp.path().display().to_string(),
            generated_at: "2026-06-24T02:17:50Z".into(),
            sources: Vec::new(),
            findings: vec![DevHistoryFinding {
                kind: "blame_hotspots".into(),
                severity: "info".into(),
                summary: "History identifies repeated blame targets.".into(),
                evidence: vec!["src/lib.rs (42)".into()],
                monitor_response: vec!["Link imported history to hunk evidence ids.".into()],
            }],
        })
        .expect("dev history");

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);
    let repo_blame = case_file
        .entropy
        .score(EntropyKind::RepoBlame)
        .expect("repo/blame entropy");

    assert!(
        (45..75).contains(&repo_blame.score),
        "overlapping hotspot prior should stay below follow-up threshold: {repo_blame:?}"
    );
    assert!(
        repo_blame
            .evidence_ids
            .contains(&dev_history_evidence_id(&case_file, "blame_hotspots"))
    );
    assert!(
        repo_blame
            .top_causes
            .iter()
            .any(|cause| cause.contains("local dev-history") && cause.contains("repo/blame")),
        "hotspot prior should explain that it is historical evidence: {repo_blame:?}"
    );
}

#[test]
fn legacy_evidence_without_redaction_status_deserializes_as_clean() {
    let evidence: coding_agent_monitor::EvidenceItem = serde_json::from_value(json!({
        "id": "evt-legacy",
        "kind": "event",
        "summary": "Legacy evidence before redaction tracking."
    }))
    .expect("legacy evidence");

    assert_eq!(evidence.redaction_status, RedactionStatus::Clean);
}

#[test]
fn legacy_case_file_without_verification_sections_deserializes_with_safe_defaults() {
    let temp = tempfile::tempdir().expect("temp dir");
    let store = ProjectStore::open(temp.path()).expect("store");
    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);
    let mut legacy_case_file = serde_json::to_value(&case_file).expect("case file json");
    let legacy_object = legacy_case_file.as_object_mut().expect("case file object");
    legacy_object.remove("latest_verification_status");
    legacy_object.remove("verification");
    legacy_object.remove("requirements");
    legacy_object.remove("memory_candidates");
    legacy_object.remove("durable_memory");

    let decoded: coding_agent_monitor::ControlCaseFile =
        serde_json::from_value(legacy_case_file).expect("legacy case file");

    assert_eq!(
        decoded.latest_verification_status,
        coding_agent_monitor::VerificationStatus::NotRun
    );
    assert_eq!(
        decoded.verification.status,
        coding_agent_monitor::VerificationStatus::NotRun
    );
    assert!(decoded.requirements.is_empty());
    assert!(decoded.memory_candidates.is_empty());
    assert!(decoded.durable_memory.is_empty());
}

#[path = "entropy_control/advisor_validation.rs"]
mod advisor_validation;

#[path = "entropy_control/policy_user_decision.rs"]
mod policy_user_decision;

#[test]
fn advisor_request_forbids_high_cost_handoffs_when_entropy_is_low() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            event_id: Some("evt-normal-progress".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some("Continuing with the next implementation step.".into()),
            ..Event::default()
        })
        .expect("event");
    let decision = json!({
        "diagnosis_id": "diagnosis-low-handoff-entropy",
        "dominant_entropy": "plan",
        "entropy_scores": {
            "plan": { "score": 25, "confidence": 80 }
        },
        "top_evidence": [],
        "cited_evidence_ids": [],
        "missing_evidence": [],
        "proposed_action": { "type": "continue_working" },
        "expected_entropy_delta": [],
        "packet_intent": "continue ordinary progress",
        "packet_draft": {
            "urgency": "follow_up",
            "summary": "Continue ordinary progress.",
            "instructions": ["Continue with the current agent."],
            "evidence_refs": []
        },
        "ask_user": null,
        "confidence": 0.7
    });
    let (endpoint, request_rx) = serve_advisor_once(decision);
    let env_name = "CAM_TEST_ADVISOR_KEY_LOW_HANDOFF_ENTROPY_PRUNED";
    set_test_env_var(env_name, "test-key");
    std::fs::write(
        store.root().join("config.json"),
        json!({
            "advisor": {
                "enabled": true,
                "provider": {
                    "endpoint": endpoint,
                    "model": "test-advisor",
                    "api_key_env": env_name,
                    "timeout_secs": 5
                }
            }
        })
        .to_string(),
    )
    .expect("config");
    drop(store);

    let advice = advise_workspace(temp.path()).expect("advice");
    let request = request_rx.recv().expect("advisor request");
    let case_file = advisor_request_case_file(&request);
    let allowed = case_file_action_values(&case_file, "allowed_actions");

    assert!(advice.advisor_used);
    assert!(!allowed.contains(&"switch_agent"));
    assert!(!allowed.contains(&"spawn_fresh_agent"));
    let forbidden = case_file
        .get("forbidden_actions")
        .and_then(serde_json::Value::as_array)
        .expect("forbidden actions");
    assert!(forbidden.iter().any(|action| {
        action.get("action").and_then(serde_json::Value::as_str) == Some("switch_agent")
            && action
                .get("reason")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|reason| reason.contains("agent-health entropy"))
    }));
    assert!(forbidden.iter().any(|action| {
        action.get("action").and_then(serde_json::Value::as_str) == Some("spawn_fresh_agent")
            && action
                .get("reason")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|reason| reason.contains("context or agent-health entropy"))
    }));
}

#[test]
fn advisor_request_forbids_handoffs_when_verification_entropy_is_high() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            event_id: Some("evt-advisor-verification-before-handoff".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some("Implementation complete; tests should pass now.".into()),
            ..Event::default()
        })
        .expect("event");
    let decision = json!({
        "diagnosis_id": "diagnosis-verification-first-handoff",
        "dominant_entropy": "verification",
        "entropy_scores": {
            "verification": { "score": 90, "confidence": 90 }
        },
        "top_evidence": [],
        "cited_evidence_ids": [],
        "missing_evidence": [],
        "proposed_action": { "type": "force_verification", "suite": "full", "blocking": true },
        "expected_entropy_delta": [{ "kind": "verification", "delta": -60 }],
        "packet_intent": "force verification before handoff",
        "packet_draft": {
            "urgency": "verification",
            "summary": "Verify before any handoff.",
            "instructions": ["Run the mapped verifier before switching agents."],
            "evidence_refs": []
        },
        "ask_user": null,
        "confidence": 0.8
    });
    let (endpoint, request_rx) = serve_advisor_once(decision);
    let env_name = "CAM_TEST_ADVISOR_KEY_VERIFICATION_FIRST_HANDOFFS";
    set_test_env_var(env_name, "test-key");
    std::fs::write(
        store.root().join("config.json"),
        json!({
            "advisor": {
                "enabled": true,
                "provider": {
                    "endpoint": endpoint,
                    "model": "test-advisor",
                    "api_key_env": env_name,
                    "timeout_secs": 5
                }
            }
        })
        .to_string(),
    )
    .expect("config");
    drop(store);

    let advice = advise_workspace(temp.path()).expect("advice");
    let request = request_rx.recv().expect("advisor request");
    let case_file = advisor_request_case_file(&request);
    let allowed = case_file_action_values(&case_file, "allowed_actions");

    assert!(matches!(
        advice.final_action,
        ControlAction::ForceVerification { .. }
    ));
    assert!(!allowed.contains(&"switch_agent"));
    assert!(!allowed.contains(&"spawn_fresh_agent"));
    let forbidden = case_file
        .get("forbidden_actions")
        .and_then(serde_json::Value::as_array)
        .expect("forbidden actions");
    for action_name in ["switch_agent", "spawn_fresh_agent"] {
        assert!(forbidden.iter().any(|action| {
            action.get("action").and_then(serde_json::Value::as_str) == Some(action_name)
                && action
                    .get("reason")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|reason| reason.contains("verification entropy is high"))
        }));
    }
}

#[test]
fn advisor_request_forbids_retry_agent_when_agent_health_entropy_is_low() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            event_id: Some("evt-normal-progress-retry".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some("Continuing with the next implementation step.".into()),
            ..Event::default()
        })
        .expect("event");
    let decision = json!({
        "diagnosis_id": "diagnosis-low-retry-entropy",
        "dominant_entropy": "plan",
        "entropy_scores": {
            "plan": { "score": 25, "confidence": 80 }
        },
        "top_evidence": [],
        "cited_evidence_ids": [],
        "missing_evidence": [],
        "proposed_action": { "type": "continue_working" },
        "expected_entropy_delta": [],
        "packet_intent": "continue ordinary progress",
        "packet_draft": {
            "urgency": "follow_up",
            "summary": "Continue ordinary progress.",
            "instructions": ["Continue with the current agent."],
            "evidence_refs": []
        },
        "ask_user": null,
        "confidence": 0.7
    });
    let (endpoint, request_rx) = serve_advisor_once(decision);
    let env_name = "CAM_TEST_ADVISOR_KEY_LOW_RETRY_ENTROPY_PRUNED";
    set_test_env_var(env_name, "test-key");
    std::fs::write(
        store.root().join("config.json"),
        json!({
            "advisor": {
                "enabled": true,
                "provider": {
                    "endpoint": endpoint,
                    "model": "test-advisor",
                    "api_key_env": env_name,
                    "timeout_secs": 5
                }
            }
        })
        .to_string(),
    )
    .expect("config");
    drop(store);

    let advice = advise_workspace(temp.path()).expect("advice");
    let request = request_rx.recv().expect("advisor request");
    let case_file = advisor_request_case_file(&request);
    let allowed = case_file_action_values(&case_file, "allowed_actions");

    assert!(advice.advisor_used);
    assert!(!allowed.contains(&"retry_agent"));
    let forbidden = case_file
        .get("forbidden_actions")
        .and_then(serde_json::Value::as_array)
        .expect("forbidden actions");
    assert!(forbidden.iter().any(|action| {
        action.get("action").and_then(serde_json::Value::as_str) == Some("retry_agent")
            && action
                .get("reason")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|reason| reason.contains("agent-health entropy"))
    }));
}

#[test]
fn advisor_request_forbids_send_follow_up_when_entropy_is_low() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            event_id: Some("evt-normal-progress-follow-up".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some("Continuing with the next implementation step.".into()),
            ..Event::default()
        })
        .expect("event");
    let decision = json!({
        "diagnosis_id": "diagnosis-low-follow-up-entropy",
        "dominant_entropy": "plan",
        "entropy_scores": {
            "plan": { "score": 25, "confidence": 80 }
        },
        "top_evidence": [],
        "cited_evidence_ids": [],
        "missing_evidence": [],
        "proposed_action": { "type": "continue_working" },
        "expected_entropy_delta": [],
        "packet_intent": "continue ordinary progress",
        "packet_draft": {
            "urgency": "follow_up",
            "summary": "Continue ordinary progress.",
            "instructions": ["Continue with the current agent."],
            "evidence_refs": []
        },
        "ask_user": null,
        "confidence": 0.7
    });
    let (endpoint, request_rx) = serve_advisor_once(decision);
    let env_name = "CAM_TEST_ADVISOR_KEY_LOW_FOLLOW_UP_ENTROPY_PRUNED";
    set_test_env_var(env_name, "test-key");
    std::fs::write(
        store.root().join("config.json"),
        json!({
            "advisor": {
                "enabled": true,
                "provider": {
                    "endpoint": endpoint,
                    "model": "test-advisor",
                    "api_key_env": env_name,
                    "timeout_secs": 5
                }
            }
        })
        .to_string(),
    )
    .expect("config");
    drop(store);

    let advice = advise_workspace(temp.path()).expect("advice");
    let request = request_rx.recv().expect("advisor request");
    let case_file = advisor_request_case_file(&request);
    let allowed = case_file_action_values(&case_file, "allowed_actions");

    assert!(advice.advisor_used);
    assert!(!allowed.contains(&"send_follow_up"));
    assert!(!allowed.contains(&"spawn_judge_agent"));
    let forbidden = case_file
        .get("forbidden_actions")
        .and_then(serde_json::Value::as_array)
        .expect("forbidden actions");
    assert!(forbidden.iter().any(|action| {
        action.get("action").and_then(serde_json::Value::as_str) == Some("send_follow_up")
            && action
                .get("reason")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|reason| reason.contains("follow-up entropy"))
    }));
    assert!(forbidden.iter().any(|action| {
        action.get("action").and_then(serde_json::Value::as_str) == Some("spawn_judge_agent")
            && action
                .get("reason")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|reason| reason.contains("repo/blame entropy"))
    }));
}

#[test]
fn advise_workspace_accepts_endpoint_advisor_spawn_judge_when_repo_blame_is_high() {
    let temp = tempfile::tempdir().expect("temp dir");
    init_git_repo(temp.path());
    std::fs::create_dir_all(temp.path().join("src")).expect("src dir");
    std::fs::write(temp.path().join("src/lib.rs"), "fn one() {}\nfn two() {}\n").expect("source");
    run_git(temp.path(), ["add", "src/lib.rs"]);
    run_git(temp.path(), ["commit", "-m", "add source"]);
    std::fs::write(
        temp.path().join("src/lib.rs"),
        "fn one() {}\nfn two_changed() {}\n",
    )
    .expect("changed source");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_verifier_run(&coding_agent_monitor::VerifierRun {
            verifier_run_id: "verifier-run-before-judge".into(),
            verifier_id: Some("rust_full".into()),
            command: "cargo test".into(),
            status: VerificationRunStatus::Passed,
            started_at: "9999-01-01T00:00:00Z".into(),
            completed_at: Some("9999-01-01T00:00:01Z".into()),
            exit_code: Some(0),
            output_digest: "fnv1a64:cbf29ce484222325".into(),
            failure_class: None,
        })
        .expect("verifier run");
    let decision = json!({
        "diagnosis_id": "diagnosis-repo-blame-judge",
        "dominant_entropy": "repo_blame",
        "entropy_scores": {
            "repo_blame": { "score": 88, "confidence": 90 }
        },
        "top_evidence": [
            {
                "event_id": "repo-audit-src-lib-rs",
                "why_it_matters": "Dirty hunk lacks trace rationale."
            }
        ],
        "cited_evidence_ids": ["repo-audit-src-lib-rs"],
        "missing_evidence": ["independent read-only review of dirty hunk"],
        "proposed_action": {
            "type": "spawn_judge_agent",
            "target_agent": "claude-code"
        },
        "expected_entropy_delta": [
            { "kind": "repo_blame", "delta": -35 }
        ],
        "packet_intent": "read-only judge review",
        "packet_draft": {
            "urgency": "context",
            "summary": "Review suspicious dirty hunk without editing.",
            "instructions": ["Review the dirty hunk and report whether it should stay."],
            "evidence_refs": ["repo-audit-src-lib-rs"]
        },
        "ask_user": null,
        "confidence": 0.78
    });
    let (endpoint, request_rx) = serve_advisor_once(decision);
    let env_name = "CAM_TEST_ADVISOR_KEY_SPAWN_JUDGE";
    set_test_env_var(env_name, "test-key");
    std::fs::write(
        store.root().join("config.json"),
        json!({
            "advisor": {
                "enabled": true,
                "provider": {
                    "endpoint": endpoint,
                    "model": "test-advisor",
                    "api_key_env": env_name,
                    "timeout_secs": 5
                }
            }
        })
        .to_string(),
    )
    .expect("config");
    drop(store);

    let advice = advise_workspace(temp.path()).expect("advice");
    let request = request_rx.recv().expect("advisor request");
    let case_file = advisor_request_case_file(&request);
    let allowed = case_file_action_values(&case_file, "allowed_actions");

    assert!(allowed.contains(&"spawn_judge_agent"));
    assert_eq!(
        advice.final_action,
        ControlAction::SpawnJudgeAgent {
            target_agent: Some("claude-code".into())
        }
    );
    assert!(matches!(
        advice.validation_outcome,
        ValidationOutcome::Approved(ControlAction::SpawnJudgeAgent { .. })
    ));
    assert!(
        advice
            .packet
            .forbidden
            .iter()
            .any(|forbidden| forbidden.contains("Do not edit files"))
    );
}

#[test]
fn advise_workspace_normalizes_null_endpoint_advisor_spawn_judge_target_to_readonly_adapter() {
    let temp = tempfile::tempdir().expect("temp dir");
    init_git_repo(temp.path());
    std::fs::create_dir_all(temp.path().join("src")).expect("src dir");
    std::fs::write(temp.path().join("src/lib.rs"), "fn one() {}\nfn two() {}\n").expect("source");
    run_git(temp.path(), ["add", "src/lib.rs"]);
    run_git(temp.path(), ["commit", "-m", "add source"]);
    std::fs::write(
        temp.path().join("src/lib.rs"),
        "fn one() {}\nfn two_changed() {}\n",
    )
    .expect("changed source");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            event_id: Some("evt-active-pi-judge".into()),
            time: Some("9999-01-01T00:00:00Z".into()),
            agent: "pi".into(),
            kind: EventKind::ModelMessage,
            content: Some("Pi session is active.".into()),
            ..Event::default()
        })
        .expect("active pi event");
    store
        .append_verifier_run(&coding_agent_monitor::VerifierRun {
            verifier_run_id: "verifier-run-before-null-judge".into(),
            verifier_id: Some("rust_full".into()),
            command: "cargo test".into(),
            status: VerificationRunStatus::Passed,
            started_at: "9999-01-01T00:00:00Z".into(),
            completed_at: Some("9999-01-01T00:00:01Z".into()),
            exit_code: Some(0),
            output_digest: "fnv1a64:cbf29ce484222325".into(),
            failure_class: None,
        })
        .expect("verifier run");
    let decision = json!({
        "diagnosis_id": "diagnosis-null-target-repo-blame-judge",
        "dominant_entropy": "repo_blame",
        "entropy_scores": {
            "repo_blame": { "score": 88, "confidence": 90 }
        },
        "top_evidence": [
            {
                "event_id": "repo-audit-src-lib-rs",
                "why_it_matters": "Dirty hunk lacks trace rationale."
            }
        ],
        "cited_evidence_ids": ["repo-audit-src-lib-rs"],
        "missing_evidence": ["independent read-only review of dirty hunk"],
        "proposed_action": {
            "type": "spawn_judge_agent",
            "target_agent": null
        },
        "expected_entropy_delta": [
            { "kind": "repo_blame", "delta": -35 }
        ],
        "packet_intent": "read-only judge review",
        "packet_draft": {
            "urgency": "context",
            "summary": "Review suspicious dirty hunk without editing.",
            "instructions": ["Review the dirty hunk and report whether it should stay."],
            "evidence_refs": ["repo-audit-src-lib-rs"]
        },
        "ask_user": null,
        "confidence": 0.78
    });
    let (endpoint, request_rx) = serve_advisor_once(decision);
    let env_name = "CAM_TEST_ADVISOR_KEY_SPAWN_JUDGE_NULL_TARGET";
    set_test_env_var(env_name, "test-key");
    std::fs::write(
        store.root().join("config.json"),
        json!({
            "advisor": {
                "enabled": true,
                "provider": {
                    "endpoint": endpoint,
                    "model": "test-advisor",
                    "api_key_env": env_name,
                    "timeout_secs": 5
                }
            }
        })
        .to_string(),
    )
    .expect("config");
    drop(store);

    let advice = advise_workspace(temp.path()).expect("advice");
    let request = request_rx.recv().expect("advisor request");
    let case_file = advisor_request_case_file(&request);
    let allowed = case_file_action_values(&case_file, "allowed_actions");

    assert!(allowed.contains(&"spawn_judge_agent"));
    assert_eq!(
        advice.final_action,
        ControlAction::SpawnJudgeAgent {
            target_agent: Some("claude-code".into())
        }
    );
    assert_ne!(advice.packet.target_agent, "pi");
    assert_eq!(advice.packet.target_agent, "claude-code");
    assert!(
        !temp
            .path()
            .join(".agent-monitor/outbox/pi/latest.md")
            .exists()
    );
    assert!(
        temp.path()
            .join(".agent-monitor/outbox/claude-code/latest.md")
            .exists()
    );
}

#[test]
fn advise_workspace_accepts_valid_endpoint_advisor_proposal() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            event_id: Some("evt-plan".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some(
                "This is a good point to stop while obvious implementation work remains.".into(),
            ),
            ..Event::default()
        })
        .expect("event");
    let decision = json!({
        "diagnosis_id": "diagnosis-plan",
        "dominant_entropy": "plan",
        "entropy_scores": {
            "plan": { "score": 65, "confidence": 80 },
            "verification": { "score": 10, "confidence": 75 }
        },
        "top_evidence": [
            {
                "event_id": "evt-plan",
                "why_it_matters": "The agent has an obvious next step but no bounded packet."
            }
        ],
        "cited_evidence_ids": ["evt-plan"],
        "missing_evidence": ["bounded next-step packet"],
        "proposed_action": { "type": "send_follow_up", "target_agent": null },
        "expected_entropy_delta": [
            { "kind": "plan", "delta": -25 }
        ],
        "packet_intent": "require a bounded next step",
        "packet_draft": {
            "urgency": "follow_up",
            "summary": "Continue with one bounded next step.",
            "instructions": ["Take one concrete implementation step."],
            "evidence_refs": ["evt-plan"]
        },
        "ask_user": null,
        "confidence": 0.72
    });
    let (endpoint, request_rx) = serve_advisor_once(decision);
    let env_name = "CAM_TEST_ADVISOR_KEY_ACCEPTS_VALID";
    set_test_env_var(env_name, "test-key");
    std::fs::write(
        store.root().join("config.json"),
        json!({
            "advisor": {
                "enabled": true,
                "provider": {
                    "endpoint": endpoint,
                    "model": "test-advisor",
                    "api_key_env": env_name,
                    "timeout_secs": 5,
                    "max_output_tokens": 700
                }
            }
        })
        .to_string(),
    )
    .expect("config");

    let advice = advise_workspace(temp.path()).expect("advice");
    let request = request_rx.recv().expect("advisor request");

    assert!(advice.advisor_used);
    assert_eq!(advice.advisor_error, None);
    assert_eq!(
        advice.final_action,
        ControlAction::SendFollowUp { target_agent: None }
    );
    assert!(matches!(
        advice.validation_outcome,
        ValidationOutcome::Approved(ControlAction::SendFollowUp { .. })
    ));
    let advisor_decision = advice.advisor_decision.expect("advisor decision");
    assert_eq!(
        advisor_decision.diagnosis_id.as_deref(),
        Some("diagnosis-plan")
    );
    assert_eq!(
        advisor_decision.expected_entropy_delta,
        vec![EntropyDelta {
            kind: EntropyKind::Plan,
            delta: -25,
        }]
    );
    assert!(request.contains("Authorization: Bearer test-key"));
    assert!(request.contains("\"model\":\"test-advisor\""));
    assert!(request.contains("You are not the controller"));
    assert!(request.contains("entropy_scores"));
    assert!(request.contains("dominant_entropy must have a matching entropy_scores entry"));
    assert!(request.contains("Use belief_state hypotheses as diagnostic priors"));
    assert!(request.contains("Prefer force_verification or run_probe"));
    assert!(request.contains("expected_entropy_delta"));
    assert!(request.contains("-100..100"));
    assert!(request.contains("at most one expected_entropy_delta per entropy kind"));
    assert!(request.contains("explicit target agents"));
    assert!(request.contains("ask_user question text will be rewritten"));
    assert!(request.contains("evt-plan"));
}

#[test]
fn advise_workspace_uses_coding_plan_credential_file_for_endpoint_advisor() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    std::fs::create_dir_all(store.root().join("credentials").join("coding-plan"))
        .expect("credential dir");
    std::fs::write(
        store
            .root()
            .join("credentials")
            .join("coding-plan")
            .join("auth.json"),
        r#"{
          "OPENAI_API_KEY": "plan-key",
          "tokens": {
            "access_token": "oauth-token"
          }
        }"#,
    )
    .expect("credential file");
    store
        .append_event(&Event {
            event_id: Some("evt-plan-credential".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some(
                "This is a good point to stop while obvious implementation work remains.".into(),
            ),
            ..Event::default()
        })
        .expect("event");
    let decision = json!({
        "diagnosis_id": "diagnosis-plan-credential",
        "dominant_entropy": "plan",
        "entropy_scores": {
            "plan": { "score": 65, "confidence": 80 }
        },
        "top_evidence": [
            {
                "event_id": "evt-plan-credential",
                "why_it_matters": "The active agent stopped before the next implementation step."
            }
        ],
        "cited_evidence_ids": ["evt-plan-credential"],
        "missing_evidence": [],
        "proposed_action": { "type": "send_follow_up", "target_agent": null },
        "expected_entropy_delta": [
            { "kind": "plan", "delta": -25 }
        ],
        "packet_intent": "continue bounded implementation",
        "packet_draft": {
            "urgency": "follow_up",
            "summary": "Continue the current implementation.",
            "instructions": ["Take one concrete implementation step."],
            "evidence_refs": ["evt-plan-credential"]
        },
        "ask_user": null,
        "confidence": 0.75
    });
    let (endpoint, request_rx) = serve_advisor_once(decision);
    std::fs::write(
        store.root().join("config.json"),
        json!({
            "advisor": {
                "enabled": true,
                "provider": {
                    "endpoint": endpoint,
                    "model": "gpt-5.5",
                    "credential_source": "coding_plan",
                    "credential_file": "credentials/coding-plan/auth.json",
                    "timeout_secs": 5
                }
            }
        })
        .to_string(),
    )
    .expect("config");

    let advice = advise_workspace(temp.path()).expect("advice");
    let request = request_rx.recv().expect("advisor request");

    assert!(advice.advisor_used);
    assert_eq!(advice.advisor_error, None);
    assert!(request.contains("Authorization: Bearer plan-key"));
    assert!(!request.contains("oauth-token"));
}

#[test]
fn advise_workspace_uses_coding_plan_profile_api_key_for_endpoint_advisor() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    std::fs::create_dir_all(store.root().join("credentials").join("coding-plan"))
        .expect("credential dir");
    std::fs::write(
        store
            .root()
            .join("credentials")
            .join("coding-plan")
            .join("profile.json"),
        r#"{
          "api_key": "profile-plan-key"
        }"#,
    )
    .expect("credential profile");
    store
        .append_event(&Event {
            event_id: Some("evt-plan-profile-credential".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some(
                "This is a good point to stop while obvious implementation work remains.".into(),
            ),
            ..Event::default()
        })
        .expect("event");
    let decision = json!({
        "diagnosis_id": "diagnosis-plan-profile-credential",
        "dominant_entropy": "plan",
        "entropy_scores": {
            "plan": { "score": 65, "confidence": 80 }
        },
        "top_evidence": [
            {
                "event_id": "evt-plan-profile-credential",
                "why_it_matters": "The active agent stopped before the next implementation step."
            }
        ],
        "cited_evidence_ids": ["evt-plan-profile-credential"],
        "missing_evidence": [],
        "proposed_action": { "type": "send_follow_up", "target_agent": null },
        "expected_entropy_delta": [
            { "kind": "plan", "delta": -25 }
        ],
        "packet_intent": "continue bounded implementation",
        "packet_draft": {
            "urgency": "follow_up",
            "summary": "Continue the current implementation.",
            "instructions": ["Take one concrete implementation step."],
            "evidence_refs": ["evt-plan-profile-credential"]
        },
        "ask_user": null,
        "confidence": 0.75
    });
    let (endpoint, request_rx) = serve_advisor_once(decision);
    std::fs::write(
        store.root().join("config.json"),
        json!({
            "advisor": {
                "enabled": true,
                "provider": {
                    "endpoint": endpoint,
                    "model": "gpt-5.5",
                    "credential_source": "coding_plan",
                    "credential_file": "credentials/coding-plan/profile.json",
                    "timeout_secs": 5
                }
            }
        })
        .to_string(),
    )
    .expect("config");

    let advice = advise_workspace(temp.path()).expect("advice");

    assert!(advice.advisor_used);
    assert_eq!(advice.advisor_error, None);
    let request = request_rx
        .recv_timeout(std::time::Duration::from_secs(1))
        .expect("advisor request");
    assert!(request.contains("Authorization: Bearer profile-plan-key"));
}

#[test]
fn advise_workspace_rejects_jwt_coding_plan_token_for_public_openai_endpoint_before_transport() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    std::fs::create_dir_all(store.root().join("credentials").join("coding-plan"))
        .expect("credential dir");
    std::fs::write(
        store
            .root()
            .join("credentials")
            .join("coding-plan")
            .join("auth.json"),
        r#"{
          "OPENAI_API_KEY": "eyJhbGciOiJSUzI1NiJ9.eyJhdWQiOiJjb2RpbmctcGxhbiJ9.signature"
        }"#,
    )
    .expect("credential file");
    store
        .append_event(&Event {
            event_id: Some("evt-jwt-plan-credential".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some(
                "This is a good point to stop while obvious implementation work remains.".into(),
            ),
            ..Event::default()
        })
        .expect("event");
    std::fs::write(
        store.root().join("config.json"),
        json!({
            "advisor": {
                "enabled": true,
                "provider": {
                    "endpoint": "https://api.openai.com/v1/chat/completions",
                    "model": "gpt-5.5",
                    "credential_source": "coding_plan",
                    "credential_file": "credentials/coding-plan/auth.json",
                    "timeout_secs": 5
                }
            }
        })
        .to_string(),
    )
    .expect("config");

    let advice = advise_workspace(temp.path()).expect("advice");
    let advisor_error = advice
        .advisor_error
        .as_deref()
        .expect("advisor should report credential compatibility error");

    assert!(advice.advisor_used);
    assert!(advisor_error.contains("JWT/OAuth-style coding-plan credential"));
    assert!(advisor_error.contains("api.openai.com"));
}

#[test]
fn advise_workspace_allows_jwt_coding_plan_token_for_configured_provider_endpoint() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    let jwt = "eyJhbGciOiJSUzI1NiJ9.eyJhdWQiOiJjb2RpbmctcGxhbiJ9.signature";
    std::fs::create_dir_all(store.root().join("credentials").join("coding-plan"))
        .expect("credential dir");
    std::fs::write(
        store
            .root()
            .join("credentials")
            .join("coding-plan")
            .join("auth.json"),
        json!({ "OPENAI_API_KEY": jwt }).to_string(),
    )
    .expect("credential file");
    store
        .append_event(&Event {
            event_id: Some("evt-jwt-provider-credential".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some(
                "This is a good point to stop while obvious implementation work remains.".into(),
            ),
            ..Event::default()
        })
        .expect("event");
    let decision = json!({
        "diagnosis_id": "diagnosis-jwt-provider-credential",
        "dominant_entropy": "plan",
        "entropy_scores": {
            "plan": { "score": 65, "confidence": 80 }
        },
        "top_evidence": [
            {
                "event_id": "evt-jwt-provider-credential",
                "why_it_matters": "The active agent stopped before the next implementation step."
            }
        ],
        "cited_evidence_ids": ["evt-jwt-provider-credential"],
        "missing_evidence": [],
        "proposed_action": { "type": "send_follow_up", "target_agent": null },
        "expected_entropy_delta": [
            { "kind": "plan", "delta": -25 }
        ],
        "packet_intent": "continue bounded implementation",
        "packet_draft": {
            "urgency": "follow_up",
            "summary": "Continue the current implementation.",
            "instructions": ["Take one concrete implementation step."],
            "evidence_refs": ["evt-jwt-provider-credential"]
        },
        "ask_user": null,
        "confidence": 0.75
    });
    let (endpoint, request_rx) = serve_advisor_once(decision);
    std::fs::write(
        store.root().join("config.json"),
        json!({
            "advisor": {
                "enabled": true,
                "provider": {
                    "endpoint": endpoint,
                    "model": "coding-plan-advisor",
                    "credential_source": "coding_plan",
                    "credential_file": "credentials/coding-plan/auth.json",
                    "timeout_secs": 5
                }
            }
        })
        .to_string(),
    )
    .expect("config");

    let advice = advise_workspace(temp.path()).expect("advice");
    let request = request_rx.recv().expect("advisor request");

    assert!(advice.advisor_used);
    assert_eq!(advice.advisor_error, None);
    assert!(request.contains(&format!("Authorization: Bearer {jwt}")));
}

#[test]
fn advise_workspace_rewrites_endpoint_advisor_ask_user_to_bounded_monitor_question() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            event_id: Some("evt-delete-prod".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some(
                "Should I delete the production database backup before continuing?".into(),
            ),
            ..Event::default()
        })
        .expect("event");
    let decision = json!({
        "diagnosis_id": "diagnosis-user-decision",
        "dominant_entropy": "user_decision",
        "entropy_scores": {
            "user_decision": { "score": 90, "confidence": 80 }
        },
        "top_evidence": [
            {
                "event_id": "evt-delete-prod",
                "why_it_matters": "Production deletion needs user authority."
            }
        ],
        "cited_evidence_ids": ["evt-delete-prod"],
        "missing_evidence": ["user authorization"],
        "proposed_action": {
            "type": "ask_user",
            "question": "Should I continue?"
        },
        "expected_entropy_delta": [
            { "kind": "user_decision", "delta": -80 }
        ],
        "packet_intent": "ask one bounded authorization question",
        "packet_draft": {
            "urgency": "urgent",
            "summary": "Ask user for authorization.",
            "instructions": ["Ask before deleting production data."],
            "evidence_refs": ["evt-delete-prod"]
        },
        "ask_user": {
            "question": "Should I continue?",
            "options": ["Yes", "No"]
        },
        "confidence": 0.72
    });
    let (endpoint, _request_rx) = serve_advisor_once(decision);
    let env_name = "CAM_TEST_ADVISOR_KEY_REWRITE_ASK_USER";
    set_test_env_var(env_name, "test-key");
    std::fs::write(
        store.root().join("config.json"),
        json!({
            "advisor": {
                "enabled": true,
                "provider": {
                    "endpoint": endpoint,
                    "model": "test-advisor",
                    "api_key_env": env_name,
                    "timeout_secs": 5
                }
            }
        })
        .to_string(),
    )
    .expect("config");

    let advice = advise_workspace(temp.path()).expect("advice");

    assert!(advice.advisor_used);
    assert!(advice.advisor_error.is_none());
    assert!(advice.advisor_decision.is_some());
    let ControlAction::AskUser { question } = &advice.final_action else {
        panic!("expected final ask_user, got {:?}", advice.final_action);
    };
    assert!(question.contains("User authorization is required"));
    assert!(question.contains("destructive or external side-effect consent is required"));
    assert!(!question.contains("Should I continue?"));
    assert!(matches!(
        advice.validation_outcome,
        ValidationOutcome::Modified {
            replacement: ControlAction::AskUser { .. },
            ..
        }
    ));
    assert!(
        advice
            .packet
            .summary
            .contains("destructive or external side-effect consent is required")
    );
}

#[test]
fn advise_workspace_falls_back_when_endpoint_advisor_ignores_verification_progress_pruning() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:01:00Z".into()),
            event_id: Some("evt-write".into()),
            agent: "codex".into(),
            kind: EventKind::FileChange,
            file: Some("src/lib.rs".into()),
            rationale: Some("Change production code.".into()),
            ..Event::default()
        })
        .expect("event");
    let decision = json!({
        "diagnosis_id": "diagnosis-follow-up",
        "dominant_entropy": "plan",
        "entropy_scores": {
            "plan": { "score": 65, "confidence": 80 }
        },
        "top_evidence": [
            {
                "event_id": "evt-write",
                "why_it_matters": "The agent needs a bounded next step."
            }
        ],
        "cited_evidence_ids": ["evt-write"],
        "missing_evidence": ["bounded next-step packet"],
        "proposed_action": { "type": "send_follow_up", "target_agent": null },
        "expected_entropy_delta": [
            { "kind": "plan", "delta": -25 }
        ],
        "packet_intent": "require a bounded next step",
        "packet_draft": {
            "urgency": "follow_up",
            "summary": "Continue with one bounded next step.",
            "instructions": ["Take one concrete implementation step."],
            "evidence_refs": ["evt-write"]
        },
        "ask_user": null,
        "confidence": 0.72
    });
    let (endpoint, _request_rx) = serve_advisor_once(decision);
    let env_name = "CAM_TEST_ADVISOR_KEY_OVERRIDE_FOLLOW_UP_VERIFICATION";
    set_test_env_var(env_name, "test-key");
    std::fs::write(
        store.root().join("config.json"),
        json!({
            "advisor": {
                "enabled": true,
                "provider": {
                    "endpoint": endpoint,
                    "model": "test-advisor",
                    "api_key_env": env_name,
                    "timeout_secs": 5
                }
            }
        })
        .to_string(),
    )
    .expect("config");

    let advice = advise_workspace(temp.path()).expect("advice");

    assert!(advice.advisor_used);
    assert!(
        advice
            .advisor_error
            .as_deref()
            .is_some_and(|error| error.contains("forbidden action"))
    );
    assert!(advice.advisor_decision.is_none());
    assert_eq!(
        advice.final_action,
        ControlAction::ForceVerification {
            suite: VerificationSuite::Full,
            blocking: true,
        }
    );
    assert!(matches!(
        advice.validation_outcome,
        ValidationOutcome::Approved(ControlAction::ForceVerification { .. })
    ));
    assert_eq!(advice.packet.urgency, PacketUrgency::Urgent);
    assert_eq!(advice.packet.title, "Verification required");
    let packet_instructions = advice
        .packet
        .instructions
        .iter()
        .map(|instruction| instruction.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        packet_instructions.contains("Record verifier command/result"),
        "{packet_instructions}"
    );
    assert!(
        packet_instructions.contains("classify the failure")
            && packet_instructions.contains("likely cause"),
        "{packet_instructions}"
    );
}

#[test]
fn advisor_request_forbids_progress_actions_when_verification_entropy_is_high() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:01:00Z".into()),
            event_id: Some("evt-write".into()),
            agent: "codex".into(),
            kind: EventKind::FileChange,
            file: Some("src/lib.rs".into()),
            rationale: Some("Change production code.".into()),
            ..Event::default()
        })
        .expect("event");
    let decision = json!({
        "diagnosis_id": "diagnosis-verification",
        "dominant_entropy": "verification",
        "entropy_scores": {
            "verification": { "score": 90, "confidence": 80 }
        },
        "top_evidence": [
            {
                "event_id": "evt-write",
                "why_it_matters": "Source changed after last verifier."
            }
        ],
        "cited_evidence_ids": ["evt-write"],
        "missing_evidence": ["passing verifier result"],
        "proposed_action": {
            "type": "force_verification",
            "suite": "full",
            "blocking": true
        },
        "expected_entropy_delta": [
            { "kind": "verification", "delta": -55 }
        ],
        "packet_intent": "require verification",
        "packet_draft": {
            "urgency": "urgent",
            "summary": "Verify before continuing.",
            "instructions": ["Run the full verifier."],
            "evidence_refs": ["evt-write"]
        },
        "ask_user": null,
        "confidence": 0.72
    });
    let (endpoint, request_rx) = serve_advisor_once(decision);
    let env_name = "CAM_TEST_ADVISOR_KEY_PRUNE_PROGRESS_VERIFICATION";
    set_test_env_var(env_name, "test-key");
    std::fs::write(
        store.root().join("config.json"),
        json!({
            "advisor": {
                "enabled": true,
                "provider": {
                    "endpoint": endpoint,
                    "model": "test-advisor",
                    "api_key_env": env_name,
                    "timeout_secs": 5
                }
            }
        })
        .to_string(),
    )
    .expect("config");

    let advice = advise_workspace(temp.path()).expect("advice");
    let request = request_rx.recv().expect("advisor request");
    let case_file = advisor_case_file_from_request(&request);
    let allowed = case_file_action_values(&case_file, "allowed_actions");

    assert!(advice.advisor_error.is_none());
    assert!(allowed.contains(&"force_verification"));
    assert!(!allowed.contains(&"continue_working"));
    assert!(!allowed.contains(&"send_follow_up"));
    let forbidden = case_file
        .get("forbidden_actions")
        .and_then(serde_json::Value::as_array)
        .expect("forbidden actions");
    assert!(forbidden.iter().any(|action| {
        action.get("action").and_then(serde_json::Value::as_str) == Some("continue_working")
            && action
                .get("reason")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|reason| reason.contains("verification entropy"))
    }));
    assert!(forbidden.iter().any(|action| {
        action.get("action").and_then(serde_json::Value::as_str) == Some("send_follow_up")
            && action
                .get("reason")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|reason| reason.contains("verification entropy"))
    }));
}

#[test]
fn advisor_request_honors_max_input_token_budget_by_bounding_case_file() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    for index in 0..80 {
        store
            .append_event(&Event {
                event_id: Some(format!("evt-budget-{index}")),
                agent: "codex".into(),
                kind: EventKind::ModelMessage,
                content: Some(format!(
                    "budget evidence {index} marker-BUDGET-TAIL-{index}"
                )),
                ..Event::default()
            })
            .expect("event");
    }
    let decision = json!({
        "diagnosis_id": "diagnosis-budgeted",
        "dominant_entropy": "plan",
        "entropy_scores": {
            "plan": { "score": 20, "confidence": 80 }
        },
        "top_evidence": [],
        "cited_evidence_ids": [],
        "missing_evidence": [],
        "proposed_action": {
            "type": "force_verification",
            "suite": "full",
            "blocking": true
        },
        "expected_entropy_delta": [
            { "kind": "verification", "delta": -40 }
        ],
        "packet_intent": "require verification",
        "packet_draft": {
            "urgency": "urgent",
            "summary": "Verify before continuing.",
            "instructions": ["Run the full verifier."],
            "evidence_refs": []
        },
        "ask_user": null,
        "confidence": 0.72
    });
    let (endpoint, request_rx) = serve_advisor_once(decision);
    let env_name = "CAM_TEST_ADVISOR_KEY_BUDGET";
    set_test_env_var(env_name, "test-key");
    std::fs::write(
        store.root().join("config.json"),
        json!({
            "advisor": {
                "enabled": true,
                "provider": {
                    "endpoint": endpoint,
                    "model": "test-advisor",
                    "api_key_env": env_name,
                    "timeout_secs": 5,
                    "max_input_tokens": 300
                }
            }
        })
        .to_string(),
    )
    .expect("config");

    let advice = advise_workspace(temp.path()).expect("advice");
    let request = request_rx.recv().expect("advisor request");

    assert!(advice.advisor_error.is_none());
    assert!(
        !request.contains("marker-BUDGET-TAIL-79"),
        "advisor request should not include low-priority evidence beyond max_input_tokens"
    );
    assert!(request.contains("case_file_id"));
    assert!(request.contains("allowed_actions"));
}

#[test]
fn advisor_request_preserves_salient_user_requirement_under_token_budget() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    for index in 0..80 {
        store
            .append_event(&Event {
                event_id: Some(format!("evt-low-salience-{index:02}")),
                agent: "codex".into(),
                kind: EventKind::ModelMessage,
                content: Some(format!(
                    "low salience background note marker-LOW-SALIENCE-{index:02} with repeated filler for budget pressure"
                )),
                ..Event::default()
            })
            .expect("noise event");
    }
    store
        .append_event(&Event {
            event_id: Some("evt-user-requirement".into()),
            agent: "user".into(),
            kind: EventKind::UserInstruction,
            content: Some("Acceptance criterion: parser must preserve comments roundtrip.".into()),
            ..Event::default()
        })
        .expect("user requirement");
    let decision = json!({
        "diagnosis_id": "diagnosis-salience-budget",
        "dominant_entropy": "plan",
        "entropy_scores": {
            "plan": { "score": 20, "confidence": 80 }
        },
        "top_evidence": [],
        "cited_evidence_ids": [],
        "missing_evidence": [],
        "proposed_action": {
            "type": "force_verification",
            "suite": "full",
            "blocking": true
        },
        "expected_entropy_delta": [
            { "kind": "verification", "delta": -40 }
        ],
        "packet_intent": "preserve the user requirement",
        "packet_draft": {
            "urgency": "follow_up",
            "summary": "Continue with the user requirement in mind.",
            "instructions": ["Preserve comments roundtrip."],
            "evidence_refs": []
        },
        "ask_user": null,
        "confidence": 0.72
    });
    let (endpoint, request_rx) = serve_advisor_once(decision);
    let env_name = "CAM_TEST_ADVISOR_KEY_SALIENCE_BUDGET";
    set_test_env_var(env_name, "test-key");
    std::fs::write(
        store.root().join("config.json"),
        json!({
            "advisor": {
                "enabled": true,
                "provider": {
                    "endpoint": endpoint,
                    "model": "test-advisor",
                    "api_key_env": env_name,
                    "timeout_secs": 5,
                    "max_input_tokens": 2200
                }
            }
        })
        .to_string(),
    )
    .expect("config");

    let advice = advise_workspace(temp.path()).expect("advice");
    let request = request_rx.recv().expect("advisor request");

    assert!(advice.advisor_error.is_none());
    assert!(
        request.contains("parser must preserve comments roundtrip"),
        "advisor-visible case file should retain salient user requirements under budget: {request}"
    );
}

#[test]
fn advisor_request_redacts_memory_candidates_and_removes_raw_repo_trace_details() {
    let temp = tempfile::tempdir().expect("temp dir");
    init_git_repo(temp.path());
    std::fs::create_dir_all(temp.path().join("src")).expect("src dir");
    std::fs::write(temp.path().join("src/lib.rs"), "fn one() {}\n").expect("source");
    run_git(temp.path(), ["add", "src/lib.rs"]);
    run_git(temp.path(), ["commit", "-m", "add source"]);
    std::fs::write(temp.path().join("src/lib.rs"), "fn one_changed() {}\n")
        .expect("changed source");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            event_id: Some("evt-design-secret".into()),
            agent: "codex".into(),
            kind: EventKind::DesignThought,
            content: Some("Durable memory mentions api_key=memory-secret-value".into()),
            ..Event::default()
        })
        .expect("event");
    store
        .append_trace(&TraceEntry {
            time: Some("9999-01-01T00:00:00Z".into()),
            event_id: Some("evt-trace-secret".into()),
            agent: "codex".into(),
            file: "src/lib.rs".into(),
            line: Some(1),
            rationale: Some("Trace rationale mentions api_key=trace-secret-value".into()),
            related_event_ids: vec!["evt-design-secret".into()],
            ..TraceEntry::default()
        })
        .expect("trace");
    let decision = json!({
        "diagnosis_id": "diagnosis-redaction",
        "dominant_entropy": "repo_blame",
        "entropy_scores": {
            "repo_blame": { "score": 20, "confidence": 80 }
        },
        "top_evidence": [],
        "cited_evidence_ids": [],
        "missing_evidence": [],
        "proposed_action": {
            "type": "force_verification",
            "suite": "full",
            "blocking": true
        },
        "expected_entropy_delta": [
            { "kind": "verification", "delta": -40 }
        ],
        "packet_intent": "require verification",
        "packet_draft": {
            "urgency": "urgent",
            "summary": "Verify before continuing.",
            "instructions": ["Run the full verifier."],
            "evidence_refs": []
        },
        "ask_user": null,
        "confidence": 0.72
    });
    let (endpoint, request_rx) = serve_advisor_once(decision);
    let env_name = "CAM_TEST_ADVISOR_KEY_REDACT_CASE";
    set_test_env_var(env_name, "test-key");
    std::fs::write(
        store.root().join("config.json"),
        json!({
            "advisor": {
                "enabled": true,
                "provider": {
                    "endpoint": endpoint,
                    "model": "test-advisor",
                    "api_key_env": env_name,
                    "timeout_secs": 5
                }
            }
        })
        .to_string(),
    )
    .expect("config");

    let advice = advise_workspace(temp.path()).expect("advice");
    let request = request_rx.recv().expect("advisor request");

    assert!(advice.advisor_error.is_none());
    assert!(!request.contains("memory-secret-value"));
    assert!(!request.contains("trace-secret-value"));
    assert!(request.contains("[REDACTED]"));
    let case_file = advisor_case_file_from_request(&request);
    let matching_traces = case_file
        .pointer("/repo_audit/changes/0/matching_traces")
        .and_then(|value| value.as_array())
        .expect("matching traces");
    assert!(
        matching_traces.is_empty(),
        "advisor request should not include raw trace records"
    );
}

#[test]
fn advisor_visible_case_file_does_not_leave_dangling_entropy_evidence_refs_after_budgeting() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:01:00Z".into()),
            event_id: Some("evt-write".into()),
            agent: "codex".into(),
            kind: EventKind::FileChange,
            file: Some("src/lib.rs".into()),
            rationale: Some("Change production code.".into()),
            ..Event::default()
        })
        .expect("event");
    for index in 0..60 {
        store
            .append_event(&Event {
                event_id: Some(format!("evt-noise-{index}")),
                agent: "codex".into(),
                kind: EventKind::ModelMessage,
                content: Some(format!("noise evidence {index}")),
                ..Event::default()
            })
            .expect("event");
    }
    let decision = json!({
        "diagnosis_id": "diagnosis-dangling-refs",
        "dominant_entropy": "plan",
        "entropy_scores": {
            "plan": { "score": 20, "confidence": 80 }
        },
        "top_evidence": [],
        "cited_evidence_ids": [],
        "missing_evidence": [],
        "proposed_action": {
            "type": "ask_user",
            "question": "Should I continue?"
        },
        "expected_entropy_delta": [],
        "packet_intent": "no blocking uncertainty",
        "packet_draft": {
            "urgency": "follow_up",
            "summary": "Continue working.",
            "instructions": ["Keep verification current."],
            "evidence_refs": []
        },
        "ask_user": null,
        "confidence": 0.72
    });
    let (endpoint, request_rx) = serve_advisor_once(decision);
    let env_name = "CAM_TEST_ADVISOR_KEY_DANGLING_REFS";
    set_test_env_var(env_name, "test-key");
    std::fs::write(
        store.root().join("config.json"),
        json!({
            "advisor": {
                "enabled": true,
                "provider": {
                    "endpoint": endpoint,
                    "model": "test-advisor",
                    "api_key_env": env_name,
                    "timeout_secs": 5,
                    "max_input_tokens": 1
                }
            }
        })
        .to_string(),
    )
    .expect("config");

    advise_workspace(temp.path()).expect("advice");
    let request = request_rx.recv().expect("advisor request");
    let case_file = advisor_case_file_from_request(&request);
    let visible_ids = advisor_visible_evidence_ids(&case_file);

    for evidence_id in case_file
        .get("entropy")
        .and_then(|entropy| entropy.get("scores"))
        .and_then(|scores| scores.as_array())
        .into_iter()
        .flatten()
        .flat_map(|score| {
            score
                .get("evidence_ids")
                .and_then(|ids| ids.as_array())
                .into_iter()
                .flatten()
                .filter_map(|id| id.as_str())
        })
    {
        assert!(
            visible_ids
                .iter()
                .any(|visible_id| visible_id == evidence_id),
            "dangling entropy evidence ref {evidence_id} in advisor-visible case file"
        );
    }
}

#[test]
fn advise_workspace_falls_back_when_endpoint_advisor_cites_unknown_packet_evidence() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            event_id: Some("evt-known".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some(
                "This is a good point to stop while obvious implementation work remains.".into(),
            ),
            ..Event::default()
        })
        .expect("event");
    let decision = json!({
        "diagnosis_id": "diagnosis-bad-ref",
        "dominant_entropy": "plan",
        "entropy_scores": {
            "plan": { "score": 65, "confidence": 80 }
        },
        "top_evidence": [
            {
                "event_id": "evt-known",
                "why_it_matters": "Known event."
            }
        ],
        "cited_evidence_ids": ["evt-known"],
        "missing_evidence": ["bounded next-step packet"],
        "proposed_action": { "type": "send_follow_up", "target_agent": null },
        "expected_entropy_delta": [
            { "kind": "plan", "delta": -25 }
        ],
        "packet_intent": "require a bounded next step",
        "packet_draft": {
            "urgency": "follow_up",
            "summary": "Continue with one bounded next step.",
            "instructions": ["Take one concrete implementation step."],
            "evidence_refs": ["evt-missing"]
        },
        "ask_user": null,
        "confidence": 0.72
    });
    let (endpoint, _request_rx) = serve_advisor_once(decision);
    let env_name = "CAM_TEST_ADVISOR_KEY_FALLBACK_UNKNOWN_REF";
    set_test_env_var(env_name, "test-key");
    std::fs::write(
        store.root().join("config.json"),
        json!({
            "advisor": {
                "enabled": true,
                "provider": {
                    "endpoint": endpoint,
                    "model": "test-advisor",
                    "api_key_env": env_name,
                    "timeout_secs": 5
                }
            }
        })
        .to_string(),
    )
    .expect("config");

    let advice = advise_workspace(temp.path()).expect("advice");

    assert!(advice.advisor_used);
    assert!(advice.advisor_decision.is_none());
    assert!(
        advice
            .advisor_error
            .as_deref()
            .is_some_and(|error| error.contains("evt-missing"))
    );
    assert_eq!(
        advice.final_action,
        ControlAction::SendFollowUp { target_agent: None }
    );
}

#[test]
fn advise_workspace_falls_back_when_endpoint_advisor_returns_out_of_range_entropy_delta() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            event_id: Some("evt-known".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some("working".into()),
            ..Event::default()
        })
        .expect("event");
    let decision = json!({
        "diagnosis_id": "diagnosis-bad-delta",
        "dominant_entropy": "plan",
        "entropy_scores": {
            "plan": { "score": 65, "confidence": 80 }
        },
        "top_evidence": [
            {
                "event_id": "evt-known",
                "why_it_matters": "Known event."
            }
        ],
        "cited_evidence_ids": ["evt-known"],
        "missing_evidence": ["bounded next-step packet"],
        "proposed_action": { "type": "send_follow_up", "target_agent": null },
        "expected_entropy_delta": [
            { "kind": "plan", "delta": -150 }
        ],
        "packet_intent": "require a bounded next step",
        "packet_draft": {
            "urgency": "follow_up",
            "summary": "Continue with one bounded next step.",
            "instructions": ["Take one concrete implementation step."],
            "evidence_refs": ["evt-known"]
        },
        "ask_user": null,
        "confidence": 0.72
    });
    let (endpoint, _request_rx) = serve_advisor_once(decision);
    let env_name = "CAM_TEST_ADVISOR_KEY_FALLBACK_BAD_DELTA";
    set_test_env_var(env_name, "test-key");
    std::fs::write(
        store.root().join("config.json"),
        json!({
            "advisor": {
                "enabled": true,
                "provider": {
                    "endpoint": endpoint,
                    "model": "test-advisor",
                    "api_key_env": env_name,
                    "timeout_secs": 5
                }
            }
        })
        .to_string(),
    )
    .expect("config");

    let advice = advise_workspace(temp.path()).expect("advice");

    assert!(advice.advisor_used);
    assert!(advice.advisor_decision.is_none());
    assert!(
        advice
            .advisor_error
            .as_deref()
            .is_some_and(|error| error.contains("entropy delta"))
    );
    assert_eq!(advice.final_action, ControlAction::ContinueWorking);
}

#[test]
fn advise_workspace_falls_back_when_endpoint_advisor_omits_dominant_entropy_score() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            event_id: Some("evt-known".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some("working".into()),
            ..Event::default()
        })
        .expect("event");
    let decision = json!({
        "diagnosis_id": "diagnosis-missing-dominant-score",
        "dominant_entropy": "verification",
        "entropy_scores": {
            "plan": { "score": 65, "confidence": 80 }
        },
        "top_evidence": [
            {
                "event_id": "evt-known",
                "why_it_matters": "Known event."
            }
        ],
        "cited_evidence_ids": ["evt-known"],
        "missing_evidence": ["passing verifier result"],
        "proposed_action": {
            "type": "force_verification",
            "suite": "full",
            "blocking": true
        },
        "expected_entropy_delta": [
            { "kind": "verification", "delta": -55 }
        ],
        "packet_intent": "require verification",
        "packet_draft": {
            "urgency": "urgent",
            "summary": "Verify before continuing.",
            "instructions": ["Run the full verifier."],
            "evidence_refs": ["evt-known"]
        },
        "ask_user": null,
        "confidence": 0.72
    });
    let (endpoint, _request_rx) = serve_advisor_once(decision);
    let env_name = "CAM_TEST_ADVISOR_KEY_FALLBACK_MISSING_DOMINANT_SCORE";
    set_test_env_var(env_name, "test-key");
    std::fs::write(
        store.root().join("config.json"),
        json!({
            "advisor": {
                "enabled": true,
                "provider": {
                    "endpoint": endpoint,
                    "model": "test-advisor",
                    "api_key_env": env_name,
                    "timeout_secs": 5
                }
            }
        })
        .to_string(),
    )
    .expect("config");

    let advice = advise_workspace(temp.path()).expect("advice");

    assert!(advice.advisor_used);
    assert!(advice.advisor_decision.is_none());
    assert!(
        advice
            .advisor_error
            .as_deref()
            .is_some_and(|error| error.contains("dominant entropy"))
    );
    assert_eq!(advice.final_action, ControlAction::ContinueWorking);
}

#[test]
fn advise_workspace_falls_back_when_endpoint_advisor_targets_unknown_agent() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    for index in 1..=3 {
        store
            .append_event(&Event {
                time: Some(format!("2026-06-22T12:2{index}:00Z")),
                event_id: Some(format!("evt-unknown-target-loop-{index}")),
                agent: "codex".into(),
                kind: EventKind::CommandResult,
                command: Some("python scripts/probe.py".into()),
                exit_code: Some(1),
                ..Event::default()
            })
            .expect("event");
    }
    let decision = json!({
        "diagnosis_id": "diagnosis-unknown-target",
        "dominant_entropy": "agent_health",
        "entropy_scores": {
            "agent_health": { "score": 82, "confidence": 88 }
        },
        "top_evidence": [
            {
                "event_id": "evt-unknown-target-loop-3",
                "why_it_matters": "The same command failed repeatedly."
            }
        ],
        "cited_evidence_ids": ["evt-unknown-target-loop-3"],
        "missing_evidence": ["loop-breaking retry result"],
        "proposed_action": {
            "type": "retry_agent",
            "target_agent": "invented-agent",
            "max_attempts": 1
        },
        "expected_entropy_delta": [
            { "kind": "agent_health", "delta": -25 }
        ],
        "packet_intent": "break the repeated command loop",
        "packet_draft": {
            "urgency": "urgent",
            "summary": "Break the repeated command loop.",
            "instructions": ["Do not repeat the same failing command without changing approach."],
            "evidence_refs": ["evt-unknown-target-loop-3"]
        },
        "ask_user": null,
        "confidence": 0.72
    });
    let (endpoint, _request_rx) = serve_advisor_once(decision);
    let env_name = "CAM_TEST_ADVISOR_KEY_FALLBACK_UNKNOWN_TARGET";
    set_test_env_var(env_name, "test-key");
    std::fs::write(
        store.root().join("config.json"),
        json!({
            "advisor": {
                "enabled": true,
                "provider": {
                    "endpoint": endpoint,
                    "model": "test-advisor",
                    "api_key_env": env_name,
                    "timeout_secs": 5
                }
            }
        })
        .to_string(),
    )
    .expect("config");

    let advice = advise_workspace(temp.path()).expect("advice");

    assert!(advice.advisor_used);
    assert!(advice.advisor_decision.is_none());
    assert!(
        advice
            .advisor_error
            .as_deref()
            .is_some_and(|error| error.contains("invented-agent"))
    );
    assert_eq!(
        advice.final_action,
        ControlAction::RetryAgent {
            target_agent: Some("codex".into()),
            max_attempts: 1,
        }
    );
    assert!(!store.root().join("outbox").join("invented-agent").exists());
}

#[test]
fn advise_workspace_falls_back_without_persisting_tainted_advisor_diagnostics() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            event_id: Some("evt-known".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some("working".into()),
            ..Event::default()
        })
        .expect("event");
    let decision = json!({
        "diagnosis_id": "diagnosis-tainted-diagnostic",
        "dominant_entropy": "plan",
        "entropy_scores": {
            "plan": { "score": 65, "confidence": 80 }
        },
        "top_evidence": [
            {
                "event_id": "evt-known",
                "why_it_matters": "authorization: bearer leaked-token"
            }
        ],
        "cited_evidence_ids": ["evt-known"],
        "missing_evidence": ["api_key=diagnostic-secret"],
        "proposed_action": { "type": "send_follow_up", "target_agent": null },
        "expected_entropy_delta": [
            { "kind": "plan", "delta": -25 }
        ],
        "packet_intent": "require a bounded next step",
        "packet_draft": {
            "urgency": "follow_up",
            "summary": "Continue with one bounded next step.",
            "instructions": ["Take one concrete implementation step."],
            "evidence_refs": ["evt-known"]
        },
        "ask_user": {
            "question": "Which credential uses password=secret?",
            "options": ["Use default"]
        },
        "confidence": 0.72
    });
    let (endpoint, _request_rx) = serve_advisor_once(decision);
    let env_name = "CAM_TEST_ADVISOR_KEY_FALLBACK_TAINTED_DIAGNOSTIC";
    set_test_env_var(env_name, "test-key");
    std::fs::write(
        store.root().join("config.json"),
        json!({
            "advisor": {
                "enabled": true,
                "provider": {
                    "endpoint": endpoint,
                    "model": "test-advisor",
                    "api_key_env": env_name,
                    "timeout_secs": 5
                }
            }
        })
        .to_string(),
    )
    .expect("config");

    let advice = advise_workspace(temp.path()).expect("advice");

    assert!(advice.advisor_used);
    assert!(advice.advisor_decision.is_none());
    assert!(
        advice
            .advisor_error
            .as_deref()
            .is_some_and(|error| error.contains("tainted"))
    );
    let advice_log =
        std::fs::read_to_string(store.root().join("advice.jsonl")).expect("advice log");
    assert!(!advice_log.contains("leaked-token"));
    assert!(!advice_log.contains("diagnostic-secret"));
    assert!(!advice_log.contains("password=secret"));
}

#[test]
fn advise_workspace_does_not_persist_tainted_unknown_advisor_evidence_id() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            event_id: Some("evt-known".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some("working".into()),
            ..Event::default()
        })
        .expect("event");
    let decision = json!({
        "diagnosis_id": "diagnosis-tainted-id",
        "dominant_entropy": "plan",
        "entropy_scores": {
            "plan": { "score": 65, "confidence": 80 }
        },
        "top_evidence": [],
        "cited_evidence_ids": ["api_key=identifier-secret"],
        "missing_evidence": ["bounded next-step packet"],
        "proposed_action": { "type": "send_follow_up", "target_agent": null },
        "expected_entropy_delta": [
            { "kind": "plan", "delta": -25 }
        ],
        "packet_intent": "require a bounded next step",
        "packet_draft": {
            "urgency": "follow_up",
            "summary": "Continue with one bounded next step.",
            "instructions": ["Take one concrete implementation step."],
            "evidence_refs": []
        },
        "ask_user": null,
        "confidence": 0.72
    });
    let (endpoint, _request_rx) = serve_advisor_once(decision);
    let env_name = "CAM_TEST_ADVISOR_KEY_FALLBACK_TAINTED_ID";
    set_test_env_var(env_name, "test-key");
    std::fs::write(
        store.root().join("config.json"),
        json!({
            "advisor": {
                "enabled": true,
                "provider": {
                    "endpoint": endpoint,
                    "model": "test-advisor",
                    "api_key_env": env_name,
                    "timeout_secs": 5
                }
            }
        })
        .to_string(),
    )
    .expect("config");

    let advice = advise_workspace(temp.path()).expect("advice");

    assert!(advice.advisor_used);
    assert!(advice.advisor_decision.is_none());
    assert!(
        advice
            .advisor_error
            .as_deref()
            .is_some_and(|error| error.contains("tainted"))
    );
    let advice_log =
        std::fs::read_to_string(store.root().join("advice.jsonl")).expect("advice log");
    assert!(!advice_log.contains("identifier-secret"));
    assert!(!advice_log.contains("api_key="));
}

#[test]
fn advise_workspace_does_not_persist_tainted_top_evidence_id() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            event_id: Some("evt-known".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some("working".into()),
            ..Event::default()
        })
        .expect("event");
    let decision = json!({
        "diagnosis_id": "diagnosis-tainted-top-id",
        "dominant_entropy": "plan",
        "entropy_scores": {
            "plan": { "score": 65, "confidence": 80 }
        },
        "top_evidence": [
            {
                "event_id": "api_key=top-evidence-secret",
                "why_it_matters": "Known event."
            }
        ],
        "cited_evidence_ids": [],
        "missing_evidence": ["bounded next-step packet"],
        "proposed_action": { "type": "send_follow_up", "target_agent": null },
        "expected_entropy_delta": [
            { "kind": "plan", "delta": -25 }
        ],
        "packet_intent": "require a bounded next step",
        "packet_draft": {
            "urgency": "follow_up",
            "summary": "Continue with one bounded next step.",
            "instructions": ["Take one concrete implementation step."],
            "evidence_refs": []
        },
        "ask_user": null,
        "confidence": 0.72
    });
    let (endpoint, _request_rx) = serve_advisor_once(decision);
    let env_name = "CAM_TEST_ADVISOR_KEY_FALLBACK_TAINTED_TOP_ID";
    set_test_env_var(env_name, "test-key");
    std::fs::write(
        store.root().join("config.json"),
        json!({
            "advisor": {
                "enabled": true,
                "provider": {
                    "endpoint": endpoint,
                    "model": "test-advisor",
                    "api_key_env": env_name,
                    "timeout_secs": 5
                }
            }
        })
        .to_string(),
    )
    .expect("config");

    let advice = advise_workspace(temp.path()).expect("advice");

    assert!(advice.advisor_used);
    assert!(advice.advisor_decision.is_none());
    assert!(
        advice
            .advisor_error
            .as_deref()
            .is_some_and(|error| error.contains("tainted"))
    );
    let advice_log =
        std::fs::read_to_string(store.root().join("advice.jsonl")).expect("advice log");
    assert!(!advice_log.contains("top-evidence-secret"));
    assert!(!advice_log.contains("api_key="));
}

#[test]
fn advise_workspace_does_not_persist_tainted_unknown_advisor_target_agent() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            event_id: Some("evt-known".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some("working".into()),
            ..Event::default()
        })
        .expect("event");
    let decision = json!({
        "diagnosis_id": "diagnosis-tainted-target",
        "dominant_entropy": "plan",
        "entropy_scores": {
            "plan": { "score": 65, "confidence": 80 }
        },
        "top_evidence": [
            {
                "event_id": "evt-known",
                "why_it_matters": "Known event."
            }
        ],
        "cited_evidence_ids": ["evt-known"],
        "missing_evidence": ["bounded next-step packet"],
        "proposed_action": {
            "type": "send_follow_up",
            "target_agent": "api_key=target-secret"
        },
        "expected_entropy_delta": [
            { "kind": "plan", "delta": -25 }
        ],
        "packet_intent": "require a bounded next step",
        "packet_draft": {
            "urgency": "follow_up",
            "summary": "Continue with one bounded next step.",
            "instructions": ["Take one concrete implementation step."],
            "evidence_refs": ["evt-known"]
        },
        "ask_user": null,
        "confidence": 0.72
    });
    let (endpoint, _request_rx) = serve_advisor_once(decision);
    let env_name = "CAM_TEST_ADVISOR_KEY_FALLBACK_TAINTED_TARGET";
    set_test_env_var(env_name, "test-key");
    std::fs::write(
        store.root().join("config.json"),
        json!({
            "advisor": {
                "enabled": true,
                "provider": {
                    "endpoint": endpoint,
                    "model": "test-advisor",
                    "api_key_env": env_name,
                    "timeout_secs": 5
                }
            }
        })
        .to_string(),
    )
    .expect("config");

    let advice = advise_workspace(temp.path()).expect("advice");

    assert!(advice.advisor_used);
    assert!(advice.advisor_decision.is_none());
    assert!(
        advice
            .advisor_error
            .as_deref()
            .is_some_and(|error| error.contains("tainted"))
    );
    let advice_log =
        std::fs::read_to_string(store.root().join("advice.jsonl")).expect("advice log");
    assert!(!advice_log.contains("target-secret"));
    assert!(!advice_log.contains("api_key="));
    assert!(
        !store
            .root()
            .join("outbox")
            .join("api_key-target-secret")
            .exists()
    );
}

#[test]
fn advise_workspace_does_not_persist_case_file_when_packet_is_rejected() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:00:00Z".into()),
            event_id: Some("evt-tainted-agent".into()),
            agent: "sk-secret-agent".into(),
            kind: EventKind::ModelMessage,
            content: Some("Continuing normally.".into()),
            ..Event::default()
        })
        .expect("event");

    let error = advise_workspace(temp.path()).expect_err("tainted packet should be rejected");

    assert!(error.to_string().contains("secret-like packet content"));
    assert!(!store.root().join("case-files.jsonl").exists());
    assert!(!store.root().join("advice.jsonl").exists());
    assert!(!store.root().join("packets.jsonl").exists());
    assert!(!store.root().join("dispatch.jsonl").exists());
}

#[test]
fn case_file_raises_agent_health_entropy_for_repeated_failing_command_loop() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    for index in 1..=3 {
        store
            .append_event(&Event {
                time: Some(format!("2026-06-22T12:0{index}:00Z")),
                event_id: Some(format!("evt-loop-{index}")),
                agent: "codex".into(),
                kind: EventKind::CommandResult,
                command: Some("python scripts/probe.py".into()),
                exit_code: Some(1),
                ..Event::default()
            })
            .expect("event");
    }
    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);
    let agent_health = case_file
        .entropy
        .score(EntropyKind::AgentHealth)
        .expect("agent health score");

    assert!(agent_health.score >= 80, "{agent_health:?}");
    assert!(
        agent_health
            .top_causes
            .iter()
            .any(|cause| cause.contains("repeated failing command"))
    );
    assert!(
        agent_health
            .missing_evidence
            .iter()
            .any(|missing| missing.contains("loop-breaking retry"))
    );
    assert!(agent_health.evidence_ids.contains(&"evt-loop-3".into()));
}

#[test]
fn case_file_classifies_repeated_failures_by_failure_layer() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    for index in 1..=3 {
        store
            .append_event(&Event {
                time: Some(format!("2026-06-22T12:0{index}:00Z")),
                event_id: Some(format!("evt-rate-limit-{index}")),
                agent: "codex".into(),
                kind: EventKind::AgentHealth,
                content: Some("429 rate limit exceeded by provider".into()),
                ..Event::default()
            })
            .expect("rate-limit event");
        store
            .append_event(&Event {
                time: Some(format!("2026-06-22T12:1{index}:00Z")),
                event_id: Some(format!("evt-shell-runtime-{index}")),
                agent: "claude-code".into(),
                kind: EventKind::CommandResult,
                command: Some("node scripts/missing.js".into()),
                exit_code: Some(1),
                content: Some("No such file or directory".into()),
                ..Event::default()
            })
            .expect("shell-runtime event");
    }

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);
    let agent_health = case_file
        .entropy
        .score(EntropyKind::AgentHealth)
        .expect("agent health score");

    assert!(
        agent_health
            .top_causes
            .iter()
            .any(|cause| cause.contains("rate_limit layer")),
        "rate-limit failures should not collapse into generic service failure: {agent_health:?}"
    );
    assert!(
        agent_health
            .top_causes
            .iter()
            .any(|cause| cause.contains("shell_runtime layer")),
        "shell-runtime failures should be separated from task difficulty: {agent_health:?}"
    );
    assert!(
        agent_health
            .missing_evidence
            .iter()
            .any(|missing| missing.contains("rate_limit-layer recovery evidence")),
        "layer-specific recovery evidence should be requested: {agent_health:?}"
    );
    assert!(
        agent_health
            .missing_evidence
            .iter()
            .any(|missing| missing.contains("shell_runtime-layer diagnosis")),
        "layer-specific command diagnosis should be requested: {agent_health:?}"
    );
}

#[test]
fn case_file_flags_repeated_unchanged_verifier_failure_after_edits_without_hypothesis() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    for index in 1..=3 {
        store
            .append_event(&Event {
                time: Some(format!("2026-06-22T12:0{index}:00Z")),
                event_id: Some(format!("evt-repeat-verifier-{index}")),
                agent: "codex".into(),
                kind: EventKind::CommandResult,
                command: Some("cargo test parser::tests::nested".into()),
                exit_code: Some(1),
                content: Some(
                    "thread 'parser::tests::nested' panicked: assertion failed expected nested parse"
                        .into(),
                ),
                ..Event::default()
            })
            .expect("verifier failure");
        if index < 3 {
            store
                .append_event(&Event {
                    time: Some(format!("2026-06-22T12:1{index}:00Z")),
                    event_id: Some(format!("evt-repeat-edit-{index}")),
                    agent: "codex".into(),
                    kind: EventKind::FileChange,
                    file: Some("src/parser.rs".into()),
                    rationale: Some("Try another parser tweak.".into()),
                    ..Event::default()
                })
                .expect("edit");
        }
    }

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);
    let verification = case_file
        .entropy
        .score(EntropyKind::Verification)
        .expect("verification score");
    let plan = case_file
        .entropy
        .score(EntropyKind::Plan)
        .expect("plan score");

    assert!(
        verification
            .top_causes
            .iter()
            .any(|cause| cause.contains("same verifier failure signature")
                && cause.contains("without a new hypothesis")),
        "verification entropy should name repeated unchanged verifier failure: {verification:?}"
    );
    assert!(
        verification
            .evidence_ids
            .contains(&"evt-repeat-verifier-3".into())
    );
    assert!(
        plan.score >= 70,
        "process conformance issue should raise plan entropy too: {plan:?}"
    );
    assert!(
        plan.missing_evidence
            .iter()
            .any(|missing| missing.contains("failure hypothesis")),
        "plan entropy should ask for diagnosis before more edits: {plan:?}"
    );
}

#[test]
fn case_file_does_not_flag_repeated_verifier_failure_when_hypothesis_changes() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:01:00Z".into()),
            event_id: Some("evt-hypothesis-fail-1".into()),
            agent: "codex".into(),
            kind: EventKind::CommandResult,
            command: Some("cargo test parser::tests::nested".into()),
            exit_code: Some(1),
            content: Some(
                "thread 'parser::tests::nested' panicked: assertion failed expected nested parse"
                    .into(),
            ),
            ..Event::default()
        })
        .expect("failure");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:02:00Z".into()),
            event_id: Some("evt-hypothesis-note".into()),
            agent: "codex".into(),
            kind: EventKind::DesignThought,
            content: Some(
                "Hypothesis: nested parser failure comes from tokenizer state reset.".into(),
            ),
            ..Event::default()
        })
        .expect("hypothesis");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:03:00Z".into()),
            event_id: Some("evt-hypothesis-edit".into()),
            agent: "codex".into(),
            kind: EventKind::FileChange,
            file: Some("src/parser.rs".into()),
            rationale: Some("Apply tokenizer-state hypothesis.".into()),
            ..Event::default()
        })
        .expect("edit");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:04:00Z".into()),
            event_id: Some("evt-hypothesis-fail-2".into()),
            agent: "codex".into(),
            kind: EventKind::CommandResult,
            command: Some("cargo test parser::tests::nested".into()),
            exit_code: Some(1),
            content: Some(
                "thread 'parser::tests::nested' panicked: assertion failed expected nested parse"
                    .into(),
            ),
            ..Event::default()
        })
        .expect("failure");

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);
    let verification = case_file
        .entropy
        .score(EntropyKind::Verification)
        .expect("verification score");

    assert!(
        !verification
            .top_causes
            .iter()
            .any(|cause| cause.contains("same verifier failure signature")),
        "new hypothesis should prevent the unchanged-failure thrash signal: {verification:?}"
    );
}

#[test]
fn case_file_flags_bug_fix_edit_before_reproduction_or_localization() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:00:00Z".into()),
            event_id: Some("evt-bugfix-goal".into()),
            agent: "user".into(),
            kind: EventKind::UserInstruction,
            content: Some("Fix the failing dashboard flow.".into()),
            ..Event::default()
        })
        .expect("goal");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:01:00Z".into()),
            event_id: Some("evt-bugfix-early-edit".into()),
            agent: "codex".into(),
            kind: EventKind::FileChange,
            file: Some("src/dashboard.rs".into()),
            rationale: Some("Patch likely dashboard failure.".into()),
            ..Event::default()
        })
        .expect("edit");

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);
    let plan = case_file
        .entropy
        .score(EntropyKind::Plan)
        .expect("plan score");

    assert!(
        plan.score >= 70,
        "bug-fix edit before probe should raise plan entropy: {plan:?}"
    );
    assert!(
        plan.top_causes.iter().any(|cause| {
            cause.contains("bug-fix edit")
                && cause.contains("reproduction or localization evidence")
        }),
        "{plan:?}"
    );
    assert!(plan.evidence_ids.contains(&"evt-bugfix-early-edit".into()));
    assert!(
        plan.missing_evidence
            .iter()
            .any(|missing| missing.contains("reproduction")),
        "{plan:?}"
    );
}

#[test]
fn force_verification_packet_names_bug_fix_reproduction_gap() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:00:00Z".into()),
            event_id: Some("evt-bugfix-packet-goal".into()),
            agent: "user".into(),
            kind: EventKind::UserInstruction,
            content: Some("Fix the failing dashboard flow.".into()),
            ..Event::default()
        })
        .expect("goal");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:01:00Z".into()),
            event_id: Some("evt-bugfix-packet-edit".into()),
            agent: "codex".into(),
            kind: EventKind::FileChange,
            file: Some("src/dashboard.rs".into()),
            rationale: Some("Patch likely dashboard failure.".into()),
            ..Event::default()
        })
        .expect("edit");

    let advice = advise_workspace(temp.path()).expect("advice");

    assert!(
        matches!(advice.final_action, ControlAction::ForceVerification { .. }),
        "stale source edit should still force verification: {:?}",
        advice.final_action
    );
    assert!(
        advice.packet.instructions.iter().any(|instruction| {
            instruction.text.contains("reproduce or localize")
                && instruction.text.contains("bug-fix")
        }),
        "verification packet should name the process gap: {:?}",
        advice.packet.instructions
    );
}

#[test]
fn case_file_allows_bug_fix_edit_after_local_probe() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:00:00Z".into()),
            event_id: Some("evt-bugfix-probed-goal".into()),
            agent: "user".into(),
            kind: EventKind::UserInstruction,
            content: Some("Fix the failing dashboard flow.".into()),
            ..Event::default()
        })
        .expect("goal");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:01:00Z".into()),
            event_id: Some("evt-bugfix-probe".into()),
            agent: "codex".into(),
            kind: EventKind::CommandResult,
            command: Some("python scripts/probe.py dashboard".into()),
            exit_code: Some(1),
            content: Some("Reproduced failure: dashboard request returns 500.".into()),
            ..Event::default()
        })
        .expect("probe");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:02:00Z".into()),
            event_id: Some("evt-bugfix-probed-edit".into()),
            agent: "codex".into(),
            kind: EventKind::FileChange,
            file: Some("src/dashboard.rs".into()),
            rationale: Some("Patch reproduced dashboard 500.".into()),
            ..Event::default()
        })
        .expect("edit");

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);

    assert!(
        case_file
            .entropy
            .score(EntropyKind::Plan)
            .is_none_or(|plan| !plan.top_causes.iter().any(|cause| {
                cause.contains("bug-fix edit")
                    && cause.contains("reproduction or localization evidence")
            })),
        "local probe should satisfy the pre-edit process obligation: {:?}",
        case_file.entropy.score(EntropyKind::Plan)
    );
}

#[test]
fn force_verification_packet_names_repeated_unchanged_failure_signature() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    for index in 1..=2 {
        store
            .append_event(&Event {
                time: Some(format!("2026-06-22T13:0{index}:00Z")),
                event_id: Some(format!("evt-packet-repeat-fail-{index}")),
                agent: "codex".into(),
                kind: EventKind::CommandResult,
                command: Some("cargo test parser::tests::nested".into()),
                exit_code: Some(1),
                content: Some(
                    "thread 'parser::tests::nested' panicked: assertion failed expected nested parse"
                        .into(),
                ),
                ..Event::default()
            })
            .expect("failure");
        if index == 1 {
            store
                .append_event(&Event {
                    time: Some("2026-06-22T13:02:30Z".into()),
                    event_id: Some("evt-packet-repeat-edit".into()),
                    agent: "codex".into(),
                    kind: EventKind::FileChange,
                    file: Some("src/parser.rs".into()),
                    rationale: Some("Try another parser tweak.".into()),
                    ..Event::default()
                })
                .expect("edit");
        }
    }

    let advice = advise_workspace(temp.path()).expect("advice");

    assert_eq!(
        advice.final_action,
        ControlAction::ForceVerification {
            suite: VerificationSuite::Full,
            blocking: true,
        }
    );
    assert!(
        advice.packet.instructions.iter().any(|instruction| {
            instruction.text.contains("same verifier failure signature")
                && instruction.text.contains("failure hypothesis")
                && instruction.text.contains("Stop editing")
        }),
        "packet should require failure isolation before more edits: {:?}",
        advice.packet
    );
}

#[test]
fn case_file_raises_context_entropy_for_repeated_inspection_loop() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    for index in 1..=4 {
        store
            .append_event(&Event {
                time: Some(format!("2026-06-22T12:2{index}:00Z")),
                event_id: Some(format!("evt-read-loop-{index}")),
                agent: "codex".into(),
                kind: EventKind::ToolCall,
                command: Some("Read src/lib.rs".into()),
                content: Some("tool command: Read src/lib.rs".into()),
                ..Event::default()
            })
            .expect("read event");
    }

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);
    let context = case_file
        .entropy
        .score(EntropyKind::Context)
        .expect("context entropy");
    let plan = case_file
        .entropy
        .score(EntropyKind::Plan)
        .expect("plan entropy");

    assert!(
        (75..80).contains(&context.score),
        "rediscovery should be visible without immediately allowing fresh-agent spawn: {context:?}"
    );
    assert!(
        context
            .top_causes
            .iter()
            .any(|cause| cause.contains("repeatedly inspected `Read src/lib.rs`")),
        "context cause should name the repeated target: {context:?}"
    );
    assert!(context.evidence_ids.contains(&"evt-read-loop-4".into()));
    assert!(
        plan.score >= 60,
        "rediscovery should allow a bounded follow-up packet: {plan:?}"
    );
    assert!(
        plan.missing_evidence
            .iter()
            .any(|missing| missing.contains("new hypothesis, edit, or verification")),
        "plan missing evidence should request progress, not more search: {plan:?}"
    );
}

#[test]
fn advise_workspace_sends_run_probe_for_repeated_context_rediscovery() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    for index in 1..=4 {
        store
            .append_event(&Event {
                time: Some(format!("2026-06-22T12:3{index}:00Z")),
                event_id: Some(format!("evt-rediscovery-{index}")),
                agent: "codex".into(),
                kind: EventKind::ToolCall,
                command: Some("Read src/lib.rs".into()),
                content: Some("tool command: Read src/lib.rs".into()),
                ..Event::default()
            })
            .expect("read event");
    }

    let advice = advise_workspace(temp.path()).expect("advice");

    assert_eq!(
        advice.final_action,
        ControlAction::RunProbe {
            probe: coding_agent_monitor::ProbeSpec::RepoInspection {
                target: Some("repeated_inspection_target".into())
            }
        }
    );
    assert!(
        advice
            .packet
            .evidence_refs
            .contains(&"evt-rediscovery-4".into())
    );
}

#[test]
fn advise_workspace_retries_agent_with_loop_breaking_packet_for_repeated_command_loop() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    for index in 1..=3 {
        store
            .append_event(&Event {
                time: Some(format!("2026-06-22T12:0{index}:00Z")),
                event_id: Some(format!("evt-loop-{index}")),
                agent: "codex".into(),
                kind: EventKind::CommandResult,
                command: Some("python scripts/probe.py".into()),
                exit_code: Some(1),
                ..Event::default()
            })
            .expect("event");
    }

    let advice = advise_workspace(temp.path()).expect("advice");

    assert_eq!(
        advice.final_action,
        ControlAction::RetryAgent {
            target_agent: Some("codex".into()),
            max_attempts: 1,
        }
    );
    assert!(
        advice
            .packet
            .instructions
            .iter()
            .any(|instruction| instruction.text.contains("Do not repeat"))
    );
    assert!(
        advice
            .packet
            .instructions
            .iter()
            .any(|instruction| instruction.text.contains("at most 1"))
    );
    let latest_path = temp
        .path()
        .join(".agent-monitor")
        .join("outbox")
        .join("codex")
        .join("latest.md");
    let latest = std::fs::read_to_string(latest_path).expect("latest retry packet");
    assert!(latest.contains("CAM BLOCKING NOTE"));
    assert!(latest.contains("Loop-breaking retry required"));
    assert!(latest.contains("Do not repeat"));
}

#[test]
fn jsonl_repeated_command_failures_trigger_control_retry_packet() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    let input = (1..=3)
        .map(|index| {
            serde_json::to_string(&Event {
                time: Some(format!("2026-06-22T12:0{index}:00Z")),
                event_id: Some(format!("evt-stream-loop-{index}")),
                agent: "codex".into(),
                kind: EventKind::CommandResult,
                command: Some("python scripts/probe.py".into()),
                exit_code: Some(1),
                ..Event::default()
            })
            .expect("event json")
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    let mut output = Vec::new();

    run_jsonl_with_store(input.as_bytes(), &mut output, Config::default(), &mut store)
        .expect("jsonl run should trigger retry advice");

    let advice_log =
        std::fs::read_to_string(store.root().join("advice.jsonl")).expect("advice log");
    let advice_lines = advice_log.lines().collect::<Vec<_>>();
    assert_eq!(advice_lines.len(), 1);
    let advice: coding_agent_monitor::AdviceRun =
        serde_json::from_str(advice_lines[0]).expect("advice json");

    assert_eq!(
        advice.final_action,
        ControlAction::RetryAgent {
            target_agent: Some("codex".into()),
            max_attempts: 1,
        }
    );
    assert_eq!(advice.packet.target_agent, "codex");
    assert!(
        advice
            .packet
            .evidence_refs
            .contains(&"evt-stream-loop-3".into())
    );
    assert!(
        store
            .root()
            .join("outbox")
            .join("codex")
            .join("latest.md")
            .exists()
    );
}

#[test]
fn jsonl_context_compaction_triggers_fresh_agent_packet() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    let input = format!(
        "{}\n",
        serde_json::to_string(&Event {
            time: Some("2026-06-22T12:00:00Z".into()),
            event_id: Some("evt-context-compaction".into()),
            agent: "codex".into(),
            kind: EventKind::AgentHealth,
            content: Some("context compaction occurred; transcript summarized".into()),
            ..Event::default()
        })
        .expect("event json")
    );
    let mut output = Vec::new();

    run_jsonl_with_store(input.as_bytes(), &mut output, Config::default(), &mut store)
        .expect("context compaction should trigger fresh-agent advice");

    let advice_log =
        std::fs::read_to_string(store.root().join("advice.jsonl")).expect("advice log");
    let advice: coding_agent_monitor::AdviceRun =
        serde_json::from_str(advice_log.lines().next().expect("one advice")).expect("advice json");

    assert!(matches!(
        advice.final_action,
        ControlAction::SpawnFreshAgent { .. }
    ));
    assert!(
        advice
            .control_rationale
            .dominant_entropy
            .is_some_and(|kind| kind == EntropyKind::Context)
    );
    assert!(
        advice
            .packet
            .evidence_refs
            .contains(&"evt-context-compaction".into())
    );
}

#[test]
fn jsonl_target_agent_event_records_spawn_fresh_outcome() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            event_id: Some("evt-context-compaction-handoff-outcome".into()),
            agent: "codex".into(),
            kind: EventKind::AgentHealth,
            content: Some("context compaction occurred; transcript summarized".into()),
            ..Event::default()
        })
        .expect("event");
    let advice = advise_workspace(temp.path()).expect("advice");
    assert!(matches!(
        advice.final_action,
        ControlAction::SpawnFreshAgent { .. }
    ));
    let target_agent = advice.packet.target_agent.clone();
    let target_event = Event {
        event_id: Some("evt-fresh-target-started".into()),
        agent: target_agent.clone(),
        kind: EventKind::AgentHealth,
        content: Some("session started".into()),
        ..Event::default()
    };
    let input = format!(
        "{}\n",
        serde_json::to_string(&target_event).expect("event json")
    );
    let mut output = Vec::new();

    run_jsonl_with_store(input.as_bytes(), &mut output, Config::default(), &mut store)
        .expect("jsonl run");

    let trails = load_decision_trails(store.root()).expect("trails");
    let trail = trails
        .iter()
        .find(|trail| trail.advice.advice_id == advice.advice_id)
        .expect("spawn_fresh trail");
    let outcome = trail.outcomes.first().expect("spawn_fresh outcome");
    assert_eq!(outcome.action, ControlActionKind::SpawnFreshAgent);
    assert_eq!(outcome.status, OutcomeStatus::Succeeded);
    assert!(
        outcome
            .evidence_ids
            .contains(&"evt-fresh-target-started".into())
    );
    assert!(
        outcome
            .expected_entropy_delta
            .iter()
            .any(|delta| delta.kind == EntropyKind::Context && delta.delta < 0)
    );
}

#[test]
fn jsonl_target_agent_event_records_switch_agent_handoff_outcome() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    for index in 1..=3 {
        store
            .append_event(&Event {
                time: Some(format!("2026-06-22T12:1{index}:00Z")),
                event_id: Some(format!("evt-switch-handoff-service-{index}")),
                agent: "codex".into(),
                kind: EventKind::ModelMessage,
                content: Some("upstream service unavailable while streaming response".into()),
                ..Event::default()
            })
            .expect("event");
    }
    let advice = advise_workspace(temp.path()).expect("advice");
    assert!(matches!(
        advice.final_action,
        ControlAction::SwitchAgent { .. }
    ));
    let target_agent = advice.packet.target_agent.clone();
    let target_event = Event {
        event_id: Some("evt-switch-target-started".into()),
        agent: target_agent.clone(),
        kind: EventKind::AgentHealth,
        content: Some("session started after switch packet".into()),
        ..Event::default()
    };
    let input = format!(
        "{}\n",
        serde_json::to_string(&target_event).expect("event json")
    );
    let mut output = Vec::new();

    run_jsonl_with_store(input.as_bytes(), &mut output, Config::default(), &mut store)
        .expect("jsonl run");

    let trails = load_decision_trails(store.root()).expect("trails");
    let trail = trails
        .iter()
        .find(|trail| trail.advice.advice_id == advice.advice_id)
        .expect("switch_agent trail");
    let outcome = trail.outcomes.first().expect("switch_agent outcome");
    assert_eq!(outcome.action, ControlActionKind::SwitchAgent);
    assert_eq!(outcome.status, OutcomeStatus::Succeeded);
    assert!(
        outcome
            .evidence_ids
            .contains(&"evt-switch-target-started".into())
    );
    assert!(
        outcome
            .expected_entropy_delta
            .iter()
            .any(|delta| delta.kind == EntropyKind::AgentHealth && delta.delta < 0)
    );
}

#[test]
fn jsonl_target_agent_error_records_failed_handoff_outcome() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            event_id: Some("evt-context-compaction-handoff-failure".into()),
            agent: "codex".into(),
            kind: EventKind::AgentHealth,
            content: Some("context compaction occurred; transcript summarized".into()),
            ..Event::default()
        })
        .expect("event");
    let advice = advise_workspace(temp.path()).expect("advice");
    assert!(matches!(
        advice.final_action,
        ControlAction::SpawnFreshAgent { .. }
    ));
    let target_event = Event {
        event_id: Some("evt-fresh-target-crashed".into()),
        agent: advice.packet.target_agent.clone(),
        kind: EventKind::AgentHealth,
        content: Some("session error [process_crash]: agent process exited".into()),
        ..Event::default()
    };
    let input = format!(
        "{}\n",
        serde_json::to_string(&target_event).expect("event json")
    );
    let mut output = Vec::new();

    run_jsonl_with_store(input.as_bytes(), &mut output, Config::default(), &mut store)
        .expect("jsonl run");

    let trails = load_decision_trails(store.root()).expect("trails");
    let trail = trails
        .iter()
        .find(|trail| trail.advice.advice_id == advice.advice_id)
        .expect("spawn_fresh trail");
    let outcome = trail.outcomes.first().expect("handoff outcome");
    assert_eq!(outcome.action, ControlActionKind::SpawnFreshAgent);
    assert_eq!(outcome.status, OutcomeStatus::Failed);
    assert!(
        outcome
            .observed_entropy_delta
            .iter()
            .any(|delta| delta.kind == EntropyKind::Context && delta.delta >= 0)
    );
    let reacquired = store
        .try_acquire_worktree_lock(&WorktreeLockRequest {
            worktree: temp.path().display().to_string(),
            owner_agent: "codex".into(),
            session: Some("after-failed-handoff".into()),
        })
        .expect("reacquire after failed handoff");
    assert!(matches!(reacquired, WorktreeLockResult::Acquired(_)));
    let lock_log =
        std::fs::read_to_string(store.root().join("locks.jsonl")).expect("lock log should exist");
    assert!(lock_log.contains("\"kind\":\"released\""));
}

#[test]
fn advise_workspace_records_failed_handoff_timeout_outcome() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    std::fs::write(
        store.root().join("config.json"),
        r#"{
          "policy": {
            "handoff_outcome_timeout_secs": 1
          }
        }"#,
    )
    .expect("config");
    let handoff_advice = append_dispatched_action_advice_at(
        &mut store,
        temp.path(),
        "1970-01-01T00:00:00Z",
        ControlAction::SpawnFreshAgent {
            target_agent: Some("claude-code".into()),
        },
        "Fresh agent handoff",
    );
    let acquired = store
        .try_acquire_worktree_lock(&WorktreeLockRequest {
            worktree: temp.path().display().to_string(),
            owner_agent: "claude-code".into(),
            session: Some("timed-out-handoff".into()),
        })
        .expect("acquire handoff lock");
    assert!(matches!(acquired, WorktreeLockResult::Acquired(_)));
    drop(store);

    advise_workspace(temp.path()).expect("advice");

    let mut store = ProjectStore::open(temp.path()).expect("store");
    let trails = load_decision_trails(store.root()).expect("trails");
    let trail = trails
        .iter()
        .find(|trail| trail.advice.advice_id == handoff_advice.advice_id)
        .expect("handoff trail");
    let outcome = trail.outcomes.first().expect("handoff timeout outcome");
    assert_eq!(outcome.action, ControlActionKind::SpawnFreshAgent);
    assert_eq!(outcome.status, OutcomeStatus::Failed);
    assert!(
        outcome
            .evidence_ids
            .contains(&handoff_advice.dispatch_result.dispatch_id)
    );
    assert!(
        outcome
            .note
            .as_deref()
            .is_some_and(|note| note.contains("timed out"))
    );
    let reacquired = store
        .try_acquire_worktree_lock(&WorktreeLockRequest {
            worktree: temp.path().display().to_string(),
            owner_agent: "codex".into(),
            session: Some("after-timeout".into()),
        })
        .expect("reacquire after timed-out handoff");
    assert!(matches!(reacquired, WorktreeLockResult::Acquired(_)));
    let lock_log =
        std::fs::read_to_string(store.root().join("locks.jsonl")).expect("lock log should exist");
    assert!(lock_log.contains("\"kind\":\"released\""));
}

#[test]
fn advise_workspace_does_not_timeout_handoff_after_target_agent_activity() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    std::fs::write(
        store.root().join("config.json"),
        r#"{
          "policy": {
            "handoff_outcome_timeout_secs": 1
          }
        }"#,
    )
    .expect("config");
    let handoff_advice = append_dispatched_action_advice_at(
        &mut store,
        temp.path(),
        "1970-01-01T00:00:00Z",
        ControlAction::SpawnFreshAgent {
            target_agent: Some("claude-code".into()),
        },
        "Fresh agent handoff",
    );
    store
        .append_event(&Event {
            time: Some("1970-01-01T00:00:02Z".into()),
            event_id: Some("evt-target-agent-after-handoff-no-timeout".into()),
            agent: "claude-code".into(),
            kind: EventKind::AgentHealth,
            content: Some("session started after handoff".into()),
            ..Event::default()
        })
        .expect("target event");
    drop(store);

    advise_workspace(temp.path()).expect("advice");

    let store = ProjectStore::open(temp.path()).expect("store");
    let trails = load_decision_trails(store.root()).expect("trails");
    let trail = trails
        .iter()
        .find(|trail| trail.advice.advice_id == handoff_advice.advice_id)
        .expect("handoff trail");
    assert!(trail.outcomes.is_empty());
}

#[test]
fn jsonl_old_agent_event_does_not_record_handoff_outcome() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            event_id: Some("evt-context-compaction-no-handoff-outcome".into()),
            agent: "codex".into(),
            kind: EventKind::AgentHealth,
            content: Some("context compaction occurred; transcript summarized".into()),
            ..Event::default()
        })
        .expect("event");
    let advice = advise_workspace(temp.path()).expect("advice");
    assert!(matches!(
        advice.final_action,
        ControlAction::SpawnFreshAgent { .. }
    ));
    let old_agent_event = Event {
        event_id: Some("evt-old-agent-after-handoff".into()),
        agent: "codex".into(),
        kind: EventKind::ModelMessage,
        content: Some("Still here after the handoff packet.".into()),
        ..Event::default()
    };
    let input = format!(
        "{}\n",
        serde_json::to_string(&old_agent_event).expect("event json")
    );
    let mut output = Vec::new();

    run_jsonl_with_store(input.as_bytes(), &mut output, Config::default(), &mut store)
        .expect("jsonl run");

    let trails = load_decision_trails(store.root()).expect("trails");
    let trail = trails
        .iter()
        .find(|trail| trail.advice.advice_id == advice.advice_id)
        .expect("spawn_fresh trail");
    assert!(trail.outcomes.is_empty());
}

#[test]
fn advise_workspace_prefers_loop_breaking_retry_over_fresh_spawn_when_both_reduce_entropy() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:00:00Z".into()),
            event_id: Some("evt-lost-memory".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some("I lost design memory and need the project context again.".into()),
            ..Event::default()
        })
        .expect("event");
    for index in 1..=3 {
        store
            .append_event(&Event {
                time: Some(format!("2026-06-22T12:0{}:00Z", index + 1)),
                event_id: Some(format!("evt-loop-{index}")),
                agent: "codex".into(),
                kind: EventKind::CommandResult,
                command: Some("python scripts/probe.py".into()),
                exit_code: Some(1),
                ..Event::default()
            })
            .expect("event");
    }

    let advice = advise_workspace(temp.path()).expect("advice");

    assert_eq!(
        advice.final_action,
        ControlAction::RetryAgent {
            target_agent: Some("codex".into()),
            max_attempts: 1,
        }
    );
    assert!(
        advice
            .control_rationale
            .reason
            .contains("expected entropy reduction")
    );
    assert!(
        advice
            .control_rationale
            .expected_entropy_delta
            .iter()
            .any(|delta| delta.kind == EntropyKind::AgentHealth && delta.delta < 0)
    );
}

#[test]
fn advise_workspace_uses_calibrated_entropy_delta_for_successful_fresh_agent_target() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    for index in 1..=3 {
        let built_at = format!("1970-01-01T00:03:0{index}Z");
        append_action_advice_at(
            &mut store,
            temp.path(),
            &built_at,
            ControlAction::SpawnFreshAgent {
                target_agent: Some("claude-code".into()),
            },
            "Fresh agent handoff",
        );
        store
            .append_action_outcome(&ActionOutcome {
                outcome_id: format!("outcome-success-claude-spawn-{index}"),
                advice_id: format!("advice-action-{built_at}"),
                action: ControlActionKind::SpawnFreshAgent,
                status: OutcomeStatus::Succeeded,
                expected_entropy_delta: vec![EntropyDelta {
                    kind: EntropyKind::Context,
                    delta: -50,
                }],
                observed_entropy_delta: vec![EntropyDelta {
                    kind: EntropyKind::Context,
                    delta: -75,
                }],
                observed_entropy_delta_evidence: Vec::new(),
                evidence_ids: vec![format!("evt-success-claude-spawn-{index}")],
                requirement_ids: Vec::new(),
                note: Some("fresh Claude Code session restored missing context".into()),
            })
            .expect("successful spawn outcome");
    }
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:00:00Z".into()),
            event_id: Some("evt-calibrated-lost-memory".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some("I lost design memory and need the project context again.".into()),
            ..Event::default()
        })
        .expect("event");
    for index in 1..=3 {
        store
            .append_event(&Event {
                time: Some(format!("2026-06-22T12:0{}:00Z", index + 1)),
                event_id: Some(format!("evt-calibrated-loop-{index}")),
                agent: "codex".into(),
                kind: EventKind::CommandResult,
                command: Some("python scripts/probe.py".into()),
                exit_code: Some(1),
                ..Event::default()
            })
            .expect("event");
    }

    let advice = advise_workspace(temp.path()).expect("advice");

    assert_eq!(
        advice.final_action,
        ControlAction::SpawnFreshAgent {
            target_agent: Some("claude-code".into()),
        }
    );
    assert!(
        advice
            .control_rationale
            .reason
            .contains("calibrated expected deltas")
    );
    assert!(
        !advice
            .control_rationale
            .reason
            .contains("calibration penalty")
    );
    let context_delta = advice
        .control_rationale
        .expected_entropy_delta
        .iter()
        .find(|delta| delta.kind == EntropyKind::Context)
        .expect("context delta");
    assert!(context_delta.delta <= -60, "{context_delta:?}");
    assert!(context_delta.delta > -75, "{context_delta:?}");
}

#[test]
fn jsonl_successful_command_records_retry_agent_outcome() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    for index in 1..=3 {
        store
            .append_event(&Event {
                time: Some(format!("2026-06-22T12:0{index}:00Z")),
                event_id: Some(format!("evt-retry-success-loop-{index}")),
                agent: "codex".into(),
                kind: EventKind::CommandResult,
                command: Some("python scripts/probe.py".into()),
                exit_code: Some(1),
                ..Event::default()
            })
            .expect("event");
    }
    let advice = advise_workspace(temp.path()).expect("advice");
    let recovery_event = Event {
        time: Some("2026-06-22T12:05:00Z".into()),
        event_id: Some("evt-retry-success-recovered".into()),
        agent: "codex".into(),
        kind: EventKind::CommandResult,
        command: Some("python scripts/probe.py".into()),
        exit_code: Some(0),
        ..Event::default()
    };
    let input = format!(
        "{}\n",
        serde_json::to_string(&recovery_event).expect("event json")
    );
    let mut output = Vec::new();

    run_jsonl_with_store(input.as_bytes(), &mut output, Config::default(), &mut store)
        .expect("jsonl run");

    let trails = load_decision_trails(store.root()).expect("trails");
    let trail = trails
        .iter()
        .find(|trail| trail.advice.advice_id == advice.advice_id)
        .expect("retry trail");
    let outcome = trail.outcomes.first().expect("retry outcome");
    assert_eq!(outcome.action, ControlActionKind::RetryAgent);
    assert_eq!(outcome.status, OutcomeStatus::Succeeded);
    assert!(
        outcome
            .evidence_ids
            .contains(&"evt-retry-success-recovered".into())
    );
    assert!(
        outcome
            .expected_entropy_delta
            .iter()
            .any(|delta| { delta.kind == EntropyKind::AgentHealth && delta.delta == -35 })
    );
    assert!(
        outcome
            .observed_entropy_delta
            .iter()
            .any(|delta| { delta.kind == EntropyKind::AgentHealth && delta.delta < 0 })
    );
    let agent_health_delta_evidence = outcome
        .observed_entropy_delta_evidence
        .iter()
        .find(|evidence| evidence.kind == EntropyKind::AgentHealth)
        .expect("agent health delta evidence");
    assert!(
        agent_health_delta_evidence
            .evidence_ids
            .contains(&"evt-retry-success-recovered".into())
    );
    assert!(
        agent_health_delta_evidence
            .cause_evidence_ids
            .contains(&"evt-retry-success-loop-3".into())
    );
    assert!(
        agent_health_delta_evidence
            .result_evidence_ids
            .contains(&"evt-retry-success-recovered".into())
    );
}

#[test]
fn jsonl_user_instruction_records_ask_user_outcome() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            event_id: Some("evt-needs-credential-outcome".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some(
                "Need credentials from the user before calling the external billing API.".into(),
            ),
            ..Event::default()
        })
        .expect("event");
    let advice = advise_workspace(temp.path()).expect("advice");
    assert!(matches!(advice.final_action, ControlAction::AskUser { .. }));

    let response_event = Event {
        event_id: Some("evt-user-authorized-outcome".into()),
        agent: "user".into(),
        kind: EventKind::UserInstruction,
        content: Some("Use the configured billing sandbox token and continue.".into()),
        ..Event::default()
    };
    let input = format!(
        "{}\n",
        serde_json::to_string(&response_event).expect("event json")
    );
    let mut output = Vec::new();

    run_jsonl_with_store(input.as_bytes(), &mut output, Config::default(), &mut store)
        .expect("jsonl run");

    let trails = load_decision_trails(store.root()).expect("trails");
    let trail = trails
        .iter()
        .find(|trail| trail.advice.advice_id == advice.advice_id)
        .expect("ask_user trail");
    let outcome = trail.outcomes.first().expect("ask_user outcome");
    assert_eq!(outcome.action, ControlActionKind::AskUser);
    assert_eq!(outcome.status, OutcomeStatus::Succeeded);
    assert!(
        outcome
            .evidence_ids
            .contains(&"evt-user-authorized-outcome".into())
    );
    assert!(
        outcome
            .expected_entropy_delta
            .iter()
            .any(|delta| delta.kind == EntropyKind::UserDecision && delta.delta == -70)
    );
    assert!(
        outcome
            .observed_entropy_delta
            .iter()
            .any(|delta| delta.kind == EntropyKind::UserDecision)
    );
}

#[test]
fn jsonl_non_user_instruction_does_not_record_ask_user_outcome() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            event_id: Some("evt-needs-user-decision-no-outcome".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some("Need credentials from the user before continuing.".into()),
            ..Event::default()
        })
        .expect("event");
    let advice = advise_workspace(temp.path()).expect("advice");
    assert!(matches!(advice.final_action, ControlAction::AskUser { .. }));

    let agent_event = Event {
        event_id: Some("evt-agent-after-ask-user".into()),
        agent: "codex".into(),
        kind: EventKind::ModelMessage,
        content: Some("Waiting for the user decision.".into()),
        ..Event::default()
    };
    let input = format!(
        "{}\n",
        serde_json::to_string(&agent_event).expect("event json")
    );
    let mut output = Vec::new();

    run_jsonl_with_store(input.as_bytes(), &mut output, Config::default(), &mut store)
        .expect("jsonl run");

    let trails = load_decision_trails(store.root()).expect("trails");
    let trail = trails
        .iter()
        .find(|trail| trail.advice.advice_id == advice.advice_id)
        .expect("ask_user trail");
    assert!(trail.outcomes.is_empty());
}

#[test]
fn jsonl_target_agent_event_records_send_follow_up_outcome() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            event_id: Some("evt-follow-up-needed".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some("This is a good point to stop while obvious work remains.".into()),
            ..Event::default()
        })
        .expect("event");
    let advice = advise_workspace(temp.path()).expect("advice");
    assert_eq!(
        advice.final_action,
        ControlAction::SendFollowUp { target_agent: None }
    );
    let target_event = Event {
        event_id: Some("evt-follow-up-agent-continued".into()),
        agent: advice.packet.target_agent.clone(),
        kind: EventKind::ModelMessage,
        content: Some("Continuing with the bounded next step from the monitor packet.".into()),
        ..Event::default()
    };
    let input = format!(
        "{}\n",
        serde_json::to_string(&target_event).expect("event json")
    );
    let mut output = Vec::new();

    run_jsonl_with_store(input.as_bytes(), &mut output, Config::default(), &mut store)
        .expect("jsonl run");

    let trails = load_decision_trails(store.root()).expect("trails");
    let trail = trails
        .iter()
        .find(|trail| trail.advice.advice_id == advice.advice_id)
        .expect("send_follow_up trail");
    let outcome = trail.outcomes.first().expect("send_follow_up outcome");
    assert_eq!(outcome.action, ControlActionKind::SendFollowUp);
    assert_eq!(outcome.status, OutcomeStatus::Succeeded);
    assert!(
        outcome
            .evidence_ids
            .contains(&"evt-follow-up-agent-continued".into())
    );
    assert!(
        outcome
            .expected_entropy_delta
            .iter()
            .any(|delta| delta.kind == EntropyKind::Plan && delta.delta < 0)
    );
}

#[test]
fn jsonl_target_agent_event_records_spawn_judge_outcome() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    let advice = append_dispatched_action_advice_at(
        &mut store,
        temp.path(),
        "2026-06-22T12:00:00Z",
        ControlAction::SpawnJudgeAgent {
            target_agent: Some("claude-code".into()),
        },
        "Read-only judge review required",
    );
    let judge_event = Event {
        event_id: Some("evt-judge-agent-reviewed".into()),
        agent: "claude-code".into(),
        kind: EventKind::ModelMessage,
        content: Some("Read-only judge review: keep the change, but add trace rationale.".into()),
        ..Event::default()
    };
    let input = format!(
        "{}\n",
        serde_json::to_string(&judge_event).expect("event json")
    );
    let mut output = Vec::new();

    run_jsonl_with_store(input.as_bytes(), &mut output, Config::default(), &mut store)
        .expect("jsonl run");

    let trails = load_decision_trails(store.root()).expect("trails");
    let trail = trails
        .iter()
        .find(|trail| trail.advice.advice_id == advice.advice_id)
        .expect("spawn_judge trail");
    let outcome = trail.outcomes.first().expect("spawn_judge outcome");
    assert_eq!(outcome.action, ControlActionKind::SpawnJudgeAgent);
    assert_eq!(outcome.status, OutcomeStatus::Succeeded);
    assert!(
        outcome
            .evidence_ids
            .contains(&"evt-judge-agent-reviewed".into())
    );
    assert!(
        outcome
            .expected_entropy_delta
            .iter()
            .any(|delta| delta.kind == EntropyKind::RepoBlame && delta.delta < 0)
    );
}

#[test]
fn jsonl_target_agent_lifecycle_does_not_satisfy_spawn_judge_outcome() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    let advice = append_dispatched_action_advice_at(
        &mut store,
        temp.path(),
        "2026-06-22T12:00:01Z",
        ControlAction::SpawnJudgeAgent {
            target_agent: Some("claude-code".into()),
        },
        "Read-only judge review required",
    );
    let judge_event = Event {
        event_id: Some("evt-judge-agent-started".into()),
        agent: "claude-code".into(),
        kind: EventKind::AgentHealth,
        content: Some("session started".into()),
        ..Event::default()
    };
    let input = format!(
        "{}\n",
        serde_json::to_string(&judge_event).expect("event json")
    );
    let mut output = Vec::new();

    run_jsonl_with_store(input.as_bytes(), &mut output, Config::default(), &mut store)
        .expect("jsonl run");

    let trails = load_decision_trails(store.root()).expect("trails");
    let trail = trails
        .iter()
        .find(|trail| trail.advice.advice_id == advice.advice_id)
        .expect("spawn_judge trail");
    assert!(trail.outcomes.is_empty());
}

#[test]
fn jsonl_target_agent_file_change_records_failed_spawn_judge_outcome() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    let advice = append_dispatched_action_advice_at(
        &mut store,
        temp.path(),
        "2026-06-22T12:00:02Z",
        ControlAction::SpawnJudgeAgent {
            target_agent: Some("claude-code".into()),
        },
        "Read-only judge review required",
    );
    let judge_event = Event {
        event_id: Some("evt-judge-agent-edited".into()),
        agent: "claude-code".into(),
        kind: EventKind::FileChange,
        file: Some("src/lib.rs".into()),
        content: Some("judge edited file during read-only review".into()),
        ..Event::default()
    };
    let input = format!(
        "{}\n",
        serde_json::to_string(&judge_event).expect("event json")
    );
    let mut output = Vec::new();

    run_jsonl_with_store(input.as_bytes(), &mut output, Config::default(), &mut store)
        .expect("jsonl run");

    let trails = load_decision_trails(store.root()).expect("trails");
    let trail = trails
        .iter()
        .find(|trail| trail.advice.advice_id == advice.advice_id)
        .expect("spawn_judge trail");
    let outcome = trail.outcomes.first().expect("spawn_judge outcome");
    assert_eq!(outcome.action, ControlActionKind::SpawnJudgeAgent);
    assert_eq!(outcome.status, OutcomeStatus::Failed);
    assert!(
        outcome
            .evidence_ids
            .contains(&"evt-judge-agent-edited".into())
    );
}

#[test]
fn jsonl_other_agent_event_does_not_record_send_follow_up_outcome() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            event_id: Some("evt-follow-up-needed-other-agent".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some("This is a good point to stop while obvious work remains.".into()),
            ..Event::default()
        })
        .expect("event");
    let advice = advise_workspace(temp.path()).expect("advice");
    assert_eq!(
        advice.final_action,
        ControlAction::SendFollowUp { target_agent: None }
    );
    let other_agent_event = Event {
        event_id: Some("evt-follow-up-other-agent".into()),
        agent: "user".into(),
        kind: EventKind::UserInstruction,
        content: Some("Continue.".into()),
        ..Event::default()
    };
    let input = format!(
        "{}\n",
        serde_json::to_string(&other_agent_event).expect("event json")
    );
    let mut output = Vec::new();

    run_jsonl_with_store(input.as_bytes(), &mut output, Config::default(), &mut store)
        .expect("jsonl run");

    let trails = load_decision_trails(store.root()).expect("trails");
    let trail = trails
        .iter()
        .find(|trail| trail.advice.advice_id == advice.advice_id)
        .expect("send_follow_up trail");
    assert!(trail.outcomes.is_empty());
}

#[test]
fn jsonl_unrelated_success_does_not_mask_retry_agent_failure() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    for index in 1..=3 {
        store
            .append_event(&Event {
                time: Some(format!("2026-06-22T12:0{index}:00Z")),
                event_id: Some(format!("evt-retry-unrelated-loop-{index}")),
                agent: "codex".into(),
                kind: EventKind::CommandResult,
                command: Some("python scripts/probe.py".into()),
                exit_code: Some(1),
                ..Event::default()
            })
            .expect("event");
    }
    let advice = advise_workspace(temp.path()).expect("advice");
    let unrelated_success = Event {
        time: Some("2026-06-22T12:05:00Z".into()),
        event_id: Some("evt-retry-unrelated-success".into()),
        agent: "codex".into(),
        kind: EventKind::CommandResult,
        command: Some("echo ok".into()),
        exit_code: Some(0),
        ..Event::default()
    };
    let input = format!(
        "{}\n",
        serde_json::to_string(&unrelated_success).expect("event json")
    );
    let mut output = Vec::new();

    run_jsonl_with_store(input.as_bytes(), &mut output, Config::default(), &mut store)
        .expect("jsonl run");

    let trails = load_decision_trails(store.root()).expect("trails");
    let trail = trails
        .iter()
        .find(|trail| trail.advice.advice_id == advice.advice_id)
        .expect("retry trail");
    assert!(trail.outcomes.is_empty());

    let repeated_failure = Event {
        time: Some("2026-06-22T12:06:00Z".into()),
        event_id: Some("evt-retry-unrelated-repeat".into()),
        agent: "codex".into(),
        kind: EventKind::CommandResult,
        command: Some("python scripts/probe.py".into()),
        exit_code: Some(1),
        ..Event::default()
    };
    let input = format!(
        "{}\n",
        serde_json::to_string(&repeated_failure).expect("event json")
    );

    run_jsonl_with_store(input.as_bytes(), &mut output, Config::default(), &mut store)
        .expect("jsonl run");

    let trails = load_decision_trails(store.root()).expect("trails");
    let trail = trails
        .iter()
        .find(|trail| trail.advice.advice_id == advice.advice_id)
        .expect("retry trail");
    let outcome = trail.outcomes.first().expect("retry outcome");
    assert_eq!(outcome.status, OutcomeStatus::Failed);
    assert!(
        outcome
            .evidence_ids
            .contains(&"evt-retry-unrelated-repeat".into())
    );
}

#[test]
fn retry_outcome_expected_delta_adds_measured_agent_health_kind() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    for index in 1..=3 {
        store
            .append_event(&Event {
                time: Some(format!("2026-06-22T12:2{index}:00Z")),
                event_id: Some(format!("evt-retry-advisor-delta-loop-{index}")),
                agent: "codex".into(),
                kind: EventKind::CommandResult,
                command: Some("python scripts/probe.py".into()),
                exit_code: Some(1),
                ..Event::default()
            })
            .expect("event");
    }
    let decision = json!({
        "diagnosis_id": "diagnosis-retry-delta-kind",
        "dominant_entropy": "agent_health",
        "entropy_scores": {
            "agent_health": { "score": 82, "confidence": 88 }
        },
        "top_evidence": [
            {
                "event_id": "evt-retry-advisor-delta-loop-3",
                "why_it_matters": "The same command failed repeatedly."
            }
        ],
        "cited_evidence_ids": ["evt-retry-advisor-delta-loop-3"],
        "missing_evidence": ["loop-breaking retry result"],
        "proposed_action": {
            "type": "retry_agent",
            "target_agent": "codex",
            "max_attempts": 1
        },
        "expected_entropy_delta": [
            { "kind": "context", "delta": -20 }
        ],
        "packet_intent": "break the repeated command loop",
        "packet_draft": {
            "urgency": "urgent",
            "summary": "Break the repeated command loop.",
            "instructions": ["Do not repeat the same failing command without changing approach."],
            "evidence_refs": ["evt-retry-advisor-delta-loop-3"]
        },
        "ask_user": null,
        "confidence": 0.78
    });
    let (endpoint, _request_rx) = serve_advisor_once(decision);
    let env_name = "CAM_TEST_ADVISOR_KEY_RETRY_DELTA_KIND";
    set_test_env_var(env_name, "test-key");
    std::fs::write(
        store.root().join("config.json"),
        json!({
            "advisor": {
                "enabled": true,
                "provider": {
                    "endpoint": endpoint,
                    "model": "test-advisor",
                    "api_key_env": env_name,
                    "timeout_secs": 5
                }
            }
        })
        .to_string(),
    )
    .expect("config");
    let advice = advise_workspace(temp.path()).expect("advice");
    let recovery_event = Event {
        time: Some("2026-06-22T12:25:00Z".into()),
        event_id: Some("evt-retry-advisor-delta-recovered".into()),
        agent: "codex".into(),
        kind: EventKind::CommandResult,
        command: Some("python scripts/probe.py".into()),
        exit_code: Some(0),
        ..Event::default()
    };
    let input = format!(
        "{}\n",
        serde_json::to_string(&recovery_event).expect("event json")
    );
    let mut output = Vec::new();

    run_jsonl_with_store(input.as_bytes(), &mut output, Config::default(), &mut store)
        .expect("jsonl run");

    let trails = load_decision_trails(store.root()).expect("trails");
    let trail = trails
        .iter()
        .find(|trail| trail.advice.advice_id == advice.advice_id)
        .expect("retry trail");
    let outcome = trail.outcomes.first().expect("retry outcome");
    assert!(
        outcome
            .expected_entropy_delta
            .iter()
            .any(|delta| { delta.kind == EntropyKind::Context && delta.delta == -20 })
    );
    assert!(
        outcome
            .expected_entropy_delta
            .iter()
            .any(|delta| { delta.kind == EntropyKind::AgentHealth && delta.delta == -35 })
    );
    assert!(
        outcome
            .observed_entropy_delta
            .iter()
            .any(|delta| delta.kind == EntropyKind::Context)
    );
    assert!(
        outcome
            .observed_entropy_delta
            .iter()
            .any(|delta| delta.kind == EntropyKind::AgentHealth)
    );
}

#[test]
fn jsonl_repeated_same_failure_records_failed_retry_agent_outcome() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    for index in 1..=3 {
        store
            .append_event(&Event {
                time: Some(format!("2026-06-22T12:1{index}:00Z")),
                event_id: Some(format!("evt-retry-failed-loop-{index}")),
                agent: "codex".into(),
                kind: EventKind::CommandResult,
                command: Some("python scripts/probe.py".into()),
                exit_code: Some(1),
                ..Event::default()
            })
            .expect("event");
    }
    let advice = advise_workspace(temp.path()).expect("advice");
    let repeated_failure = Event {
        time: Some("2026-06-22T12:15:00Z".into()),
        event_id: Some("evt-retry-failed-repeat".into()),
        agent: "codex".into(),
        kind: EventKind::CommandResult,
        command: Some("python scripts/probe.py".into()),
        exit_code: Some(1),
        ..Event::default()
    };
    let input = format!(
        "{}\n",
        serde_json::to_string(&repeated_failure).expect("event json")
    );
    let mut output = Vec::new();

    run_jsonl_with_store(input.as_bytes(), &mut output, Config::default(), &mut store)
        .expect("jsonl run");

    let trails = load_decision_trails(store.root()).expect("trails");
    let trail = trails
        .iter()
        .find(|trail| trail.advice.advice_id == advice.advice_id)
        .expect("retry trail");
    let outcome = trail.outcomes.first().expect("retry outcome");
    assert_eq!(outcome.action, ControlActionKind::RetryAgent);
    assert_eq!(outcome.status, OutcomeStatus::Failed);
    assert!(
        outcome
            .evidence_ids
            .contains(&"evt-retry-failed-repeat".into())
    );
    assert!(
        outcome
            .observed_entropy_delta
            .iter()
            .any(|delta| { delta.kind == EntropyKind::AgentHealth && delta.delta >= 0 })
    );
}

#[test]
fn advise_workspace_retries_the_agent_that_repeated_the_failing_command() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:00:00Z".into()),
            event_id: Some("evt-codex-active".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some("I am still active.".into()),
            ..Event::default()
        })
        .expect("event");
    for index in 1..=3 {
        store
            .append_event(&Event {
                time: Some(format!("2026-06-22T12:1{index}:00Z")),
                event_id: Some(format!("evt-claude-loop-{index}")),
                agent: "claude-code".into(),
                kind: EventKind::CommandResult,
                command: Some("python scripts/probe.py".into()),
                exit_code: Some(1),
                ..Event::default()
            })
            .expect("event");
    }

    let advice = advise_workspace(temp.path()).expect("advice");

    assert_eq!(
        advice.final_action,
        ControlAction::RetryAgent {
            target_agent: Some("claude-code".into()),
            max_attempts: 1,
        }
    );
    assert_eq!(advice.packet.target_agent, "claude-code");
}

#[test]
fn validator_targets_advisor_retry_at_unhealthy_agent_when_target_is_missing() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:00:00Z".into()),
            event_id: Some("evt-codex-active".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some("I am still active.".into()),
            ..Event::default()
        })
        .expect("event");
    for index in 1..=3 {
        store
            .append_event(&Event {
                time: Some(format!("2026-06-22T12:1{index}:00Z")),
                event_id: Some(format!("evt-claude-loop-{index}")),
                agent: "claude-code".into(),
                kind: EventKind::CommandResult,
                command: Some("python scripts/probe.py".into()),
                exit_code: Some(1),
                ..Event::default()
            })
            .expect("event");
    }
    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);

    let outcome = validate_control_action_detailed(
        ControlAction::RetryAgent {
            target_agent: None,
            max_attempts: 1,
        },
        &case_file,
    );

    match outcome {
        ValidationOutcome::Modified { replacement, .. } => {
            assert_eq!(
                replacement,
                ControlAction::RetryAgent {
                    target_agent: Some("claude-code".into()),
                    max_attempts: 1,
                }
            );
        }
        other => panic!("expected retry target fill, got {other:?}"),
    }
}

#[test]
fn validator_replaces_advisor_retry_that_targets_a_different_agent() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:00:00Z".into()),
            event_id: Some("evt-codex-active".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some("I am still active.".into()),
            ..Event::default()
        })
        .expect("event");
    for index in 1..=3 {
        store
            .append_event(&Event {
                time: Some(format!("2026-06-22T12:1{index}:00Z")),
                event_id: Some(format!("evt-claude-loop-{index}")),
                agent: "claude-code".into(),
                kind: EventKind::CommandResult,
                command: Some("python scripts/probe.py".into()),
                exit_code: Some(1),
                ..Event::default()
            })
            .expect("event");
    }
    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);

    let outcome = validate_control_action_detailed(
        ControlAction::RetryAgent {
            target_agent: Some("codex".into()),
            max_attempts: 1,
        },
        &case_file,
    );

    match outcome {
        ValidationOutcome::Modified { replacement, .. } => {
            assert_eq!(
                replacement,
                ControlAction::RetryAgent {
                    target_agent: Some("claude-code".into()),
                    max_attempts: 1,
                }
            );
        }
        other => panic!("expected retry target replacement, got {other:?}"),
    }
}

#[test]
fn repeated_failing_command_entropy_is_cleared_by_later_success() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    for index in 1..=3 {
        store
            .append_event(&Event {
                time: Some(format!("2026-06-22T12:0{index}:00Z")),
                event_id: Some(format!("evt-loop-{index}")),
                agent: "codex".into(),
                kind: EventKind::CommandResult,
                command: Some("python scripts/probe.py".into()),
                exit_code: Some(1),
                ..Event::default()
            })
            .expect("event");
    }
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:10:00Z".into()),
            event_id: Some("evt-loop-recovered".into()),
            agent: "codex".into(),
            kind: EventKind::CommandResult,
            command: Some("python scripts/probe.py".into()),
            exit_code: Some(0),
            ..Event::default()
        })
        .expect("event");

    let advice = advise_workspace(temp.path()).expect("advice");

    assert_eq!(advice.final_action, ControlAction::ContinueWorking);
}

#[test]
fn repeated_failing_command_entropy_is_cleared_by_different_successful_command() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    for index in 1..=3 {
        store
            .append_event(&Event {
                time: Some(format!("2026-06-22T12:0{index}:00Z")),
                event_id: Some(format!("evt-loop-{index}")),
                agent: "codex".into(),
                kind: EventKind::CommandResult,
                command: Some("python scripts/probe.py".into()),
                exit_code: Some(1),
                ..Event::default()
            })
            .expect("event");
    }
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:10:00Z".into()),
            event_id: Some("evt-loop-recovered".into()),
            agent: "codex".into(),
            kind: EventKind::CommandResult,
            command: Some("cargo test".into()),
            exit_code: Some(0),
            ..Event::default()
        })
        .expect("event");

    let advice = advise_workspace(temp.path()).expect("advice");

    assert_eq!(advice.final_action, ControlAction::ContinueWorking);
}

#[test]
fn advise_workspace_switches_agent_after_repeated_service_failures() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    for index in 1..=3 {
        store
            .append_event(&Event {
                time: Some(format!("2026-06-22T12:1{index}:00Z")),
                event_id: Some(format!("evt-service-{index}")),
                agent: "codex".into(),
                kind: EventKind::ModelMessage,
                content: Some("upstream service unavailable while streaming response".into()),
                ..Event::default()
            })
            .expect("event");
    }

    let advice = advise_workspace(temp.path()).expect("advice");

    assert_eq!(
        advice.final_action,
        ControlAction::SwitchAgent {
            target_agent: "claude-code".into(),
        }
    );
    let agent_health = advice
        .packet
        .evidence_refs
        .iter()
        .any(|evidence_id| evidence_id == "evt-service-3");
    assert!(agent_health);
}

#[test]
fn advise_workspace_demotes_soft_action_with_poor_calibration_history() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    std::fs::write(
        store.root().join("config.json"),
        r#"{
          "policy": {
            "switch_agent_cooldown_min": 0,
            "spawn_fresh_cooldown_min": 0
          }
        }"#,
    )
    .expect("config");
    for index in 1..=3 {
        let built_at = format!("1970-01-01T00:00:0{index}Z");
        append_action_advice_at(
            &mut store,
            temp.path(),
            &built_at,
            ControlAction::SwitchAgent {
                target_agent: "claude-code".into(),
            },
            "Switch agent",
        );
        store
            .append_action_outcome(&ActionOutcome {
                outcome_id: format!("outcome-failed-switch-{index}"),
                advice_id: format!("advice-action-{built_at}"),
                action: ControlActionKind::SwitchAgent,
                status: OutcomeStatus::Failed,
                expected_entropy_delta: vec![EntropyDelta {
                    kind: EntropyKind::AgentHealth,
                    delta: -45,
                }],
                observed_entropy_delta: vec![EntropyDelta {
                    kind: EntropyKind::AgentHealth,
                    delta: 0,
                }],
                observed_entropy_delta_evidence: Vec::new(),
                evidence_ids: vec![format!("evt-failed-switch-{index}")],
                requirement_ids: Vec::new(),
                note: Some("historical switch did not recover the agent session".into()),
            })
            .expect("outcome");
    }
    for index in 1..=3 {
        store
            .append_event(&Event {
                time: Some(format!("2026-06-22T12:3{index}:00Z")),
                event_id: Some(format!("evt-service-calibrated-{index}")),
                agent: "codex".into(),
                kind: EventKind::ModelMessage,
                content: Some("upstream service unavailable while streaming response".into()),
                ..Event::default()
            })
            .expect("event");
    }

    let advice = advise_workspace(temp.path()).expect("advice");

    assert_eq!(
        advice.final_action,
        ControlAction::RetryAgent {
            target_agent: Some("codex".into()),
            max_attempts: 1,
        }
    );
    assert!(
        advice
            .control_rationale
            .reason
            .contains("calibration penalty")
    );
}

#[test]
fn advise_workspace_applies_calibration_penalty_to_the_specific_handoff_target() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    std::fs::write(
        store.root().join("config.json"),
        r#"{
          "policy": {
            "switch_agent_cooldown_min": 0,
            "spawn_fresh_cooldown_min": 0
          }
        }"#,
    )
    .expect("config");
    for index in 1..=3 {
        let built_at = format!("1970-01-01T00:01:0{index}Z");
        append_action_advice_at(
            &mut store,
            temp.path(),
            &built_at,
            ControlAction::SwitchAgent {
                target_agent: "claude-code".into(),
            },
            "Switch agent to Claude",
        );
        store
            .append_action_outcome(&ActionOutcome {
                outcome_id: format!("outcome-failed-claude-switch-{index}"),
                advice_id: format!("advice-action-{built_at}"),
                action: ControlActionKind::SwitchAgent,
                status: OutcomeStatus::Failed,
                expected_entropy_delta: vec![EntropyDelta {
                    kind: EntropyKind::AgentHealth,
                    delta: -45,
                }],
                observed_entropy_delta: vec![EntropyDelta {
                    kind: EntropyKind::AgentHealth,
                    delta: 0,
                }],
                observed_entropy_delta_evidence: Vec::new(),
                evidence_ids: vec![format!("evt-failed-claude-switch-{index}")],
                requirement_ids: Vec::new(),
                note: None,
            })
            .expect("failed claude outcome");
    }
    for index in 1..=3 {
        let built_at = format!("1970-01-01T00:02:0{index}Z");
        append_action_advice_at(
            &mut store,
            temp.path(),
            &built_at,
            ControlAction::SwitchAgent {
                target_agent: "opencode".into(),
            },
            "Switch agent to OpenCode",
        );
        store
            .append_action_outcome(&ActionOutcome {
                outcome_id: format!("outcome-success-opencode-switch-{index}"),
                advice_id: format!("advice-action-{built_at}"),
                action: ControlActionKind::SwitchAgent,
                status: OutcomeStatus::Succeeded,
                expected_entropy_delta: vec![EntropyDelta {
                    kind: EntropyKind::AgentHealth,
                    delta: -45,
                }],
                observed_entropy_delta: vec![EntropyDelta {
                    kind: EntropyKind::AgentHealth,
                    delta: -40,
                }],
                observed_entropy_delta_evidence: Vec::new(),
                evidence_ids: vec![format!("evt-success-opencode-switch-{index}")],
                requirement_ids: Vec::new(),
                note: None,
            })
            .expect("successful opencode outcome");
    }
    for index in 1..=3 {
        store
            .append_event(&Event {
                time: Some(format!("2026-06-22T12:4{index}:00Z")),
                event_id: Some(format!("evt-service-target-calibrated-{index}")),
                agent: "codex".into(),
                kind: EventKind::ModelMessage,
                content: Some("upstream service unavailable while streaming response".into()),
                ..Event::default()
            })
            .expect("event");
    }

    let advice = advise_workspace(temp.path()).expect("advice");

    assert_eq!(
        advice.final_action,
        ControlAction::RetryAgent {
            target_agent: Some("codex".into()),
            max_attempts: 1,
        }
    );
    assert!(advice.control_rationale.reason.contains("claude-code"));
}

#[test]
fn advise_workspace_does_not_switch_to_the_same_agent_after_service_failures() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    for index in 1..=3 {
        store
            .append_event(&Event {
                time: Some(format!("2026-06-22T12:2{index}:00Z")),
                event_id: Some(format!("evt-service-claude-{index}")),
                agent: "claude-code".into(),
                kind: EventKind::ModelMessage,
                content: Some("upstream service unavailable while streaming response".into()),
                ..Event::default()
            })
            .expect("event");
    }

    let advice = advise_workspace(temp.path()).expect("advice");

    assert_eq!(
        advice.final_action,
        ControlAction::SwitchAgent {
            target_agent: "opencode".into(),
        }
    );
    assert_eq!(advice.packet.target_agent, "opencode");
}

#[test]
fn advise_workspace_pauses_switch_agent_when_cooldown_is_active() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    std::fs::write(
        store.root().join("config.json"),
        r#"{
          "policy": {
            "switch_agent_cooldown_min": 30
          }
        }"#,
    )
    .expect("config");
    for index in 1..=3 {
        store
            .append_event(&Event {
                time: Some(format!("2026-06-22T12:1{index}:00Z")),
                event_id: Some(format!("evt-service-cooldown-{index}")),
                agent: "codex".into(),
                kind: EventKind::ModelMessage,
                content: Some("upstream service unavailable while streaming response".into()),
                ..Event::default()
            })
            .expect("event");
    }
    drop(store);

    let first = advise_workspace(temp.path()).expect("first advice");
    assert!(matches!(
        first.final_action,
        ControlAction::SwitchAgent { .. }
    ));
    let second = advise_workspace(temp.path()).expect("second advice");

    match second.final_action {
        ControlAction::Pause { reason } => {
            assert!(reason.contains("switch_agent cooldown"));
        }
        other => panic!("expected cooldown pause, got {other:?}"),
    }
    assert_eq!(second.packet.title, "Monitor paused");
}

#[test]
fn advise_workspace_records_pause_outcome_when_pause_packet_is_dispatched() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    std::fs::write(
        store.root().join("config.json"),
        r#"{
          "policy": {
            "switch_agent_cooldown_min": 30
          }
        }"#,
    )
    .expect("config");
    for index in 1..=3 {
        store
            .append_event(&Event {
                time: Some(format!("2026-06-22T12:1{index}:00Z")),
                event_id: Some(format!("evt-service-pause-outcome-{index}")),
                agent: "codex".into(),
                kind: EventKind::ModelMessage,
                content: Some("upstream service unavailable while streaming response".into()),
                ..Event::default()
            })
            .expect("event");
    }
    drop(store);

    let first = advise_workspace(temp.path()).expect("first advice");
    assert!(matches!(
        first.final_action,
        ControlAction::SwitchAgent { .. }
    ));
    let second = advise_workspace(temp.path()).expect("second advice");
    assert!(matches!(second.final_action, ControlAction::Pause { .. }));

    let store = ProjectStore::open(temp.path()).expect("store");
    let trails = load_decision_trails(store.root()).expect("trails");
    let trail = trails
        .iter()
        .find(|trail| trail.advice.advice_id == second.advice_id)
        .expect("pause trail");
    let outcome = trail.outcomes.first().expect("pause outcome");
    assert_eq!(outcome.action, ControlActionKind::Pause);
    assert_eq!(outcome.status, OutcomeStatus::Succeeded);
    assert!(outcome.expected_entropy_delta.is_empty());
    assert!(outcome.observed_entropy_delta.is_empty());
    assert!(
        outcome
            .evidence_ids
            .contains(&second.dispatch_result.dispatch_id)
    );
    assert!(outcome.note.as_deref().is_some_and(|note| {
        note.contains("pause") && note.contains(&second.dispatch_result.dispatch_id)
    }));
}

#[test]
fn advise_workspace_allows_switch_agent_when_prior_switch_is_outside_cooldown() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    std::fs::write(
        store.root().join("config.json"),
        r#"{
          "policy": {
            "switch_agent_cooldown_min": 30
          }
        }"#,
    )
    .expect("config");
    append_action_advice_at(
        &mut store,
        temp.path(),
        "1970-01-01T00:00:00Z",
        ControlAction::SwitchAgent {
            target_agent: "claude-code".into(),
        },
        "Switch agent",
    );
    for index in 1..=3 {
        store
            .append_event(&Event {
                time: Some(format!("2026-06-22T12:2{index}:00Z")),
                event_id: Some(format!("evt-service-old-cooldown-{index}")),
                agent: "codex".into(),
                kind: EventKind::ModelMessage,
                content: Some("upstream service unavailable while streaming response".into()),
                ..Event::default()
            })
            .expect("event");
    }
    drop(store);

    let advice = advise_workspace(temp.path()).expect("advice");

    assert!(matches!(
        advice.final_action,
        ControlAction::SwitchAgent { .. }
    ));
}

#[test]
fn advisor_request_forbids_switch_agent_when_cooldown_is_active() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    append_action_advice_at(
        &mut store,
        temp.path(),
        "9999-01-01T00:10:00Z",
        ControlAction::SwitchAgent {
            target_agent: "claude-code".into(),
        },
        "Switch agent",
    );
    for index in 1..=3 {
        store
            .append_event(&Event {
                event_id: Some(format!("evt-service-advisor-cooldown-{index}")),
                agent: "codex".into(),
                kind: EventKind::ModelMessage,
                content: Some("upstream service unavailable while streaming response".into()),
                ..Event::default()
            })
            .expect("event");
    }
    let decision = json!({
        "diagnosis_id": "diagnosis-switch-cooldown-pruned",
        "dominant_entropy": "agent_health",
        "entropy_scores": {
            "agent_health": { "score": 80, "confidence": 80 }
        },
        "top_evidence": [],
        "cited_evidence_ids": [],
        "missing_evidence": [],
        "proposed_action": { "type": "send_follow_up", "target_agent": null },
        "expected_entropy_delta": [],
        "packet_intent": "send follow-up while switch cooldown is active",
        "packet_draft": {
            "urgency": "follow_up",
            "summary": "Switch cooldown is active; continue bounded diagnosis.",
            "instructions": ["Do not switch agents right now."],
            "evidence_refs": []
        },
        "ask_user": null,
        "confidence": 0.7
    });
    let (endpoint, request_rx) = serve_advisor_once(decision);
    let env_name = "CAM_TEST_ADVISOR_KEY_SWITCH_COOLDOWN_PRUNED";
    set_test_env_var(env_name, "test-key");
    std::fs::write(
        store.root().join("config.json"),
        json!({
            "policy": {
                "switch_agent_cooldown_min": 30
            },
            "advisor": {
                "enabled": true,
                "provider": {
                    "endpoint": endpoint,
                    "model": "test-advisor",
                    "api_key_env": env_name,
                    "timeout_secs": 5
                }
            }
        })
        .to_string(),
    )
    .expect("config");
    drop(store);

    let advice = advise_workspace(temp.path()).expect("advice");
    let request = request_rx.recv().expect("advisor request");
    let case_file = advisor_request_case_file(&request);
    let allowed = case_file_action_values(&case_file, "allowed_actions");

    assert!(advice.advisor_used);
    assert!(!allowed.contains(&"switch_agent"));
    assert!(allowed.contains(&"spawn_fresh_agent"));
    let forbidden = case_file
        .get("forbidden_actions")
        .and_then(serde_json::Value::as_array)
        .expect("forbidden actions");
    assert!(forbidden.iter().any(|action| {
        action.get("action").and_then(serde_json::Value::as_str) == Some("switch_agent")
            && action
                .get("reason")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|reason| reason.contains("switch_agent cooldown"))
    }));
}

#[test]
fn validator_replaces_advisor_switch_that_targets_failed_agent() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    for index in 1..=3 {
        store
            .append_event(&Event {
                time: Some(format!("2026-06-22T12:2{index}:00Z")),
                event_id: Some(format!("evt-service-claude-{index}")),
                agent: "claude-code".into(),
                kind: EventKind::ModelMessage,
                content: Some("upstream service unavailable while streaming response".into()),
                ..Event::default()
            })
            .expect("event");
    }
    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);

    let outcome = validate_control_action_detailed(
        ControlAction::SwitchAgent {
            target_agent: "claude-code".into(),
        },
        &case_file,
    );

    match outcome {
        ValidationOutcome::Modified { replacement, .. } => {
            assert_eq!(
                replacement,
                ControlAction::SwitchAgent {
                    target_agent: "opencode".into(),
                }
            );
        }
        other => panic!("expected switch target replacement, got {other:?}"),
    }
}

#[test]
fn validator_rejects_switch_rewrite_when_agent_health_entropy_is_not_severe() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    for index in 1..=3 {
        store
            .append_event(&Event {
                time: Some(format!("2026-06-22T12:3{index}:00Z")),
                event_id: Some(format!("evt-retry-level-loop-{index}")),
                agent: "codex".into(),
                kind: EventKind::CommandResult,
                command: Some("python scripts/probe.py".into()),
                exit_code: Some(1),
                ..Event::default()
            })
            .expect("event");
    }
    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);

    let outcome = validate_control_action_detailed(
        ControlAction::SwitchAgent {
            target_agent: "codex".into(),
        },
        &case_file,
    );

    match outcome {
        ValidationOutcome::Modified {
            original,
            replacement,
            reason,
        } => {
            assert_eq!(
                original,
                ControlAction::SwitchAgent {
                    target_agent: "codex".into(),
                }
            );
            assert_eq!(
                replacement,
                ControlAction::RetryAgent {
                    target_agent: Some("codex".into()),
                    max_attempts: 1,
                }
            );
            assert!(reason.contains("agent-health entropy"));
        }
        other => panic!("expected low-severity switch rewrite rejection, got {other:?}"),
    }
}

#[test]
fn validator_rejects_switch_to_different_agent_when_agent_health_entropy_is_not_severe() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    for index in 1..=3 {
        store
            .append_event(&Event {
                time: Some(format!("2026-06-22T12:3{index}:30Z")),
                event_id: Some(format!("evt-different-target-loop-{index}")),
                agent: "codex".into(),
                kind: EventKind::CommandResult,
                command: Some("python scripts/probe.py".into()),
                exit_code: Some(1),
                ..Event::default()
            })
            .expect("event");
    }
    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);

    let outcome = validate_control_action_detailed(
        ControlAction::SwitchAgent {
            target_agent: "claude-code".into(),
        },
        &case_file,
    );

    match outcome {
        ValidationOutcome::Modified {
            original,
            replacement,
            reason,
        } => {
            assert_eq!(
                original,
                ControlAction::SwitchAgent {
                    target_agent: "claude-code".into(),
                }
            );
            assert_eq!(
                replacement,
                ControlAction::RetryAgent {
                    target_agent: Some("codex".into()),
                    max_attempts: 1,
                }
            );
            assert!(reason.contains("agent-health entropy"));
        }
        other => panic!("expected moderate-health switch rejection, got {other:?}"),
    }
}

#[test]
fn repeated_service_failure_entropy_is_cleared_by_later_healthy_message() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    for index in 1..=3 {
        store
            .append_event(&Event {
                time: Some(format!("2026-06-22T12:3{index}:00Z")),
                event_id: Some(format!("evt-service-{index}")),
                agent: "codex".into(),
                kind: EventKind::ModelMessage,
                content: Some("upstream service unavailable while streaming response".into()),
                ..Event::default()
            })
            .expect("event");
    }
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:40:00Z".into()),
            event_id: Some("evt-service-recovered".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some("Recovered provider session and continuing normally.".into()),
            ..Event::default()
        })
        .expect("event");

    let advice = advise_workspace(temp.path()).expect("advice");

    assert_eq!(advice.final_action, ControlAction::ContinueWorking);
}

#[test]
fn repeated_service_failure_entropy_is_cleared_by_later_successful_command_result() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    for index in 1..=3 {
        store
            .append_event(&Event {
                time: Some(format!("2026-06-22T12:3{index}:00Z")),
                event_id: Some(format!("evt-service-{index}")),
                agent: "codex".into(),
                kind: EventKind::ModelMessage,
                content: Some("upstream service unavailable while streaming response".into()),
                ..Event::default()
            })
            .expect("event");
    }
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:40:00Z".into()),
            event_id: Some("evt-command-recovered".into()),
            agent: "codex".into(),
            kind: EventKind::CommandResult,
            command: Some("codex exec".into()),
            exit_code: Some(0),
            ..Event::default()
        })
        .expect("event");

    let advice = advise_workspace(temp.path()).expect("advice");

    assert_eq!(advice.final_action, ControlAction::ContinueWorking);
}

#[test]
fn recovered_service_failure_interventions_do_not_keep_agent_health_degraded() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    for index in 1..=3 {
        store
            .append_event(&Event {
                time: Some(format!("2026-06-22T12:3{index}:00Z")),
                event_id: Some(format!("evt-service-{index}")),
                agent: "codex".into(),
                kind: EventKind::ModelMessage,
                content: Some("upstream service unavailable while streaming response".into()),
                ..Event::default()
            })
            .expect("event");
        store
            .append_intervention(&coding_agent_monitor::Intervention {
                kind: coding_agent_monitor::InterventionKind::ServiceFailure,
                action: coding_agent_monitor::Action::RetrySameAgent,
                agent: Some("codex".into()),
                reason: "transient service failure".into(),
            })
            .expect("intervention");
    }
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:40:00Z".into()),
            event_id: Some("evt-service-recovered".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some("Recovered provider session and continuing normally.".into()),
            ..Event::default()
        })
        .expect("event");

    let advice = advise_workspace(temp.path()).expect("advice");

    assert_eq!(advice.final_action, ControlAction::ContinueWorking);
}

#[test]
fn successful_command_result_clears_recovered_service_failure_interventions() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    for index in 1..=3 {
        store
            .append_event(&Event {
                time: Some(format!("2026-06-22T12:3{index}:00Z")),
                event_id: Some(format!("evt-service-{index}")),
                agent: "codex".into(),
                kind: EventKind::ModelMessage,
                content: Some("upstream service unavailable while streaming response".into()),
                ..Event::default()
            })
            .expect("event");
        store
            .append_intervention(&coding_agent_monitor::Intervention {
                kind: coding_agent_monitor::InterventionKind::ServiceFailure,
                action: coding_agent_monitor::Action::RetrySameAgent,
                agent: Some("codex".into()),
                reason: "transient service failure".into(),
            })
            .expect("intervention");
    }
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:40:00Z".into()),
            event_id: Some("evt-command-recovered".into()),
            agent: "codex".into(),
            kind: EventKind::CommandResult,
            command: Some("codex exec".into()),
            exit_code: Some(0),
            ..Event::default()
        })
        .expect("event");

    let advice = advise_workspace(temp.path()).expect("advice");

    assert_eq!(advice.final_action, ControlAction::ContinueWorking);
}

#[test]
fn service_failure_switch_intervention_does_not_degrade_fallback_target() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    for index in 1..=3 {
        store
            .append_event(&Event {
                time: Some(format!("2026-06-22T12:3{index}:00Z")),
                event_id: Some(format!("evt-service-{index}")),
                agent: "codex".into(),
                kind: EventKind::ModelMessage,
                content: Some("upstream service unavailable while streaming response".into()),
                ..Event::default()
            })
            .expect("event");
    }
    store
        .append_intervention(&coding_agent_monitor::Intervention {
            kind: coding_agent_monitor::InterventionKind::ServiceFailure,
            action: coding_agent_monitor::Action::SwitchAgent,
            agent: Some("claude-code".into()),
            reason: "retry limit exceeded; switch to a fallback agent".into(),
        })
        .expect("intervention");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:40:00Z".into()),
            event_id: Some("evt-service-recovered".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some("Recovered provider session and continuing normally.".into()),
            ..Event::default()
        })
        .expect("event");

    let advice = advise_workspace(temp.path()).expect("advice");

    assert_eq!(advice.final_action, ControlAction::ContinueWorking);
}

#[test]
fn advise_workspace_skips_disabled_fallback_agent() {
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
    for index in 1..=3 {
        store
            .append_event(&Event {
                time: Some(format!("2026-06-22T12:4{index}:00Z")),
                event_id: Some(format!("evt-service-disabled-fallback-{index}")),
                agent: "codex".into(),
                kind: EventKind::ModelMessage,
                content: Some("upstream service unavailable while streaming response".into()),
                ..Event::default()
            })
            .expect("event");
    }

    let advice = advise_workspace(temp.path()).expect("advice");

    assert_eq!(
        advice.final_action,
        ControlAction::SwitchAgent {
            target_agent: "opencode".into(),
        }
    );
    assert!(
        !store
            .root()
            .join("outbox")
            .join("claude-code")
            .join("latest.md")
            .exists()
    );
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
fn session_derived_agent_health_targets_the_degraded_session_agent() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:00:00Z".into()),
            event_id: Some("evt-codex-active".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some("I am still active.".into()),
            ..Event::default()
        })
        .expect("event");
    store
        .append_intervention(&coding_agent_monitor::Intervention {
            kind: coding_agent_monitor::InterventionKind::AgentDegraded,
            action: coding_agent_monitor::Action::SpawnFreshAgent,
            agent: Some("codex".into()),
            reason: "agent appears to have lost design memory".into(),
        })
        .expect("intervention");

    let advice = advise_workspace(temp.path()).expect("advice");

    assert_eq!(
        advice.final_action,
        ControlAction::RetryAgent {
            target_agent: Some("codex".into()),
            max_attempts: 1,
        }
    );
    assert_eq!(advice.packet.target_agent, "codex");
}

#[test]
fn verification_packet_includes_trace_rationale_instruction_when_repo_blame_is_high() {
    let temp = tempfile::tempdir().expect("temp dir");
    init_git_repo(temp.path());
    std::fs::create_dir_all(temp.path().join("src")).expect("src dir");
    std::fs::write(temp.path().join("src/lib.rs"), "fn one() {}\n").expect("source");
    run_git(temp.path(), ["add", "src/lib.rs"]);
    run_git(temp.path(), ["commit", "-m", "add source"]);
    std::fs::write(temp.path().join("src/lib.rs"), "fn one_changed() {}\n")
        .expect("changed source");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:01:00Z".into()),
            event_id: Some("evt-write".into()),
            agent: "codex".into(),
            kind: EventKind::FileChange,
            file: Some("src/lib.rs".into()),
            rationale: Some("Change production code.".into()),
            ..Event::default()
        })
        .expect("event");

    let advice = advise_workspace(temp.path()).expect("advice");

    assert!(matches!(
        advice.final_action,
        ControlAction::ForceVerification { .. }
    ));
    assert!(
        advice
            .packet
            .instructions
            .iter()
            .any(|instruction| instruction.text.contains("trace rationale"))
    );
}

#[test]
fn policy_validator_clamps_retry_attempts_to_safe_range() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    for index in 1..=3 {
        store
            .append_event(&Event {
                time: Some(format!("2026-06-22T12:4{index}:00Z")),
                event_id: Some(format!("evt-retry-clamp-loop-{index}")),
                agent: "codex".into(),
                kind: EventKind::CommandResult,
                command: Some("python scripts/probe.py".into()),
                exit_code: Some(1),
                ..Event::default()
            })
            .expect("event");
    }
    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);

    let outcome = validate_control_action_detailed(
        ControlAction::RetryAgent {
            target_agent: None,
            max_attempts: 0,
        },
        &case_file,
    );

    match outcome {
        ValidationOutcome::Modified { replacement, .. } => {
            assert_eq!(
                replacement,
                ControlAction::RetryAgent {
                    target_agent: Some("codex".into()),
                    max_attempts: 1,
                }
            );
        }
        other => panic!("expected retry attempt clamp, got {other:?}"),
    }
}

#[test]
fn detailed_policy_validation_reports_modified_continue_decision() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:01:00Z".into()),
            event_id: Some("evt-write".into()),
            agent: "codex".into(),
            kind: EventKind::FileChange,
            file: Some("src/lib.rs".into()),
            rationale: Some("Change production code.".into()),
            ..Event::default()
        })
        .expect("event");
    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);

    let outcome = validate_control_action_detailed(ControlAction::ContinueWorking, &case_file);

    match outcome {
        ValidationOutcome::Modified {
            original,
            replacement,
            reason,
        } => {
            assert_eq!(original, ControlAction::ContinueWorking);
            assert_eq!(
                replacement,
                ControlAction::ForceVerification {
                    suite: VerificationSuite::Full,
                    blocking: true,
                }
            );
            assert!(reason.contains("verification"));
        }
        other => panic!("expected modified outcome, got {other:?}"),
    }
}

#[test]
fn policy_validator_replaces_unsafe_continue_with_force_verification() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:01:00Z".into()),
            event_id: Some("evt-write".into()),
            agent: "codex".into(),
            kind: EventKind::FileChange,
            file: Some("src/lib.rs".into()),
            rationale: Some("Change production code.".into()),
            ..Event::default()
        })
        .expect("event");
    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);

    let action = validate_control_action(ControlAction::ContinueWorking, &case_file);

    assert_eq!(
        action,
        ControlAction::ForceVerification {
            suite: VerificationSuite::Full,
            blocking: true,
        }
    );
}

#[test]
fn policy_validator_replaces_follow_up_with_force_verification_when_verification_entropy_is_high() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:01:00Z".into()),
            event_id: Some("evt-write".into()),
            agent: "codex".into(),
            kind: EventKind::FileChange,
            file: Some("src/lib.rs".into()),
            rationale: Some("Change production code.".into()),
            ..Event::default()
        })
        .expect("event");
    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);

    let outcome = validate_control_action_detailed(
        ControlAction::SendFollowUp { target_agent: None },
        &case_file,
    );

    match outcome {
        ValidationOutcome::Modified {
            original,
            replacement,
            reason,
        } => {
            assert_eq!(original, ControlAction::SendFollowUp { target_agent: None });
            assert_eq!(
                replacement,
                ControlAction::ForceVerification {
                    suite: VerificationSuite::Full,
                    blocking: true,
                }
            );
            assert!(reason.contains("verification entropy is high"));
        }
        other => panic!("expected force-verification replacement, got {other:?}"),
    }
}

#[test]
fn adapter_capabilities_are_declared_by_control_surface() {
    let codex = adapter_capabilities_for(AgentKind::Codex);
    let claude = adapter_capabilities_for(AgentKind::ClaudeCode);
    let opencode = adapter_capabilities_for(AgentKind::OpenCode);
    let pi = adapter_capabilities_for(AgentKind::Pi);

    assert!(codex.can_run_headless);
    assert!(codex.ingest_jsonl);
    assert!(codex.hook_pre_tool);
    assert!(codex.can_block_tool);
    assert!(claude.can_block_tool);
    assert!(claude.hook_pre_tool);
    assert!(opencode.ingest_jsonl);
    assert!(opencode.can_export_session);
    assert!(pi.requires_external_sandbox);
    assert!(!pi.can_block_tool);
}

#[test]
fn adapter_capabilities_apply_project_overrides() {
    let mut config = ProjectConfig::default();
    config.adapters.claude_code.enabled = Some(false);
    config.adapters.pi.supports_workspace_write_mode = Some(true);
    config.adapters.pi.requires_external_sandbox = Some(false);
    config.adapters.pi.ingest_jsonl = Some(true);

    let claude = adapter_capabilities_for_config(AgentKind::ClaudeCode, &config.adapters);
    let pi = adapter_capabilities_for_config(AgentKind::Pi, &config.adapters);

    let claude_json = serde_json::to_value(&claude).expect("capabilities json");
    assert_eq!(
        claude_json
            .get("enabled")
            .and_then(serde_json::Value::as_bool),
        Some(false)
    );
    assert!(pi.supports_workspace_write_mode);
    assert!(!pi.requires_external_sandbox);
    assert!(pi.ingest_jsonl);
}

#[test]
fn case_file_carries_effective_adapter_capabilities() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut config = ProjectConfig::default();
    config.adapters.pi.supports_workspace_write_mode = Some(true);
    config.adapters.pi.requires_external_sandbox = Some(false);
    let snapshot =
        DashboardSnapshot::load(ProjectStore::open(temp.path()).expect("store").root(), 20)
            .expect("snapshot");

    let case_file = build_control_case_file_with_config(temp.path(), &snapshot, &config);
    let pi = case_file
        .adapter_capabilities
        .get("pi")
        .expect("pi capabilities");

    assert!(pi.supports_workspace_write_mode);
    assert!(!pi.requires_external_sandbox);
}

#[test]
fn case_file_forbids_writable_handoffs_when_no_safe_adapter_target_exists() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut config = ProjectConfig::default();
    config.adapters.codex.enabled = Some(false);
    config.adapters.claude_code.enabled = Some(false);
    config.adapters.opencode.enabled = Some(false);
    let snapshot =
        DashboardSnapshot::load(ProjectStore::open(temp.path()).expect("store").root(), 20)
            .expect("snapshot");

    let case_file = build_control_case_file_with_config(temp.path(), &snapshot, &config);

    assert!(
        !case_file
            .allowed_actions
            .contains(&ControlActionKind::SwitchAgent)
    );
    assert!(
        !case_file
            .allowed_actions
            .contains(&ControlActionKind::SpawnFreshAgent)
    );
    assert!(case_file.forbidden_actions.iter().any(|forbidden| {
        forbidden.action == ControlActionKind::SwitchAgent
            && forbidden.reason.contains("no adapter capability")
    }));
    assert!(case_file.forbidden_actions.iter().any(|forbidden| {
        forbidden.action == ControlActionKind::SpawnFreshAgent
            && forbidden.reason.contains("no adapter capability")
    }));
}

#[test]
fn case_file_forbids_writable_handoffs_when_recent_file_change_has_active_writer() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            event_id: Some("evt-active-writer-context-loss".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some("I lost design memory and need the project context again.".into()),
            ..Event::default()
        })
        .expect("context event");
    store
        .append_event(&Event {
            event_id: Some("evt-active-writer-change".into()),
            agent: "codex".into(),
            kind: EventKind::FileChange,
            file: Some("docs/notes.md".into()),
            rationale: Some("Continue current implementation.".into()),
            ..Event::default()
        })
        .expect("file change");

    let advice = advise_workspace(temp.path()).expect("advice");
    match advice.final_action {
        ControlAction::Pause { reason } => {
            assert!(reason.contains("active writer codex"));
        }
        other => panic!("expected active-writer pause, got {other:?}"),
    }
    assert!(!store.root().join("locks.jsonl").exists());
    let case_file_log =
        std::fs::read_to_string(store.root().join("case-files.jsonl")).expect("case file log");
    let case_file: ControlCaseFile =
        serde_json::from_str(case_file_log.lines().next().expect("case file"))
            .expect("case file json");
    assert!(
        !case_file
            .allowed_actions
            .contains(&ControlActionKind::SwitchAgent)
    );
    assert!(
        !case_file
            .allowed_actions
            .contains(&ControlActionKind::SpawnFreshAgent)
    );
    assert!(case_file.forbidden_actions.iter().any(|forbidden| {
        forbidden.action == ControlActionKind::SpawnFreshAgent
            && forbidden.reason.contains("active writer codex")
    }));
}

#[test]
fn validator_pauses_writable_handoff_when_recent_file_change_has_other_writer() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            event_id: Some("evt-active-writer-context-loss".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some("I lost design memory and need the project context again.".into()),
            ..Event::default()
        })
        .expect("context event");
    store
        .append_event(&Event {
            event_id: Some("evt-active-writer-change".into()),
            agent: "codex".into(),
            kind: EventKind::FileChange,
            file: Some("src/lib.rs".into()),
            rationale: Some("Continue current implementation.".into()),
            ..Event::default()
        })
        .expect("file change");
    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);

    let outcome = validate_control_action_detailed(
        ControlAction::SpawnFreshAgent {
            target_agent: Some("claude-code".into()),
        },
        &case_file,
    );

    match outcome {
        ValidationOutcome::Modified {
            original,
            replacement,
            reason,
        } => {
            assert_eq!(
                original,
                ControlAction::SpawnFreshAgent {
                    target_agent: Some("claude-code".into()),
                }
            );
            assert!(matches!(replacement, ControlAction::Pause { .. }));
            assert!(reason.contains("active writer codex"));
            assert!(reason.contains("claude-code"));
        }
        other => panic!("expected active-writer handoff pause, got {other:?}"),
    }
}

#[test]
fn validator_replaces_writable_handoff_to_disabled_adapter() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut config = ProjectConfig::default();
    config.adapters.claude_code.enabled = Some(false);
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:00:00Z".into()),
            event_id: Some("evt-disabled-target-context-loss".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some("I lost design memory and need the project context again.".into()),
            ..Event::default()
        })
        .expect("event");
    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file_with_config(temp.path(), &snapshot, &config);

    let outcome = validate_control_action_detailed(
        ControlAction::SpawnFreshAgent {
            target_agent: Some("claude-code".into()),
        },
        &case_file,
    );

    match outcome {
        ValidationOutcome::Modified {
            original,
            replacement,
            reason,
        } => {
            assert_eq!(
                original,
                ControlAction::SpawnFreshAgent {
                    target_agent: Some("claude-code".into()),
                }
            );
            assert_eq!(
                replacement,
                ControlAction::SpawnFreshAgent {
                    target_agent: Some("opencode".into()),
                }
            );
            assert!(reason.contains("disabled"));
            assert!(reason.contains("claude-code"));
        }
        other => panic!("expected disabled adapter target replacement, got {other:?}"),
    }
}

#[test]
fn validator_replaces_writable_handoff_to_adapter_requiring_external_sandbox() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    for index in 1..=3 {
        store
            .append_event(&Event {
                time: Some(format!("2026-06-22T12:0{index}:00Z")),
                event_id: Some(format!("evt-unsafe-target-service-{index}")),
                agent: "codex".into(),
                kind: EventKind::ModelMessage,
                content: Some("upstream service unavailable while streaming response".into()),
                ..Event::default()
            })
            .expect("event");
    }
    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);

    let outcome = validate_control_action_detailed(
        ControlAction::SwitchAgent {
            target_agent: "pi".into(),
        },
        &case_file,
    );

    match outcome {
        ValidationOutcome::Modified {
            original,
            replacement,
            reason,
        } => {
            assert_eq!(
                original,
                ControlAction::SwitchAgent {
                    target_agent: "pi".into(),
                }
            );
            assert_eq!(
                replacement,
                ControlAction::SwitchAgent {
                    target_agent: "claude-code".into(),
                }
            );
            assert!(reason.contains("adapter capabilities"));
            assert!(reason.contains("pi"));
        }
        other => panic!("expected unsafe adapter target replacement, got {other:?}"),
    }
}

#[test]
fn validator_allows_configured_sandboxed_pi_handoff() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut config = ProjectConfig::default();
    config.adapters.pi.supports_workspace_write_mode = Some(true);
    config.adapters.pi.requires_external_sandbox = Some(false);
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:00:00Z".into()),
            event_id: Some("evt-pi-context-loss".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some("I lost design memory and need the project context again.".into()),
            ..Event::default()
        })
        .expect("event");
    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file_with_config(temp.path(), &snapshot, &config);

    let outcome = validate_control_action_detailed(
        ControlAction::SpawnFreshAgent {
            target_agent: Some("pi".into()),
        },
        &case_file,
    );

    assert!(matches!(
        outcome,
        ValidationOutcome::Approved(ControlAction::SpawnFreshAgent { .. })
    ));
}

#[test]
fn control_packet_is_written_to_agent_outbox_and_packet_log() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            event_id: Some("evt-write".into()),
            agent: "codex".into(),
            kind: EventKind::FileChange,
            file: Some("src/lib.rs".into()),
            rationale: Some("Source changed after verifier.".into()),
            ..Event::default()
        })
        .expect("evidence event");
    let packet = ControlPacket {
        packet_id: "packet-1".into(),
        target_agent: "codex".into(),
        urgency: PacketUrgency::Urgent,
        title: "Verification required".into(),
        summary: "Source changed after the last passing verifier.".into(),
        instructions: vec![PacketInstruction {
            priority: PacketInstructionPriority::Must,
            text: "Run cargo test before editing more code.".into(),
        }],
        evidence_refs: vec!["evt-write".into()],
        forbidden: vec!["Do not ask the user whether to continue.".into()],
        success_criteria: vec!["Verifier result is recorded.".into()],
        preconditions: PacketPreconditions::default(),
    };

    let path = store.write_control_packet(&packet).expect("write packet");

    assert!(path.starts_with(store.root().join("outbox").join("codex")));
    assert!(path.ends_with("packet-1.md"));
    let rendered = std::fs::read_to_string(path).expect("packet text");
    assert!(rendered.contains("Verification required"));
    assert!(rendered.contains("Run cargo test"));
    let latest =
        std::fs::read_to_string(store.root().join("outbox").join("codex").join("latest.md"))
            .expect("latest packet");
    assert_eq!(latest, rendered);
    let packet_log = std::fs::read_to_string(store.root().join("packets.jsonl"))
        .expect("packet log should exist");
    assert!(packet_log.contains("\"packet_id\":\"packet-1\""));
}

#[test]
fn control_packet_rendering_is_action_first_and_preconditioned() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            event_id: Some("evt-write".into()),
            agent: "codex".into(),
            kind: EventKind::FileChange,
            file: Some("src/lib.rs".into()),
            rationale: Some("Source changed after verifier.".into()),
            ..Event::default()
        })
        .expect("evidence event");

    let cases = [
        ("codex", "CAM BLOCKING NOTE"),
        ("claude-code", "CLAUDE CODE HOOK PACKET"),
        ("opencode", "OPENCODE PLUGIN PACKET"),
        ("pi", "PI SUPERVISOR PACKET"),
    ];

    for (agent, heading) in cases {
        let packet = ControlPacket {
            packet_id: format!("packet-{agent}"),
            target_agent: agent.into(),
            urgency: PacketUrgency::Urgent,
            title: "Verification required".into(),
            summary: "Source changed after the last passing verifier.".into(),
            instructions: vec![PacketInstruction {
                priority: PacketInstructionPriority::Must,
                text: "Run cargo test before editing more code.".into(),
            }],
            evidence_refs: vec!["evt-write".into()],
            forbidden: vec!["Do not ask the user whether to continue.".into()],
            success_criteria: vec!["Verifier result is recorded.".into()],
            preconditions: PacketPreconditions {
                adapter: Some(agent.into()),
                worktree: Some(temp.path().display().to_string()),
                ..PacketPreconditions::default()
            },
        };

        let path = store.write_control_packet(&packet).expect("write packet");
        let rendered = std::fs::read_to_string(path).expect("packet text");

        assert!(
            rendered.starts_with(&format!("# {heading}")),
            "packet should start with the control signal in {agent} packet:\n{rendered}"
        );
        assert!(
            rendered.contains(heading),
            "missing {heading} in {agent} packet:\n{rendered}"
        );
        assert!(
            rendered.contains(&format!("Target agent: {agent}")),
            "missing target agent in {agent} packet:\n{rendered}"
        );
        assert!(
            rendered.contains("Action: Verification required"),
            "missing action-first title in {agent} packet:\n{rendered}"
        );
        assert!(
            rendered.contains("If any precondition no longer matches"),
            "missing stale-precondition instruction in {agent} packet:\n{rendered}"
        );
        assert!(
            !rendered.contains("Delivery surface:"),
            "packet should not spend prompt budget on delivery plumbing:\n{rendered}"
        );
    }
}

#[test]
fn control_packet_with_secret_like_content_is_rejected_before_outbox_or_log_write() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    let packet = ControlPacket {
        packet_id: "packet-secret".into(),
        target_agent: "codex".into(),
        urgency: PacketUrgency::Urgent,
        title: "Verification required".into(),
        summary: "Retry failed with Authorization: Bearer sk-live-secret-token".into(),
        instructions: vec![PacketInstruction {
            priority: PacketInstructionPriority::Must,
            text: "Do not expose api_key=super-secret-value to another agent.".into(),
        }],
        evidence_refs: vec!["evt-secret".into()],
        forbidden: vec![],
        success_criteria: vec![],
        preconditions: PacketPreconditions::default(),
    };

    let error = store
        .write_control_packet(&packet)
        .expect_err("secret-like packet should be rejected");

    assert!(error.to_string().contains("secret-like packet content"));
    assert!(!store.root().join("outbox").join("codex").exists());
    assert!(!store.root().join("packets.jsonl").exists());
}

#[test]
fn control_packet_with_unknown_evidence_ref_is_rejected_before_outbox_or_log_write() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    let packet = ControlPacket {
        packet_id: "packet-missing-evidence".into(),
        target_agent: "codex".into(),
        urgency: PacketUrgency::Urgent,
        title: "Verification required".into(),
        summary: "Source changed after the last passing verifier.".into(),
        instructions: vec![PacketInstruction {
            priority: PacketInstructionPriority::Must,
            text: "Run cargo test before editing more code.".into(),
        }],
        evidence_refs: vec!["evt-missing".into()],
        forbidden: vec![],
        success_criteria: vec![],
        preconditions: PacketPreconditions::default(),
    };

    let error = store
        .write_control_packet(&packet)
        .expect_err("missing evidence ref should reject packet persistence");

    assert!(error.to_string().contains("unknown packet evidence ref"));
    assert!(!store.root().join("outbox").join("codex").exists());
    assert!(!store.root().join("packets.jsonl").exists());
}

#[test]
fn append_advice_rejects_packet_evidence_ref_missing_from_referenced_case_file() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);
    store.append_case_file(&case_file).expect("case file");
    let packet = ControlPacket {
        packet_id: "packet-advice-missing-evidence".into(),
        target_agent: "codex".into(),
        urgency: PacketUrgency::FollowUp,
        title: "Continue working".into(),
        summary: "No blocking uncertainty.".into(),
        instructions: vec![PacketInstruction {
            priority: PacketInstructionPriority::Should,
            text: "Continue the current task.".into(),
        }],
        evidence_refs: vec!["evt-missing".into()],
        forbidden: vec![],
        success_criteria: vec!["Work continues.".into()],
        preconditions: PacketPreconditions::default(),
    };
    let advice = coding_agent_monitor::AdviceRun {
        advice_id: "advice-missing-evidence".into(),
        case_file_id: case_file.case_file_id,
        advisor_used: false,
        advisor_error: None,
        advisor_decision: None,
        validation_outcome: ValidationOutcome::Approved(ControlAction::ContinueWorking),
        final_action: ControlAction::ContinueWorking,
        control_rationale: Default::default(),
        packet,
        dispatch_result: coding_agent_monitor::DispatchResult::default(),
        packet_path: None,
    };

    let error = store
        .append_advice(&advice)
        .expect_err("advice packet evidence refs must resolve against the referenced case file");

    assert!(error.to_string().contains("unknown packet evidence ref"));
    assert!(!store.root().join("advice.jsonl").exists());
}

#[test]
fn append_advice_rejects_missing_referenced_case_file_even_without_packet_evidence_refs() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    let packet = ControlPacket {
        packet_id: "packet-missing-case-file".into(),
        target_agent: "codex".into(),
        urgency: PacketUrgency::FollowUp,
        title: "Continue working".into(),
        summary: "No blocking uncertainty.".into(),
        instructions: vec![PacketInstruction {
            priority: PacketInstructionPriority::Should,
            text: "Continue the current task.".into(),
        }],
        evidence_refs: vec![],
        forbidden: vec![],
        success_criteria: vec!["Work continues.".into()],
        preconditions: PacketPreconditions::default(),
    };
    let advice = coding_agent_monitor::AdviceRun {
        advice_id: "advice-missing-case-file".into(),
        case_file_id: "case-missing".into(),
        advisor_used: false,
        advisor_error: None,
        advisor_decision: None,
        validation_outcome: ValidationOutcome::Approved(ControlAction::ContinueWorking),
        final_action: ControlAction::ContinueWorking,
        control_rationale: Default::default(),
        packet,
        dispatch_result: coding_agent_monitor::DispatchResult::default(),
        packet_path: None,
    };

    let error = store
        .append_advice(&advice)
        .expect_err("advice must reference a persisted case file");

    assert!(error.to_string().contains("missing case file"));
    assert!(!store.root().join("advice.jsonl").exists());
}

#[test]
fn dispatch_control_packet_rejects_stale_git_head_precondition() {
    let temp = tempfile::tempdir().expect("temp dir");
    let _head = init_git_repo(temp.path());
    let mut store = ProjectStore::open(temp.path()).expect("store");
    let packet = ControlPacket {
        packet_id: "packet-stale-head".into(),
        target_agent: "codex".into(),
        urgency: PacketUrgency::Urgent,
        title: "Verification required".into(),
        summary: "Source changed after verifier.".into(),
        instructions: vec![PacketInstruction {
            priority: PacketInstructionPriority::Must,
            text: "Run cargo test.".into(),
        }],
        evidence_refs: vec!["evt-write".into()],
        forbidden: vec![],
        success_criteria: vec!["Verifier result is recorded.".into()],
        preconditions: PacketPreconditions {
            git_head: Some("0000000000000000000000000000000000000000".into()),
            worktree: Some(temp.path().display().to_string()),
            ..PacketPreconditions::default()
        },
    };

    let error = store
        .dispatch_control_packet(&packet)
        .expect_err("stale precondition should block dispatch");

    assert!(error.to_string().contains("packet precondition failed"));
    assert!(!store.root().join("outbox").join("codex").exists());
    assert!(!store.root().join("dispatch.jsonl").exists());
}

#[test]
fn dispatch_control_packet_records_outbox_delivery_result() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            event_id: Some("evt-write".into()),
            agent: "codex".into(),
            kind: EventKind::FileChange,
            file: Some("src/lib.rs".into()),
            rationale: Some("Source changed after verifier.".into()),
            ..Event::default()
        })
        .expect("evidence event");
    let packet = ControlPacket {
        packet_id: "packet-dispatch".into(),
        target_agent: "codex".into(),
        urgency: PacketUrgency::Urgent,
        title: "Verification required".into(),
        summary: "Source changed after verifier.".into(),
        instructions: vec![PacketInstruction {
            priority: PacketInstructionPriority::Must,
            text: "Run cargo test.".into(),
        }],
        evidence_refs: vec!["evt-write".into()],
        forbidden: vec![],
        success_criteria: vec!["Verifier result is recorded.".into()],
        preconditions: PacketPreconditions::default(),
    };

    let dispatch = store
        .dispatch_control_packet(&packet)
        .expect("dispatch packet");

    assert_eq!(dispatch.packet_id, "packet-dispatch");
    assert_eq!(dispatch.target_agent, "codex");
    assert_eq!(dispatch.status, DispatchStatus::OutboxWritten);
    let path = dispatch.path.as_ref().expect("outbox path");
    assert!(std::path::Path::new(path).exists());
    let dispatch_log =
        std::fs::read_to_string(store.root().join("dispatch.jsonl")).expect("dispatch log");
    assert!(dispatch_log.contains("\"packet_id\":\"packet-dispatch\""));
    assert!(dispatch_log.contains("\"status\":\"outbox_written\""));
}

#[test]
fn action_outcome_persistence_records_expected_and_observed_entropy_delta() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    let outcome = coding_agent_monitor::ActionOutcome {
        outcome_id: "outcome-1".into(),
        advice_id: "advice-1".into(),
        action: ControlActionKind::ForceVerification,
        status: OutcomeStatus::Succeeded,
        expected_entropy_delta: vec![EntropyDelta {
            kind: EntropyKind::Verification,
            delta: -55,
        }],
        observed_entropy_delta: vec![EntropyDelta {
            kind: EntropyKind::Verification,
            delta: -40,
        }],
        observed_entropy_delta_evidence: vec![coding_agent_monitor::EntropyDeltaEvidence {
            kind: EntropyKind::Verification,
            evidence_ids: vec!["evt-test-pass".into()],
            cause_evidence_ids: Vec::new(),
            result_evidence_ids: vec!["evt-test-pass".into()],
        }],
        evidence_ids: vec!["evt-test-pass".into()],
        requirement_ids: Vec::new(),
        note: Some("verification passed after packet".into()),
    };

    store
        .append_action_outcome(&outcome)
        .expect("append outcome");

    let outcome_log =
        std::fs::read_to_string(store.root().join("outcomes.jsonl")).expect("outcome log");
    assert!(outcome_log.contains("\"outcome_id\":\"outcome-1\""));
    assert!(outcome_log.contains("\"delta\":-40"));
    assert!(outcome_log.contains("\"observed_entropy_delta_evidence\""));
    assert!(outcome_log.contains("\"evt-test-pass\""));
}

#[test]
fn worktree_lock_prevents_second_writable_owner() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    let request = WorktreeLockRequest {
        worktree: temp.path().join("repo").display().to_string(),
        owner_agent: "codex".into(),
        session: Some("s1".into()),
    };

    let first = store
        .try_acquire_worktree_lock(&request)
        .expect("first lock");
    let second = store
        .try_acquire_worktree_lock(&WorktreeLockRequest {
            owner_agent: "claude-code".into(),
            session: Some("s2".into()),
            ..request.clone()
        })
        .expect("second lock attempt");

    let WorktreeLockResult::Acquired(lock) = first else {
        panic!("expected first lock to acquire");
    };
    let WorktreeLockResult::Conflict { existing } = second else {
        panic!("expected second lock to conflict");
    };
    assert_eq!(existing.lock_id, lock.lock_id);
    assert_eq!(existing.owner_agent, "codex");
    assert!(store.root().join("locks.jsonl").exists());
}

#[test]
fn worktree_lock_normalizes_dot_segment_equivalent_paths() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    let repo = temp.path().join("repo");
    std::fs::create_dir_all(&repo).expect("repo dir");
    let first = store
        .try_acquire_worktree_lock(&WorktreeLockRequest {
            worktree: repo.display().to_string(),
            owner_agent: "codex".into(),
            session: Some("s1".into()),
        })
        .expect("first lock");
    let second = store
        .try_acquire_worktree_lock(&WorktreeLockRequest {
            worktree: repo.join("..").join("repo").display().to_string(),
            owner_agent: "claude-code".into(),
            session: Some("s2".into()),
        })
        .expect("second lock attempt");

    assert!(matches!(first, WorktreeLockResult::Acquired(_)));
    let WorktreeLockResult::Conflict { existing } = second else {
        panic!("dot-segment equivalent path should conflict");
    };
    assert_eq!(existing.owner_agent, "codex");
}

#[test]
fn stale_worktree_lock_release_removes_old_lock_and_logs_expiry() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    let worktree = temp.path().join("repo").display().to_string();
    let first = store
        .try_acquire_worktree_lock(&WorktreeLockRequest {
            worktree: worktree.clone(),
            owner_agent: "codex".into(),
            session: Some("s1".into()),
        })
        .expect("first lock");
    let WorktreeLockResult::Acquired(first_lock) = first else {
        panic!("expected first lock to acquire");
    };
    rewrite_only_worktree_lock_timestamp(store.root(), "1970-01-01T00:00:00Z");

    let released = store
        .release_stale_worktree_locks(1)
        .expect("release stale locks");

    assert_eq!(released.len(), 1);
    assert_eq!(released[0].lock_id, first_lock.lock_id);
    let reacquired = store
        .try_acquire_worktree_lock(&WorktreeLockRequest {
            worktree,
            owner_agent: "claude-code".into(),
            session: Some("s2".into()),
        })
        .expect("reacquire");
    assert!(matches!(reacquired, WorktreeLockResult::Acquired(_)));
    let lock_log =
        std::fs::read_to_string(store.root().join("locks.jsonl")).expect("lock log should exist");
    assert!(lock_log.contains("\"kind\":\"expired\""));
}

#[test]
fn stale_worktree_lock_release_keeps_lock_with_unparseable_timestamp() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    let worktree = temp.path().join("repo").display().to_string();
    let first = store
        .try_acquire_worktree_lock(&WorktreeLockRequest {
            worktree: worktree.clone(),
            owner_agent: "codex".into(),
            session: Some("s1".into()),
        })
        .expect("first lock");
    assert!(matches!(first, WorktreeLockResult::Acquired(_)));
    rewrite_only_worktree_lock_timestamp(store.root(), "not-a-timestamp");

    let released = store
        .release_stale_worktree_locks(1)
        .expect("release stale locks");
    let second = store
        .try_acquire_worktree_lock(&WorktreeLockRequest {
            worktree,
            owner_agent: "claude-code".into(),
            session: Some("s2".into()),
        })
        .expect("second lock attempt");

    assert!(released.is_empty());
    let WorktreeLockResult::Conflict { existing } = second else {
        panic!("unparseable lock timestamp should fail closed");
    };
    assert_eq!(existing.owner_agent, "codex");
}

#[test]
fn worktree_lock_release_allows_new_owner() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    let request = WorktreeLockRequest {
        worktree: temp.path().join("repo").display().to_string(),
        owner_agent: "codex".into(),
        session: Some("s1".into()),
    };
    let WorktreeLockResult::Acquired(lock) = store
        .try_acquire_worktree_lock(&request)
        .expect("first lock")
    else {
        panic!("expected first lock to acquire");
    };

    assert!(
        store
            .release_worktree_lock(&lock.worktree, &lock.lock_id)
            .expect("release lock")
    );
    let reacquired = store
        .try_acquire_worktree_lock(&WorktreeLockRequest {
            owner_agent: "claude-code".into(),
            session: Some("s2".into()),
            ..request
        })
        .expect("reacquire");

    let WorktreeLockResult::Acquired(new_lock) = reacquired else {
        panic!("expected lock reacquire");
    };
    assert_eq!(new_lock.owner_agent, "claude-code");
}

#[test]
fn advise_workspace_acquires_worktree_lock_for_fresh_agent_handoff() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:00:00Z".into()),
            event_id: Some("evt-lost-memory".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some("I lost design memory and need the project context again.".into()),
            ..Event::default()
        })
        .expect("event");

    let advice = advise_workspace(temp.path()).expect("advice");

    assert_eq!(
        advice.final_action,
        ControlAction::SpawnFreshAgent {
            target_agent: Some("claude-code".into()),
        }
    );
    let lock_log = std::fs::read_to_string(store.root().join("locks.jsonl"))
        .expect("handoff lock should be logged");
    assert!(lock_log.contains("\"kind\":\"acquired\""));
    assert!(lock_log.contains("\"owner_agent\":\"claude-code\""));
}

#[test]
fn advise_workspace_pauses_spawn_fresh_when_cooldown_is_active() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    std::fs::write(
        store.root().join("config.json"),
        r#"{
          "policy": {
            "spawn_fresh_cooldown_min": 10
          }
        }"#,
    )
    .expect("config");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:00:00Z".into()),
            event_id: Some("evt-lost-memory-cooldown".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some("I lost design memory and need the project context again.".into()),
            ..Event::default()
        })
        .expect("event");
    drop(store);

    let first = advise_workspace(temp.path()).expect("first advice");
    assert!(matches!(
        first.final_action,
        ControlAction::SpawnFreshAgent { .. }
    ));
    let second = advise_workspace(temp.path()).expect("second advice");

    match second.final_action {
        ControlAction::Pause { reason } => {
            assert!(reason.contains("spawn_fresh_agent cooldown"));
        }
        other => panic!("expected spawn cooldown pause, got {other:?}"),
    }
}

#[test]
fn advisor_request_forbids_spawn_fresh_when_cooldown_is_active() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    append_action_advice_at(
        &mut store,
        temp.path(),
        "9999-01-01T00:20:00Z",
        ControlAction::SpawnFreshAgent {
            target_agent: Some("claude-code".into()),
        },
        "Fresh agent handoff",
    );
    store
        .append_event(&Event {
            event_id: Some("evt-lost-memory-advisor-cooldown".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some("I forgot the design memory and project constraints.".into()),
            ..Event::default()
        })
        .expect("event");
    let decision = json!({
        "diagnosis_id": "diagnosis-spawn-cooldown-pruned",
        "dominant_entropy": "context",
        "entropy_scores": {
            "context": { "score": 80, "confidence": 80 }
        },
        "top_evidence": [],
        "cited_evidence_ids": [],
        "missing_evidence": [],
        "proposed_action": { "type": "send_follow_up", "target_agent": null },
        "expected_entropy_delta": [],
        "packet_intent": "send follow-up while spawn cooldown is active",
        "packet_draft": {
            "urgency": "follow_up",
            "summary": "Spawn cooldown is active; send bounded context instead.",
            "instructions": ["Do not spawn a fresh agent right now."],
            "evidence_refs": []
        },
        "ask_user": null,
        "confidence": 0.7
    });
    let (endpoint, request_rx) = serve_advisor_once(decision);
    let env_name = "CAM_TEST_ADVISOR_KEY_SPAWN_COOLDOWN_PRUNED";
    set_test_env_var(env_name, "test-key");
    std::fs::write(
        store.root().join("config.json"),
        json!({
            "policy": {
                "spawn_fresh_cooldown_min": 10
            },
            "advisor": {
                "enabled": true,
                "provider": {
                    "endpoint": endpoint,
                    "model": "test-advisor",
                    "api_key_env": env_name,
                    "timeout_secs": 5
                }
            }
        })
        .to_string(),
    )
    .expect("config");
    drop(store);

    let advice = advise_workspace(temp.path()).expect("advice");
    let request = request_rx.recv().expect("advisor request");
    let case_file = advisor_request_case_file(&request);
    let allowed = case_file_action_values(&case_file, "allowed_actions");

    assert!(advice.advisor_used);
    assert!(!allowed.contains(&"spawn_fresh_agent"));
    assert!(!allowed.contains(&"switch_agent"));
    let forbidden = case_file
        .get("forbidden_actions")
        .and_then(serde_json::Value::as_array)
        .expect("forbidden actions");
    assert!(forbidden.iter().any(|action| {
        action.get("action").and_then(serde_json::Value::as_str) == Some("spawn_fresh_agent")
            && action
                .get("reason")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|reason| reason.contains("spawn_fresh_agent cooldown"))
    }));
}

#[test]
fn advisor_request_forbids_writable_handoffs_when_parallel_limit_is_zero() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            event_id: Some("evt-lost-memory-capacity".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some("I forgot the design memory and project constraints.".into()),
            ..Event::default()
        })
        .expect("event");
    let decision = json!({
        "diagnosis_id": "diagnosis-capacity-pruned",
        "dominant_entropy": "context",
        "entropy_scores": {
            "context": { "score": 80, "confidence": 80 }
        },
        "top_evidence": [],
        "cited_evidence_ids": [],
        "missing_evidence": [],
        "proposed_action": { "type": "send_follow_up", "target_agent": null },
        "expected_entropy_delta": [],
        "packet_intent": "send follow-up while writable capacity is unavailable",
        "packet_draft": {
            "urgency": "follow_up",
            "summary": "Writable handoff capacity is unavailable.",
            "instructions": ["Do not spawn or switch agents right now."],
            "evidence_refs": []
        },
        "ask_user": null,
        "confidence": 0.7
    });
    let (endpoint, request_rx) = serve_advisor_once(decision);
    let env_name = "CAM_TEST_ADVISOR_KEY_WRITABLE_CAPACITY_PRUNED";
    set_test_env_var(env_name, "test-key");
    std::fs::write(
        store.root().join("config.json"),
        json!({
            "policy": {
                "max_parallel_writable_agents": 0
            },
            "advisor": {
                "enabled": true,
                "provider": {
                    "endpoint": endpoint,
                    "model": "test-advisor",
                    "api_key_env": env_name,
                    "timeout_secs": 5
                }
            }
        })
        .to_string(),
    )
    .expect("config");

    let advice = advise_workspace(temp.path()).expect("advice");
    let request = request_rx.recv().expect("advisor request");
    let case_file = advisor_request_case_file(&request);
    let allowed = case_file_action_values(&case_file, "allowed_actions");

    assert!(advice.advisor_used);
    assert!(!allowed.contains(&"spawn_fresh_agent"));
    assert!(!allowed.contains(&"switch_agent"));
    let forbidden = case_file
        .get("forbidden_actions")
        .and_then(serde_json::Value::as_array)
        .expect("forbidden actions");
    for action_name in ["spawn_fresh_agent", "switch_agent"] {
        assert!(forbidden.iter().any(|action| {
            action.get("action").and_then(serde_json::Value::as_str) == Some(action_name)
                && action
                    .get("reason")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|reason| reason.contains("max_parallel_writable_agents"))
        }));
    }
}

#[test]
fn advise_workspace_pauses_writable_handoff_when_parallel_limit_is_zero() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    std::fs::write(
        store.root().join("config.json"),
        r#"{
          "policy": {
            "max_parallel_writable_agents": 0
          }
        }"#,
    )
    .expect("config");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:00:00Z".into()),
            event_id: Some("evt-lost-memory-no-capacity".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some("I lost design memory and need the project context again.".into()),
            ..Event::default()
        })
        .expect("event");

    let advice = advise_workspace(temp.path()).expect("advice");

    match advice.final_action {
        ControlAction::Pause { reason } => {
            assert!(reason.contains("max_parallel_writable_agents"));
        }
        other => panic!("expected writable capacity pause, got {other:?}"),
    }
    assert!(!store.root().join("locks.jsonl").exists());
    assert!(!store.root().join("locks").join("worktrees").exists());
}

#[test]
fn advise_workspace_pauses_switch_when_worktree_is_already_locked() {
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
    for index in 1..=3 {
        store
            .append_event(&Event {
                time: Some(format!("2026-06-22T12:1{index}:00Z")),
                event_id: Some(format!("evt-service-{index}")),
                agent: "codex".into(),
                kind: EventKind::ModelMessage,
                content: Some("upstream service unavailable while streaming response".into()),
                ..Event::default()
            })
            .expect("event");
    }

    let advice = advise_workspace(temp.path()).expect("advice");

    match advice.final_action {
        ControlAction::Pause { reason } => {
            assert!(reason.contains("worktree"));
            assert!(reason.contains("locked"));
        }
        other => panic!("expected pause after lock conflict, got {other:?}"),
    }
    assert_eq!(advice.packet.target_agent, "codex");
    assert!(
        !store
            .root()
            .join("outbox")
            .join("claude-code")
            .join("latest.md")
            .exists()
    );
    let lock_log = std::fs::read_to_string(store.root().join("locks.jsonl"))
        .expect("lock conflict should be logged");
    assert!(lock_log.contains("\"kind\":\"conflict\""));
    assert!(lock_log.contains("\"requested_owner\":\"claude-code\""));
}

#[test]
fn handoff_workspace_reclaims_configured_stale_worktree_lock_before_dispatch() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    std::fs::write(
        store.root().join("config.json"),
        r#"{
          "policy": {
            "worktree_lock_stale_after_secs": 1
          }
        }"#,
    )
    .expect("config");
    let existing = store
        .try_acquire_worktree_lock(&WorktreeLockRequest {
            worktree: temp.path().display().to_string(),
            owner_agent: "codex".into(),
            session: Some("s1".into()),
        })
        .expect("existing lock");
    assert!(matches!(existing, WorktreeLockResult::Acquired(_)));
    rewrite_only_worktree_lock_timestamp(store.root(), "1970-01-01T00:00:00Z");

    let handoff = coding_agent_monitor::handoff_workspace(temp.path(), AgentKind::ClaudeCode)
        .expect("handoff should reclaim stale lock");

    assert_eq!(handoff.packet.target_agent, "claude-code");
    let lock_log =
        std::fs::read_to_string(store.root().join("locks.jsonl")).expect("lock log should exist");
    assert!(lock_log.contains("\"kind\":\"expired\""));
    assert!(lock_log.contains("\"owner_agent\":\"claude-code\""));
}

#[test]
fn advise_workspace_records_validation_outcome_and_targeted_verifier_packet() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    std::fs::write(
        store.root().join("config.json"),
        r#"{
          "verifiers": [
            {
              "id": "parser_targeted",
              "command": "cargo test parser::tests::handles_nested",
              "scope": "targeted",
              "timeout_secs": 120,
              "paths": ["src/parser.rs"]
            }
          ]
        }"#,
    )
    .expect("config");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:01:00Z".into()),
            event_id: Some("evt-parser-write".into()),
            agent: "codex".into(),
            kind: EventKind::FileChange,
            file: Some("src/parser.rs".into()),
            rationale: Some("Implement nested parser behavior.".into()),
            ..Event::default()
        })
        .expect("event");

    let advice = advise_workspace(temp.path()).expect("advice");

    assert_eq!(
        advice.final_action,
        ControlAction::ForceVerification {
            suite: VerificationSuite::Targeted,
            blocking: true,
        }
    );
    assert!(matches!(
        advice.validation_outcome,
        ValidationOutcome::Approved(ControlAction::ForceVerification {
            suite: VerificationSuite::Targeted,
            blocking: true
        })
    ));
    assert!(advice.packet.instructions.iter().any(|instruction| {
        instruction
            .text
            .contains("cargo test parser::tests::handles_nested")
    }));
    assert_eq!(
        advice.control_rationale.selected_action,
        ControlActionKind::ForceVerification
    );
    assert_eq!(
        advice.control_rationale.dominant_entropy,
        Some(EntropyKind::Verification)
    );
    assert!(
        advice
            .control_rationale
            .reason
            .contains("verification entropy")
    );
    assert!(
        advice
            .control_rationale
            .evidence_ids
            .contains(&"evt-parser-write".into())
    );
    assert!(
        advice
            .control_rationale
            .expected_entropy_delta
            .iter()
            .any(|delta| { delta.kind == EntropyKind::Verification && delta.delta < 0 })
    );
    assert_eq!(advice.dispatch_result.status, DispatchStatus::OutboxWritten);
    assert!(
        advice
            .dispatch_result
            .path
            .as_deref()
            .is_some_and(|path| path.contains("packet-"))
    );
    let advice_log =
        std::fs::read_to_string(store.root().join("advice.jsonl")).expect("advice log");
    assert!(advice_log.contains("\"validation_outcome\""));
    assert!(advice_log.contains("\"dispatch_result\""));
    assert!(advice_log.contains("\"control_rationale\""));
}

#[test]
fn advise_workspace_persists_failed_dispatch_decision_trail() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    let outbox = store.root().join("outbox").join("codex");
    std::fs::create_dir_all(outbox.join("latest.md")).expect("blocking latest directory");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:01:00Z".into()),
            event_id: Some("evt-dispatch-failure-write".into()),
            agent: "codex".into(),
            kind: EventKind::FileChange,
            file: Some("src/lib.rs".into()),
            rationale: Some("Trigger verification packet dispatch.".into()),
            ..Event::default()
        })
        .expect("event");

    let advice = advise_workspace(temp.path()).expect("dispatch failure should be replayable");

    assert_eq!(
        advice.final_action.kind(),
        ControlActionKind::ForceVerification
    );
    assert_eq!(advice.dispatch_result.status, DispatchStatus::Failed);
    assert!(
        advice
            .dispatch_result
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("latest.md")),
        "missing dispatch failure reason: {:?}",
        advice.dispatch_result
    );
    assert!(store.root().join("case-files.jsonl").exists());
    assert!(store.root().join("advice.jsonl").exists());
    assert!(store.root().join("dispatch.jsonl").exists());
    assert!(store.root().join("outcomes.jsonl").exists());
    let outcome_log =
        std::fs::read_to_string(store.root().join("outcomes.jsonl")).expect("outcome log");
    let outcome: ActionOutcome =
        serde_json::from_str(outcome_log.lines().next().expect("one outcome"))
            .expect("outcome json");
    assert_eq!(outcome.advice_id, advice.advice_id);
    assert_eq!(outcome.action, ControlActionKind::ForceVerification);
    assert_eq!(outcome.status, OutcomeStatus::Failed);
    let trails = load_decision_trails(store.root()).expect("decision trails");
    assert_eq!(trails.len(), 1);
    assert_eq!(trails[0].dispatch.status, DispatchStatus::Failed);
    assert_eq!(trails[0].advice.advice_id, advice.advice_id);
}

#[test]
fn advise_workspace_suppresses_duplicate_urgent_packet_without_new_evidence() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:01:00Z".into()),
            event_id: Some("evt-parser-write".into()),
            agent: "codex".into(),
            kind: EventKind::FileChange,
            file: Some("src/parser.rs".into()),
            rationale: Some("Implement nested parser behavior.".into()),
            ..Event::default()
        })
        .expect("event");

    let first = advise_workspace(temp.path()).expect("first advice");
    let second = advise_workspace(temp.path()).expect("second advice");

    assert!(matches!(
        first.final_action,
        ControlAction::ForceVerification { blocking: true, .. }
    ));
    assert!(matches!(
        second.final_action,
        ControlAction::ForceVerification { blocking: true, .. }
    ));
    assert!(
        second
            .dispatch_result
            .reason
            .as_deref()
            .is_some_and(|reason| reason.contains("duplicate urgent packet")),
        "missing duplicate suppression reason: {:?}",
        second.dispatch_result
    );

    let packets = std::fs::read_to_string(store.root().join("packets.jsonl")).expect("packets");
    assert_eq!(packets.lines().count(), 1);

    let dispatches =
        std::fs::read_to_string(store.root().join("dispatch.jsonl")).expect("dispatches");
    assert_eq!(dispatches.lines().count(), 2);
    assert!(dispatches.contains("\"status\":\"suppressed_duplicate\""));
}

#[test]
fn advise_workspace_stamps_packet_with_workspace_and_git_head_preconditions() {
    let temp = tempfile::tempdir().expect("temp dir");
    let head = init_git_repo(temp.path());
    let advice = advise_workspace(temp.path()).expect("advice");
    let expected_worktree = temp.path().display().to_string();

    assert_eq!(
        advice.packet.preconditions.worktree.as_deref(),
        Some(expected_worktree.as_str())
    );
    assert_eq!(
        advice.packet.preconditions.git_head.as_deref(),
        Some(head.as_str())
    );
}

#[test]
fn decision_trail_replay_links_case_advice_dispatch_packet_and_outcomes() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:01:00Z".into()),
            event_id: Some("evt-write".into()),
            agent: "codex".into(),
            kind: EventKind::FileChange,
            file: Some("src/lib.rs".into()),
            rationale: Some("Change production code.".into()),
            ..Event::default()
        })
        .expect("event");
    let advice = advise_workspace(temp.path()).expect("advice");
    store
        .append_action_outcome(&coding_agent_monitor::ActionOutcome {
            outcome_id: "outcome-for-advice".into(),
            advice_id: advice.advice_id.clone(),
            action: advice.final_action.kind(),
            status: OutcomeStatus::Succeeded,
            expected_entropy_delta: vec![EntropyDelta {
                kind: EntropyKind::Verification,
                delta: -55,
            }],
            observed_entropy_delta: vec![EntropyDelta {
                kind: EntropyKind::Verification,
                delta: -50,
            }],
            observed_entropy_delta_evidence: Vec::new(),
            evidence_ids: vec!["evt-test-pass".into()],
            requirement_ids: Vec::new(),
            note: None,
        })
        .expect("outcome");

    let trails = load_decision_trails(store.root()).expect("trails");

    assert_eq!(trails.len(), 1);
    let trail = &trails[0];
    assert_eq!(trail.advice.advice_id, advice.advice_id);
    assert_eq!(trail.case_file.case_file_id, advice.case_file_id);
    assert_eq!(trail.packet.packet_id, advice.packet.packet_id);
    assert_eq!(
        trail.dispatch.dispatch_id,
        advice.dispatch_result.dispatch_id
    );
    assert_eq!(trail.outcomes.len(), 1);
    assert_eq!(trail.outcomes[0].outcome_id, "outcome-for-advice");
}

#[test]
fn case_file_replay_boundary_counts_decisions_packets_outcomes_and_locks() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    let advice = append_dispatched_action_advice_at(
        &mut store,
        temp.path(),
        "2026-06-22T12:30:00Z",
        ControlAction::ForceVerification {
            suite: VerificationSuite::Targeted,
            blocking: true,
        },
        "Verification required",
    );
    store
        .append_action_outcome(&ActionOutcome {
            outcome_id: "outcome-replay-boundary".into(),
            advice_id: advice.advice_id.clone(),
            action: advice.final_action.kind(),
            status: OutcomeStatus::Succeeded,
            expected_entropy_delta: vec![EntropyDelta {
                kind: EntropyKind::Verification,
                delta: -55,
            }],
            observed_entropy_delta: vec![EntropyDelta {
                kind: EntropyKind::Verification,
                delta: -50,
            }],
            observed_entropy_delta_evidence: Vec::new(),
            evidence_ids: Vec::new(),
            requirement_ids: Vec::new(),
            note: None,
        })
        .expect("outcome");
    store
        .try_acquire_worktree_lock(&WorktreeLockRequest {
            worktree: temp.path().display().to_string(),
            owner_agent: "claude-code".into(),
            session: Some("replay-boundary".into()),
        })
        .expect("worktree lock");

    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);

    assert_eq!(snapshot.advice_count, 1);
    assert_eq!(snapshot.packet_count, 1);
    assert_eq!(snapshot.dispatch_count, 1);
    assert_eq!(snapshot.outcome_count, 1);
    assert_eq!(snapshot.lock_event_count, 1);
    assert_eq!(case_file.replay.input.advice_count, 1);
    assert_eq!(case_file.replay.input.packet_count, 1);
    assert_eq!(case_file.replay.input.dispatch_count, 1);
    assert_eq!(case_file.replay.input.outcome_count, 1);
    assert_eq!(case_file.replay.input.lock_event_count, 1);
}

#[test]
fn run_verifier_records_successful_outcome_for_latest_force_verification_advice() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    std::fs::write(
        store.root().join("config.json"),
        json!({
            "verifiers": [
                {
                    "id": "smoke",
                    "command": passing_verifier_command(),
                    "scope": "targeted",
                    "timeout_secs": 5,
                    "paths": ["src/lib.rs"]
                }
            ]
        })
        .to_string(),
    )
    .expect("config");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:01:00Z".into()),
            event_id: Some("evt-write".into()),
            agent: "codex".into(),
            kind: EventKind::FileChange,
            file: Some("src/lib.rs".into()),
            rationale: Some("Change production code.".into()),
            ..Event::default()
        })
        .expect("event");
    let advice = advise_workspace(temp.path()).expect("advice");
    assert_eq!(
        advice.final_action,
        ControlAction::ForceVerification {
            suite: VerificationSuite::Targeted,
            blocking: true,
        }
    );

    let run = run_verifier(temp.path(), "smoke").expect("verifier");
    let trails = load_decision_trails(store.root()).expect("trails");

    assert_eq!(run.status, VerificationRunStatus::Passed);
    let trail = trails
        .iter()
        .find(|trail| trail.advice.advice_id == advice.advice_id)
        .expect("advice trail");
    assert_eq!(trail.outcomes.len(), 1);
    let outcome = &trail.outcomes[0];
    assert_eq!(outcome.advice_id, advice.advice_id);
    assert_eq!(outcome.action, ControlActionKind::ForceVerification);
    assert_eq!(outcome.status, OutcomeStatus::Succeeded);
    assert!(outcome.evidence_ids.contains(&run.verifier_run_id));
    assert!(
        outcome
            .observed_entropy_delta
            .iter()
            .any(|delta| delta.kind == EntropyKind::Verification && delta.delta < 0)
    );
}

#[test]
fn force_verification_links_project_contract_requirement_to_control_and_outcome_proof() {
    let temp = tempfile::tempdir().expect("temp dir");
    std::fs::write(
        temp.path().join("AGENTS.md"),
        "# Project\n\n## Non-Negotiable Invariants\n\n- Do not continue after source/test changes when relevant verification is stale, unless the change is docs-only and policy allows it.\n",
    )
    .expect("AGENTS.md");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    std::fs::write(
        store.root().join("config.json"),
        json!({
            "verifiers": [
                {
                    "id": "smoke",
                    "command": passing_verifier_command(),
                    "scope": "targeted",
                    "timeout_secs": 5,
                    "paths": ["src/lib.rs"]
                }
            ]
        })
        .to_string(),
    )
    .expect("config");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:01:00Z".into()),
            event_id: Some("evt-source-write".into()),
            agent: "codex".into(),
            kind: EventKind::FileChange,
            file: Some("src/lib.rs".into()),
            rationale: Some("Change production code.".into()),
            ..Event::default()
        })
        .expect("event");

    let advice = advise_workspace(temp.path()).expect("advice");
    let requirement_id = "req-contract-do-not-continue-after-source-test-changes-when-relevant-verification-is-stale--unless-the-change-is-docs-only-and-policy-allows-it";

    assert_eq!(
        advice.final_action,
        ControlAction::ForceVerification {
            suite: VerificationSuite::Targeted,
            blocking: true,
        }
    );
    assert!(
        advice
            .control_rationale
            .requirement_ids
            .contains(&requirement_id.to_string()),
        "{:?}",
        advice.control_rationale
    );

    let run = run_verifier(temp.path(), "smoke").expect("verifier");
    assert_eq!(run.status, VerificationRunStatus::Passed);

    let report = load_requirement_graph(
        temp.path(),
        RequirementGraphQuery {
            requirement_id: Some(requirement_id.into()),
            limit: 10,
            ..RequirementGraphQuery::default()
        },
    )
    .expect("requirements report");

    assert_eq!(report.proofs.len(), 1);
    assert_eq!(
        report.proofs[0].control_refs[0].requirement_ids,
        vec![requirement_id.to_string()]
    );
    assert_eq!(
        report.proofs[0].control_refs[0].necessity,
        coding_agent_monitor::RequirementEvidenceNecessity::Necessary
    );
    assert_eq!(
        report.proofs[0].outcome_refs[0].requirement_ids,
        vec![requirement_id.to_string()]
    );
    assert_eq!(
        report.proofs[0].outcome_refs[0].status,
        OutcomeStatus::Succeeded
    );
    assert!(
        report.proofs[0]
            .proof_strength
            .signals
            .contains(&"monitor_control_decision".into()),
        "{:?}",
        report.proofs[0].proof_strength
    );
    assert!(
        report.proofs[0]
            .proof_strength
            .signals
            .contains(&"successful_outcome".into()),
        "{:?}",
        report.proofs[0].proof_strength
    );
}

#[test]
fn run_probe_records_successful_repo_inspection_outcome_for_latest_run_probe_advice() {
    let temp = tempfile::tempdir().expect("temp dir");
    init_git_repo(temp.path());
    std::fs::create_dir_all(temp.path().join("src")).expect("src dir");
    std::fs::write(temp.path().join("src/lib.rs"), "pub fn changed() {}\n").expect("dirty source");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    let advice = append_dispatched_action_advice_at(
        &mut store,
        temp.path(),
        "2026-06-22T12:20:00Z",
        ControlAction::RunProbe {
            probe: ProbeSpec::RepoInspection {
                target: Some("src/lib.rs".into()),
            },
        },
        "Local probe required",
    );

    let run = run_probe(temp.path()).expect("probe");
    let probe_log =
        std::fs::read_to_string(store.root().join("probe-runs.jsonl")).expect("probe log");
    let trails = load_decision_trails(store.root()).expect("trails");

    assert_eq!(run.advice_id, advice.advice_id);
    assert_eq!(run.status, OutcomeStatus::Succeeded);
    assert!(run.summary.contains("repo inspection"));
    assert!(run.summary.contains("src/lib.rs"));
    assert!(probe_log.contains(&run.probe_run_id));
    let trail = trails
        .iter()
        .find(|trail| trail.advice.advice_id == advice.advice_id)
        .expect("probe trail");
    assert_eq!(trail.outcomes.len(), 1);
    let outcome = &trail.outcomes[0];
    assert_eq!(outcome.action, ControlActionKind::RunProbe);
    assert_eq!(outcome.status, OutcomeStatus::Succeeded);
    assert!(outcome.evidence_ids.contains(&run.probe_run_id));
    assert!(
        outcome
            .observed_entropy_delta
            .iter()
            .any(|delta| delta.kind == EntropyKind::Plan && delta.delta < 0),
        "{outcome:?}"
    );
}

#[test]
fn run_probe_records_runtime_validation_evidence_probe_without_browser_bias() {
    let temp = tempfile::tempdir().expect("temp dir");
    init_git_repo(temp.path());
    let mut store = ProjectStore::open(temp.path()).expect("store");
    let advice = append_dispatched_action_advice_at(
        &mut store,
        temp.path(),
        "2026-06-22T12:20:30Z",
        ControlAction::RunProbe {
            probe: ProbeSpec::RuntimeValidation {
                surface: RuntimeValidationSurface::MobileApp,
                target: Some("login flow".into()),
            },
        },
        "Runtime validation evidence required",
    );

    let run = run_probe(temp.path()).expect("runtime validation probe");
    let trails = load_decision_trails(store.root()).expect("trails");

    assert_eq!(run.advice_id, advice.advice_id);
    assert_eq!(run.status, OutcomeStatus::Unknown);
    assert!(run.summary.contains("runtime validation unsupported"));
    assert!(run.summary.contains("mobile app"));
    assert!(run.summary.contains("login flow"));
    assert!(
        !run.summary.contains("browser"),
        "mobile runtime probe should not use browser wording: {run:?}"
    );
    let trail = trails
        .iter()
        .find(|trail| trail.advice.advice_id == advice.advice_id)
        .expect("probe trail");
    let outcome = trail.outcomes.first().expect("probe outcome");
    assert_eq!(outcome.action, ControlActionKind::RunProbe);
    assert_eq!(outcome.status, OutcomeStatus::Unknown);
    assert!(outcome.evidence_ids.contains(&run.probe_run_id));
}

#[test]
fn run_probe_executes_configured_runtime_validation_verifier() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    let command = passing_verifier_command();
    std::fs::write(
        store.root().join("config.json"),
        json!({
            "verifiers": [
                {
                    "id": "mobile-runtime",
                    "command": command,
                    "scope": "targeted",
                    "timeout_secs": 5,
                    "paths": ["mobile/app/src/MainActivity.kt"],
                    "acceptance_patterns": ["runtime_validation:mobile_app"]
                }
            ]
        })
        .to_string(),
    )
    .expect("config");
    let advice = append_dispatched_action_advice_at(
        &mut store,
        temp.path(),
        "2026-06-22T12:20:45Z",
        ControlAction::RunProbe {
            probe: ProbeSpec::RuntimeValidation {
                surface: RuntimeValidationSurface::MobileApp,
                target: Some("login flow".into()),
            },
        },
        "Runtime validation verifier required",
    );

    let run = run_probe(temp.path()).expect("runtime validation probe");
    let verifier_log =
        std::fs::read_to_string(store.root().join("verifier-runs.jsonl")).expect("verifier log");
    let trails = load_decision_trails(store.root()).expect("trails");

    assert_eq!(run.advice_id, advice.advice_id);
    assert_eq!(run.status, OutcomeStatus::Succeeded);
    assert!(run.summary.contains("runtime validation passed"));
    assert!(run.summary.contains("mobile-runtime"));
    assert!(verifier_log.contains("mobile-runtime"));
    let trail = trails
        .iter()
        .find(|trail| trail.advice.advice_id == advice.advice_id)
        .expect("probe trail");
    let outcome = trail.outcomes.first().expect("probe outcome");
    assert_eq!(outcome.action, ControlActionKind::RunProbe);
    assert_eq!(outcome.status, OutcomeStatus::Succeeded);
    assert!(outcome.evidence_ids.contains(&run.probe_run_id));
    assert!(
        run.evidence_ids
            .iter()
            .any(|evidence_id| evidence_id.starts_with("verifier-run-")),
        "{run:?}"
    );
}

#[test]
fn run_probe_executes_configured_targeted_test_probe() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    let command = passing_verifier_command();
    std::fs::write(
        store.root().join("config.json"),
        json!({
            "verifiers": [
                {
                    "id": "targeted-smoke",
                    "command": command,
                    "scope": "targeted",
                    "timeout_secs": 5,
                    "paths": ["src/lib.rs"]
                }
            ]
        })
        .to_string(),
    )
    .expect("config");
    let advice = append_dispatched_action_advice_at(
        &mut store,
        temp.path(),
        "2026-06-22T12:23:00Z",
        ControlAction::RunProbe {
            probe: ProbeSpec::TargetedTest {
                command: command.to_string(),
            },
        },
        "Local probe required",
    );

    let run = run_probe(temp.path()).expect("probe");
    let verifier_log =
        std::fs::read_to_string(store.root().join("verifier-runs.jsonl")).expect("verifier log");
    let trails = load_decision_trails(store.root()).expect("trails");

    assert_eq!(run.advice_id, advice.advice_id);
    assert_eq!(run.status, OutcomeStatus::Succeeded);
    assert!(run.summary.contains("targeted test probe passed"));
    assert!(run.summary.contains("targeted-smoke"));
    assert!(verifier_log.contains("targeted-smoke"));
    assert!(
        run.evidence_ids
            .iter()
            .any(|evidence_id| evidence_id.starts_with("verifier-run-")),
        "{run:?}"
    );
    let trail = trails
        .iter()
        .find(|trail| trail.advice.advice_id == advice.advice_id)
        .expect("probe trail");
    assert_eq!(trail.outcomes.len(), 1);
    let outcome = &trail.outcomes[0];
    assert_eq!(outcome.action, ControlActionKind::RunProbe);
    assert_eq!(outcome.status, OutcomeStatus::Succeeded);
    assert!(outcome.evidence_ids.contains(&run.probe_run_id));
    assert!(
        outcome
            .evidence_ids
            .iter()
            .any(|evidence_id| evidence_id.starts_with("verifier-run-")),
        "{outcome:?}"
    );
}

#[test]
fn run_probe_records_local_evidence_without_running_verifiers() {
    let temp = tempfile::tempdir().expect("temp dir");
    init_git_repo(temp.path());
    std::fs::create_dir_all(temp.path().join("src")).expect("src dir");
    std::fs::write(temp.path().join("src/lib.rs"), "pub fn changed() {}\n").expect("dirty source");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    std::fs::write(
        store.root().join("config.json"),
        json!({
            "verifiers": [
                {
                    "id": "should-not-run",
                    "command": passing_verifier_command(),
                    "scope": "targeted",
                    "timeout_secs": 5,
                    "paths": ["src/lib.rs"]
                }
            ]
        })
        .to_string(),
    )
    .expect("config");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:21:00Z".into()),
            event_id: Some("evt-local-goal".into()),
            agent: "user".into(),
            kind: EventKind::UserInstruction,
            content: Some("Fix the routine next step without asking me.".into()),
            ..Event::default()
        })
        .expect("goal event");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:22:00Z".into()),
            event_id: Some("evt-local-question".into()),
            agent: "codex".into(),
            kind: EventKind::ModelMessage,
            content: Some("Which file should I inspect next?".into()),
            ..Event::default()
        })
        .expect("question event");
    let advice = append_dispatched_action_advice_at(
        &mut store,
        temp.path(),
        "2026-06-22T12:25:00Z",
        ControlAction::RunProbe {
            probe: ProbeSpec::LocalEvidence {
                target: Some("routine_next_step".into()),
            },
        },
        "Local probe required",
    );

    let run = run_probe(temp.path()).expect("local evidence probe");
    let trails = load_decision_trails(store.root()).expect("trails");

    assert_eq!(run.advice_id, advice.advice_id);
    assert_eq!(run.status, OutcomeStatus::Succeeded);
    assert!(run.summary.contains("local evidence probe"));
    assert!(run.summary.contains("routine_next_step"));
    assert!(run.summary.contains("repo inspection"));
    assert!(run.evidence_ids.contains(&"evt-local-question".into()));
    assert!(
        run.evidence_ids
            .iter()
            .any(|evidence_id| evidence_id.starts_with("repo-audit-")),
        "{run:?}"
    );
    assert!(
        !store.root().join("verifier-runs.jsonl").exists(),
        "LocalEvidence must not execute configured verifiers"
    );
    let trail = trails
        .iter()
        .find(|trail| trail.advice.advice_id == advice.advice_id)
        .expect("probe trail");
    let outcome = trail.outcomes.first().expect("probe outcome");
    assert_eq!(outcome.action, ControlActionKind::RunProbe);
    assert_eq!(outcome.status, OutcomeStatus::Succeeded);
    assert!(outcome.evidence_ids.contains(&run.probe_run_id));
    assert!(outcome.evidence_ids.contains(&"evt-local-question".into()));
}

#[test]
fn run_probe_rejects_unconfigured_targeted_test_command_without_side_effects() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    append_dispatched_action_advice_at(
        &mut store,
        temp.path(),
        "2026-06-22T12:24:00Z",
        ControlAction::RunProbe {
            probe: ProbeSpec::TargetedTest {
                command: "cargo test parser::nested".into(),
            },
        },
        "Local probe required",
    );

    let error = run_probe(temp.path()).expect_err("unconfigured command should be rejected");

    assert!(
        error
            .to_string()
            .contains("targeted_test command is not configured as a verifier"),
        "{error}"
    );
    assert!(!store.root().join("probe-runs.jsonl").exists());
    assert!(!store.root().join("verifier-runs.jsonl").exists());
    assert!(!store.root().join("outcomes.jsonl").exists());
}

#[test]
fn run_probe_does_not_attach_outcome_when_latest_advice_is_not_run_probe() {
    let temp = tempfile::tempdir().expect("temp dir");
    init_git_repo(temp.path());
    let mut store = ProjectStore::open(temp.path()).expect("store");
    let probe_advice = append_dispatched_action_advice_at(
        &mut store,
        temp.path(),
        "2026-06-22T12:21:00Z",
        ControlAction::RunProbe {
            probe: ProbeSpec::RepoInspection { target: None },
        },
        "Local probe required",
    );
    append_action_advice_at(
        &mut store,
        temp.path(),
        "2026-06-22T12:22:00Z",
        ControlAction::ContinueWorking,
        "Continue working",
    );

    let error = run_probe(temp.path()).expect_err("stale probe advice should be rejected");
    let trails = load_decision_trails(store.root()).expect("trails");
    let probe_trail = trails
        .iter()
        .find(|trail| trail.advice.advice_id == probe_advice.advice_id)
        .expect("probe trail");

    assert!(error.to_string().contains("latest advice is not run_probe"));
    assert!(probe_trail.outcomes.is_empty());
    assert!(!store.root().join("probe-runs.jsonl").exists());
}

#[test]
fn run_verifier_records_failed_outcome_for_latest_force_verification_advice() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    std::fs::write(
        store.root().join("config.json"),
        json!({
            "verifiers": [
                {
                    "id": "smoke",
                    "command": failing_verifier_command(),
                    "scope": "targeted",
                    "timeout_secs": 5,
                    "paths": ["src/lib.rs"]
                }
            ]
        })
        .to_string(),
    )
    .expect("config");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:01:00Z".into()),
            event_id: Some("evt-write".into()),
            agent: "codex".into(),
            kind: EventKind::FileChange,
            file: Some("src/lib.rs".into()),
            rationale: Some("Change production code.".into()),
            ..Event::default()
        })
        .expect("event");
    let advice = advise_workspace(temp.path()).expect("advice");

    let run = run_verifier(temp.path(), "smoke").expect("verifier");
    let trails = load_decision_trails(store.root()).expect("trails");

    assert_eq!(run.status, VerificationRunStatus::Failed);
    let trail = trails
        .iter()
        .find(|trail| trail.advice.advice_id == advice.advice_id)
        .expect("advice trail");
    assert_eq!(trail.outcomes.len(), 1);
    let outcome = &trail.outcomes[0];
    assert_eq!(outcome.action, ControlActionKind::ForceVerification);
    assert_eq!(outcome.status, OutcomeStatus::Failed);
    assert!(outcome.evidence_ids.contains(&run.verifier_run_id));
    assert!(
        outcome
            .observed_entropy_delta
            .iter()
            .any(|delta| delta.kind == EntropyKind::Verification && delta.delta >= 0)
    );
}

#[test]
fn run_verifier_does_not_attach_outcome_to_stale_force_verification_advice() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    std::fs::write(
        store.root().join("config.json"),
        json!({
            "verifiers": [
                {
                    "id": "smoke",
                    "command": passing_verifier_command(),
                    "scope": "targeted",
                    "timeout_secs": 5,
                    "paths": ["src/lib.rs"]
                }
            ]
        })
        .to_string(),
    )
    .expect("config");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:01:00Z".into()),
            event_id: Some("evt-write".into()),
            agent: "codex".into(),
            kind: EventKind::FileChange,
            file: Some("src/lib.rs".into()),
            rationale: Some("Change production code.".into()),
            ..Event::default()
        })
        .expect("event");
    let force_advice = advise_workspace(temp.path()).expect("force advice");
    let mut later_advice = force_advice.clone();
    later_advice.advice_id = "later-continue".into();
    later_advice.final_action = ControlAction::ContinueWorking;
    store
        .append_advice(&later_advice)
        .expect("append later advice");

    run_verifier(temp.path(), "smoke").expect("verifier");
    let trails = load_decision_trails(store.root()).expect("trails");
    let force_trail = trails
        .iter()
        .find(|trail| trail.advice.advice_id == force_advice.advice_id)
        .expect("force trail");

    assert!(force_trail.outcomes.is_empty());
}

#[test]
fn run_verifier_does_not_attach_outcome_when_current_force_verification_command_differs() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    std::fs::write(
        store.root().join("config.json"),
        json!({
            "verifiers": [
                {
                    "id": "smoke",
                    "command": passing_verifier_command(),
                    "scope": "targeted",
                    "timeout_secs": 5,
                    "paths": ["src/lib.rs"]
                }
            ]
        })
        .to_string(),
    )
    .expect("config");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:01:00Z".into()),
            event_id: Some("evt-write".into()),
            agent: "codex".into(),
            kind: EventKind::FileChange,
            file: Some("src/lib.rs".into()),
            rationale: Some("Change production code.".into()),
            ..Event::default()
        })
        .expect("event");
    let first_advice = advise_workspace(temp.path()).expect("force advice");
    let mut mismatched_advice = first_advice.clone();
    mismatched_advice.advice_id = "later-different-verifier".into();
    mismatched_advice.packet.instructions = vec![PacketInstruction {
        priority: PacketInstructionPriority::Must,
        text: "Run `different verifier command` before making more code edits.".into(),
    }];
    store
        .append_advice(&mismatched_advice)
        .expect("append mismatched advice");

    run_verifier(temp.path(), "smoke").expect("verifier");
    let trails = load_decision_trails(store.root()).expect("trails");
    let mismatched_trail = trails
        .iter()
        .find(|trail| trail.advice.advice_id == "later-different-verifier")
        .expect("mismatched trail");

    assert!(mismatched_trail.outcomes.is_empty());
}

#[test]
fn run_verifier_does_not_attach_outcome_after_newer_handoff_dispatch() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    std::fs::write(
        store.root().join("config.json"),
        json!({
            "verifiers": [
                {
                    "id": "smoke",
                    "command": passing_verifier_command(),
                    "scope": "targeted",
                    "timeout_secs": 5,
                    "paths": ["src/lib.rs"]
                }
            ]
        })
        .to_string(),
    )
    .expect("config");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:01:00Z".into()),
            event_id: Some("evt-write".into()),
            agent: "codex".into(),
            kind: EventKind::FileChange,
            file: Some("src/lib.rs".into()),
            rationale: Some("Change production code.".into()),
            ..Event::default()
        })
        .expect("event");
    let force_advice = advise_workspace(temp.path()).expect("force advice");
    coding_agent_monitor::handoff_workspace(temp.path(), AgentKind::ClaudeCode)
        .expect("handoff dispatch");

    run_verifier(temp.path(), "smoke").expect("verifier");
    let trails = load_decision_trails(store.root()).expect("trails");
    let force_trail = trails
        .iter()
        .find(|trail| trail.advice.advice_id == force_advice.advice_id)
        .expect("force trail");

    assert!(force_trail.outcomes.is_empty());
}

#[test]
fn full_force_verification_advice_respects_explicit_packet_command_mismatch() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    std::fs::write(
        store.root().join("config.json"),
        json!({
            "verifiers": [
                {
                    "id": "smoke",
                    "command": passing_verifier_command(),
                    "scope": "full",
                    "timeout_secs": 5,
                    "paths": []
                }
            ]
        })
        .to_string(),
    )
    .expect("config");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:01:00Z".into()),
            event_id: Some("evt-write".into()),
            agent: "codex".into(),
            kind: EventKind::FileChange,
            file: Some("src/lib.rs".into()),
            rationale: Some("Change production code.".into()),
            ..Event::default()
        })
        .expect("event");
    let first_advice = advise_workspace(temp.path()).expect("force advice");
    let mut mismatched_advice = first_advice.clone();
    mismatched_advice.advice_id = "later-full-different-verifier".into();
    mismatched_advice.final_action = ControlAction::ForceVerification {
        suite: VerificationSuite::Full,
        blocking: true,
    };
    mismatched_advice.packet.instructions = vec![PacketInstruction {
        priority: PacketInstructionPriority::Must,
        text: "Run `different verifier command` before making more code edits.".into(),
    }];
    store
        .append_advice(&mismatched_advice)
        .expect("append mismatched advice");

    run_verifier(temp.path(), "smoke").expect("verifier");
    let trails = load_decision_trails(store.root()).expect("trails");
    let mismatched_trail = trails
        .iter()
        .find(|trail| trail.advice.advice_id == "later-full-different-verifier")
        .expect("mismatched trail");

    assert!(mismatched_trail.outcomes.is_empty());
}

#[test]
fn verifier_outcome_expected_delta_uses_final_policy_action_after_advisor_rejection() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:01:00Z".into()),
            event_id: Some("evt-write".into()),
            agent: "codex".into(),
            kind: EventKind::FileChange,
            file: Some("src/lib.rs".into()),
            rationale: Some("Change production code.".into()),
            ..Event::default()
        })
        .expect("event");
    let decision = json!({
        "diagnosis_id": "diagnosis-plan",
        "dominant_entropy": "plan",
        "entropy_scores": {
            "plan": { "score": 80, "confidence": 80 }
        },
        "top_evidence": [
            {
                "event_id": "evt-write",
                "why_it_matters": "Advisor focused on planning."
            }
        ],
        "cited_evidence_ids": ["evt-write"],
        "missing_evidence": [],
        "proposed_action": {
            "type": "ask_user",
            "question": "Should I continue?"
        },
        "expected_entropy_delta": [
            { "kind": "plan", "delta": -25 }
        ],
        "packet_intent": "ask whether to continue",
        "packet_draft": {
            "urgency": "follow_up",
            "summary": "Continue.",
            "instructions": ["Continue."],
            "evidence_refs": ["evt-write"]
        },
        "ask_user": null,
        "confidence": 0.82
    });
    let (endpoint, _request_rx) = serve_advisor_once(decision);
    let env_name = "CAM_TEST_ADVISOR_KEY_OUTCOME_EXPECTED";
    set_test_env_var(env_name, "test-key");
    std::fs::write(
        store.root().join("config.json"),
        json!({
            "advisor": {
                "enabled": true,
                "provider": {
                    "endpoint": endpoint,
                    "model": "test-advisor",
                    "api_key_env": env_name,
                    "timeout_secs": 5
                }
            },
            "verifiers": [
                {
                    "id": "smoke",
                    "command": passing_verifier_command(),
                    "scope": "targeted",
                    "timeout_secs": 5,
                    "paths": ["src/lib.rs"]
                }
            ]
        })
        .to_string(),
    )
    .expect("config");
    let advice = advise_workspace(temp.path()).expect("advice");
    assert!(advice.advisor_decision.is_none());
    assert!(
        advice
            .advisor_error
            .as_deref()
            .is_some_and(|error| error.contains("forbidden action"))
    );
    assert!(matches!(
        advice.validation_outcome,
        ValidationOutcome::Approved(ControlAction::ForceVerification { .. })
    ));

    run_verifier(temp.path(), "smoke").expect("verifier");
    let trails = load_decision_trails(store.root()).expect("trails");
    let trail = trails
        .iter()
        .find(|trail| trail.advice.advice_id == advice.advice_id)
        .expect("advice trail");
    let outcome = trail.outcomes.first().expect("outcome");

    assert!(
        outcome
            .expected_entropy_delta
            .iter()
            .any(|delta| delta.kind == EntropyKind::Verification && delta.delta < 0)
    );
    assert!(
        !outcome
            .expected_entropy_delta
            .iter()
            .any(|delta| delta.kind == EntropyKind::Plan)
    );
}

#[test]
fn decision_trail_replay_ignores_incomplete_trailing_jsonl_record() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:01:00Z".into()),
            event_id: Some("evt-write".into()),
            agent: "codex".into(),
            kind: EventKind::FileChange,
            file: Some("src/lib.rs".into()),
            rationale: Some("Change production code.".into()),
            ..Event::default()
        })
        .expect("event");
    let advice = advise_workspace(temp.path()).expect("advice");
    std::fs::OpenOptions::new()
        .append(true)
        .open(store.root().join("case-files.jsonl"))
        .expect("open case log")
        .write_all(br#"{"case_file_id":"partial"#)
        .expect("partial write");

    let trails = load_decision_trails(store.root()).expect("trails");

    assert_eq!(trails.len(), 1);
    assert_eq!(trails[0].advice.advice_id, advice.advice_id);
}

#[test]
fn decision_trail_replay_accepts_legacy_advice_without_inline_dispatch_result() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    store
        .append_event(&Event {
            time: Some("2026-06-22T12:01:00Z".into()),
            event_id: Some("evt-write".into()),
            agent: "codex".into(),
            kind: EventKind::FileChange,
            file: Some("src/lib.rs".into()),
            rationale: Some("Change production code.".into()),
            ..Event::default()
        })
        .expect("event");
    let advice = advise_workspace(temp.path()).expect("advice");
    let mut legacy_advice = serde_json::to_value(&advice).expect("advice json");
    let legacy_object = legacy_advice.as_object_mut().expect("advice object");
    legacy_object.remove("dispatch_result");
    legacy_object.remove("validation_outcome");
    legacy_object.remove("control_rationale");
    std::fs::write(
        store.root().join("advice.jsonl"),
        format!("{legacy_advice}\n"),
    )
    .expect("legacy advice");

    let trails = load_decision_trails(store.root()).expect("trails");

    assert_eq!(trails.len(), 1);
    assert_eq!(trails[0].advice.advice_id, advice.advice_id);
    assert!(matches!(
        trails[0].advice.validation_outcome,
        ValidationOutcome::Approved(ControlAction::ContinueWorking)
    ));
    assert_eq!(
        trails[0].advice.control_rationale.selected_action,
        ControlActionKind::ContinueWorking
    );
    assert!(trails[0].advice.control_rationale.reason.contains("legacy"));
    assert_eq!(
        trails[0].dispatch.dispatch_id,
        advice.dispatch_result.dispatch_id
    );
}

#[test]
fn decision_trail_replay_uses_inline_packet_and_dispatch_when_side_logs_are_missing() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut store = ProjectStore::open(temp.path()).expect("store");
    let snapshot = DashboardSnapshot::load(store.root(), 20).expect("snapshot");
    let case_file = build_control_case_file(temp.path(), &snapshot);
    store.append_case_file(&case_file).expect("case file");
    let packet = ControlPacket {
        packet_id: "packet-inline-only".into(),
        target_agent: "codex".into(),
        urgency: PacketUrgency::FollowUp,
        title: "Continue working".into(),
        summary: "No blocking uncertainty.".into(),
        instructions: vec![PacketInstruction {
            priority: PacketInstructionPriority::Should,
            text: "Continue the current task.".into(),
        }],
        evidence_refs: vec![],
        forbidden: vec![],
        success_criteria: vec!["Work continues.".into()],
        preconditions: PacketPreconditions::default(),
    };
    let dispatch = coding_agent_monitor::DispatchResult {
        dispatch_id: "dispatch-inline-only".into(),
        packet_id: packet.packet_id.clone(),
        target_agent: packet.target_agent.clone(),
        status: DispatchStatus::OutboxWritten,
        path: Some("inline/path.md".into()),
        reason: None,
    };
    let advice = coding_agent_monitor::AdviceRun {
        advice_id: "advice-inline-only".into(),
        case_file_id: case_file.case_file_id.clone(),
        advisor_used: false,
        advisor_error: None,
        advisor_decision: None,
        validation_outcome: ValidationOutcome::Approved(ControlAction::ContinueWorking),
        final_action: ControlAction::ContinueWorking,
        control_rationale: Default::default(),
        packet: packet.clone(),
        dispatch_result: dispatch.clone(),
        packet_path: dispatch.path.clone(),
    };
    store.append_advice(&advice).expect("advice");

    let trails = load_decision_trails(store.root()).expect("trails");

    assert_eq!(trails.len(), 1);
    assert_eq!(trails[0].packet.packet_id, packet.packet_id);
    assert_eq!(trails[0].dispatch.dispatch_id, dispatch.dispatch_id);
}
