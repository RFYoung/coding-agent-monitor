//! Deterministic control-decision and calibration layer.
//!
//! Maps a bounded case file to the cheapest permitted control action, detects
//! agent-health/verifier failure loops, and shrinks expected entropy deltas
//! using recorded outcome calibration. Pure logic over `model` types; no I/O
//! beyond loading the calibration report through `ProjectStore`.

use crate::*;

pub(crate) const USER_DECISION_ASK_USER_THRESHOLD: u8 = 80;
pub(crate) const RETRY_AGENT_HEALTH_THRESHOLD: u8 = 75;
pub(crate) const FOLLOW_UP_PLAN_THRESHOLD: u8 = 60;
pub(crate) const FOLLOW_UP_REPO_BLAME_THRESHOLD: u8 = 75;
pub(crate) const SPAWN_FRESH_CONTEXT_THRESHOLD: u8 = 80;
pub(crate) const SPAWN_FRESH_AGENT_HEALTH_THRESHOLD: u8 = 90;
pub(crate) const SWITCH_AGENT_HEALTH_THRESHOLD: u8 = 90;
pub(crate) const SUBAGENT_WIP_CAP: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum FailureLayer {
    Provider,
    Transport,
    RateLimit,
    Auth,
    ShellRuntime,
    RepoState,
    TestRuntime,
    TaskLogic,
    Unknown,
}

impl FailureLayer {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Provider => "provider",
            Self::Transport => "transport",
            Self::RateLimit => "rate_limit",
            Self::Auth => "auth",
            Self::ShellRuntime => "shell_runtime",
            Self::RepoState => "repo_state",
            Self::TestRuntime => "test_runtime",
            Self::TaskLogic => "task_logic",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SubagentLifecycleAction {
    Spawned,
    Terminal,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct VerifierFailureLoop {
    pub(crate) command: String,
    pub(crate) repeated_after_edits: usize,
    pub(crate) edits_since_last_failure: usize,
    pub(crate) hypothesis_since_last_failure: bool,
    pub(crate) evidence_id: String,
}

pub(crate) fn verifier_run_verification_status(run: &VerifierRun) -> VerificationStatus {
    match run.status {
        VerificationRunStatus::Passed => VerificationStatus::Passed,
        VerificationRunStatus::Failed | VerificationRunStatus::TimedOut => {
            VerificationStatus::Failed
        }
    }
}

pub(crate) fn normalize_command_signature(command: &str) -> String {
    command.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub(crate) fn event_is_verification_result(event: &Event) -> bool {
    matches!(event.kind, EventKind::CommandResult | EventKind::TestResult)
        && event
            .command
            .as_deref()
            .is_some_and(is_verification_command)
}

pub(crate) fn event_is_intended_environment_validation_result(event: &Event) -> bool {
    matches!(event.kind, EventKind::CommandResult | EventKind::TestResult)
        && event
            .command
            .as_deref()
            .is_some_and(|command| !validation_surfaces_for_command(command).is_empty())
}

pub(crate) fn verifier_failure_signature(event: &Event) -> Option<String> {
    if !event_is_verification_result(event) || event.exit_code == Some(0) {
        return None;
    }
    let command = event.command.as_deref().map(normalize_command_signature)?;
    let output = event
        .content
        .as_deref()
        .map(verifier_failure_output_signature)
        .unwrap_or_else(|| "<no-output>".into());
    Some(format!("{command}::{output}"))
}

pub(crate) fn verifier_failure_output_signature(output: &str) -> String {
    let first_line = output
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("<empty>");
    truncate_evidence(&normalize_command_signature(first_line)).to_ascii_lowercase()
}

pub(crate) fn event_records_failure_hypothesis(event: &Event, content: &str) -> bool {
    if matches!(event.kind, EventKind::DesignThought) && !content.trim().is_empty() {
        return true;
    }
    let text = content.to_ascii_lowercase();
    contains_failure_signal(
        &text,
        &[
            "hypothesis:",
            "diagnosis:",
            "root cause",
            "failure hypothesis",
            "failure signature",
            "isolation probe",
            "suspect ",
            "i think the failure",
        ],
    )
}

pub(crate) fn event_establishes_bug_reproduction_or_localization(
    event: &Event,
    content: &str,
) -> bool {
    if event_is_change_like(event) {
        return false;
    }
    if event_is_verification_result(event) || event_is_intended_environment_validation_result(event)
    {
        return true;
    }
    let text = content.to_ascii_lowercase();
    if contains_failure_signal(
        &text,
        &[
            "reproduce",
            "reproduced",
            "localized",
            "localised",
            "root cause",
            "diagnosis:",
            "hypothesis:",
            "stack trace",
            "traceback",
            "failing test",
            "failure signature",
            "logs show",
            "browser shows",
            "playwright shows",
            "simulator shows",
            "emulator shows",
            "device shows",
            "gui shows",
            "service smoke shows",
            "integration shows",
            "eval shows",
            "evaluation shows",
            "benchmark shows",
            "observed failure",
        ],
    ) {
        return true;
    }
    event
        .command
        .as_deref()
        .is_some_and(is_localization_probe_command)
}

pub(crate) fn is_localization_probe_command(command: &str) -> bool {
    let command = normalize_command_signature(command).to_ascii_lowercase();
    [
        "rg ",
        "grep ",
        "findstr ",
        "select-string ",
        "get-content ",
        "cat ",
        "type ",
        "sed ",
        "ls ",
        "dir ",
        "git diff",
        "git status",
        "curl ",
        "invoke-webrequest ",
        "playwright",
        "scripts/probe",
        "probe.py",
    ]
    .iter()
    .any(|signal| command.starts_with(signal) || command.contains(signal))
}

pub(crate) fn event_is_change_like(event: &Event) -> bool {
    matches!(event.kind, EventKind::FileChange | EventKind::RepoDiff)
}

pub(crate) fn event_breaks_rediscovery_loop(event: &Event) -> bool {
    event_is_change_like(event)
        || event_is_verification_result(event)
        || matches!(
            event.kind,
            EventKind::UserInstruction | EventKind::DesignThought | EventKind::HandoffSummary
        )
}

pub(crate) fn inspection_loop_target(event: &Event) -> Option<String> {
    if !matches!(
        event.kind,
        EventKind::ToolCall | EventKind::ToolResult | EventKind::CommandResult
    ) {
        return None;
    }
    let label = event
        .command
        .as_deref()
        .or_else(|| event.content.as_deref()?.strip_prefix("tool command: "))?;
    inspection_command_target(label)
}

pub(crate) fn inspection_command_target(command: &str) -> Option<String> {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return None;
    }
    let lower = trimmed.to_ascii_lowercase();
    let head = lower.split_whitespace().next()?;
    let is_inspection = matches!(
        head,
        "read"
            | "grep"
            | "glob"
            | "search"
            | "rg"
            | "findstr"
            | "select-string"
            | "get-content"
            | "cat"
            | "type"
            | "ls"
            | "dir"
    );
    is_inspection.then(|| normalize_command_signature(trimmed))
}

pub(crate) fn subagent_lifecycle_action(event: &Event) -> Option<SubagentLifecycleAction> {
    if !matches!(
        event.kind,
        EventKind::ToolCall
            | EventKind::ToolResult
            | EventKind::CommandResult
            | EventKind::AgentHealth
    ) {
        return None;
    }

    let text = subagent_lifecycle_text(event);
    if text.is_empty() {
        return None;
    }
    if contains_failure_signal(
        &text,
        &[
            "close_agent",
            "wait_agent",
            "subagent stopped",
            "subagent stop",
            "joined_with_summary",
            "cancelled_with_reason",
            "timed_out",
            "superseded",
            "subagent failed",
            "worker failed",
        ],
    ) {
        return Some(SubagentLifecycleAction::Terminal);
    }
    if contains_failure_signal(
        &text,
        &[
            "spawn_agent",
            "subagent started",
            "spawned subagent",
            "worker started",
        ],
    ) || subagent_task_tool_started(event)
    {
        return Some(SubagentLifecycleAction::Spawned);
    }
    None
}

pub(crate) fn subagent_lifecycle_text(event: &Event) -> String {
    let mut text = String::new();
    if let Some(command) = event.command.as_deref() {
        text.push_str(command);
        text.push('\n');
    }
    if let Some(content) = event.content.as_deref() {
        text.push_str(content);
    }
    text.to_ascii_lowercase()
}

pub(crate) fn subagent_ownership_paths(event: &Event) -> Vec<String> {
    let mut paths = Vec::new();
    if let Some(file) = event.file.as_deref() {
        push_subagent_ownership_path(&mut paths, file);
    }
    if let Some(command) = event.command.as_deref() {
        collect_subagent_ownership_paths_from_text(&mut paths, command);
    }
    if let Some(content) = event.content.as_deref() {
        collect_subagent_ownership_paths_from_text(&mut paths, content);
    }
    paths
}

pub(crate) fn collect_subagent_ownership_paths_from_text(paths: &mut Vec<String>, text: &str) {
    for token in text.split_whitespace() {
        let candidate = token
            .split_once('=')
            .map(|(_, value)| value)
            .unwrap_or(token);
        push_subagent_ownership_path(paths, candidate);
    }
}

pub(crate) fn push_subagent_ownership_path(paths: &mut Vec<String>, candidate: &str) {
    let path = normalize_subagent_ownership_path(candidate);
    if path.is_empty() || paths.contains(&path) {
        return;
    }
    paths.push(path);
}

pub(crate) fn normalize_subagent_ownership_path(candidate: &str) -> String {
    let trimmed = candidate
        .trim_matches(|ch: char| {
            ch == '`'
                || ch == '\''
                || ch == '"'
                || ch == ','
                || ch == ';'
                || ch == ')'
                || ch == '('
                || ch == '['
                || ch == ']'
                || ch == '{'
                || ch == '}'
        })
        .replace('\\', "/")
        .to_ascii_lowercase();
    if trimmed.is_empty() || !(trimmed.contains('/') || trimmed.contains('.')) {
        return String::new();
    }
    let path = trimmed
        .trim_start_matches("./")
        .trim_start_matches('/')
        .to_string();
    if is_source_or_test_file(&path) || is_ui_validation_relevant_file(&path) {
        path
    } else {
        String::new()
    }
}

pub(crate) fn subagent_task_tool_started(event: &Event) -> bool {
    if event.kind != EventKind::ToolCall {
        return false;
    }
    event.command.as_deref().is_some_and(|command| {
        command
            .split_whitespace()
            .next()
            .is_some_and(|head| head.eq_ignore_ascii_case("task"))
    })
}

pub(crate) fn event_is_successful_result(event: &Event) -> bool {
    matches!(
        event.kind,
        EventKind::CommandResult | EventKind::ToolResult | EventKind::TestResult
    ) && event.exit_code == Some(0)
}

pub(crate) fn event_can_clear_service_failure(event: &Event, content: &str) -> bool {
    (matches!(
        event.kind,
        EventKind::ModelMessage
            | EventKind::CommandOutput
            | EventKind::CommandResult
            | EventKind::ToolCall
            | EventKind::ToolResult
            | EventKind::TestResult
            | EventKind::UserInstruction
            | EventKind::HandoffSummary
            | EventKind::AgentHealth
            | EventKind::VerificationClaim
            | EventKind::InterventionResult
    ) && !content.trim().is_empty())
        || event_is_successful_result(event)
}

pub(crate) fn retry_agent_entropy_allowed(case_file: &ControlCaseFile) -> bool {
    case_file
        .entropy
        .score(EntropyKind::AgentHealth)
        .is_some_and(|score| score.score >= RETRY_AGENT_HEALTH_THRESHOLD)
}

pub(crate) fn send_follow_up_entropy_allowed(case_file: &ControlCaseFile) -> bool {
    case_file
        .entropy
        .score(EntropyKind::Plan)
        .is_some_and(|score| score.score >= FOLLOW_UP_PLAN_THRESHOLD)
        || case_file
            .entropy
            .score(EntropyKind::RepoBlame)
            .is_some_and(|score| score.score >= FOLLOW_UP_REPO_BLAME_THRESHOLD)
        || case_file
            .entropy
            .score(EntropyKind::Context)
            .is_some_and(|score| score.score >= SPAWN_FRESH_CONTEXT_THRESHOLD)
}

pub(crate) fn run_probe_entropy_allowed(case_file: &ControlCaseFile) -> bool {
    case_file
        .entropy
        .score(EntropyKind::Plan)
        .is_some_and(|score| {
            score.score >= FOLLOW_UP_PLAN_THRESHOLD
                && score.top_causes.iter().any(|cause| {
                    let cause = cause.to_ascii_lowercase();
                    cause.contains("routine user question")
                        || cause.contains("reproduction or localization evidence")
                        || cause.contains("repeatedly inspected")
                        || cause.contains("same verifier failure signature")
                })
        })
}

pub(crate) fn probe_spec_for_case_file(case_file: &ControlCaseFile) -> ProbeSpec {
    if plan_entropy_mentions_routine_agent_question(case_file) {
        return ProbeSpec::LocalEvidence {
            target: Some("routine_next_step".into()),
        };
    }
    if plan_entropy_mentions_bug_fix_pre_edit_gap(case_file) {
        return ProbeSpec::LocalEvidence {
            target: Some("bug_reproduction_or_localization".into()),
        };
    }
    if plan_entropy_mentions_repeated_inspection_loop(case_file) {
        return ProbeSpec::RepoInspection {
            target: Some("repeated_inspection_target".into()),
        };
    }
    ProbeSpec::LocalEvidence { target: None }
}

pub(crate) fn spawn_judge_entropy_allowed(case_file: &ControlCaseFile) -> bool {
    case_file
        .entropy
        .score(EntropyKind::RepoBlame)
        .is_some_and(|score| score.score >= FOLLOW_UP_REPO_BLAME_THRESHOLD)
}

pub(crate) fn switch_agent_entropy_allowed(case_file: &ControlCaseFile) -> bool {
    case_file
        .entropy
        .score(EntropyKind::AgentHealth)
        .is_some_and(|score| score.score >= SWITCH_AGENT_HEALTH_THRESHOLD)
}

pub(crate) fn spawn_fresh_entropy_allowed(case_file: &ControlCaseFile) -> bool {
    case_file
        .entropy
        .score(EntropyKind::Context)
        .is_some_and(|score| score.score >= SPAWN_FRESH_CONTEXT_THRESHOLD)
        || case_file
            .entropy
            .score(EntropyKind::AgentHealth)
            .is_some_and(|score| score.score >= SPAWN_FRESH_AGENT_HEALTH_THRESHOLD)
}

pub(crate) fn deterministic_control_action(case_file: &ControlCaseFile) -> ControlAction {
    deterministic_control_action_with_calibration(case_file, &ControlCalibration::default())
}

pub(crate) fn deterministic_control_action_with_calibration(
    case_file: &ControlCaseFile,
    calibration: &ControlCalibration,
) -> ControlAction {
    if case_file
        .entropy
        .score(EntropyKind::Verification)
        .is_some_and(|score| score.score >= 75)
    {
        return force_verification_control_action(case_file);
    }

    if trace_and_verification_block_required(case_file)
        && judge_agent_for_case_file(case_file).is_none()
    {
        return trace_and_verification_block_action(case_file);
    }

    utility_ranked_control_action(case_file, calibration).unwrap_or(ControlAction::ContinueWorking)
}

pub(crate) fn force_verification_control_action(case_file: &ControlCaseFile) -> ControlAction {
    ControlAction::ForceVerification {
        suite: if case_file.verification.recommended_commands.is_empty() {
            VerificationSuite::Full
        } else {
            VerificationSuite::Targeted
        },
        blocking: true,
    }
}

pub(crate) fn trace_and_verification_block_required(case_file: &ControlCaseFile) -> bool {
    case_file
        .entropy
        .score(EntropyKind::RepoBlame)
        .is_some_and(|score| {
            score.score >= FOLLOW_UP_REPO_BLAME_THRESHOLD
                && score.top_causes.iter().any(|cause| {
                    let cause = cause.to_ascii_lowercase();
                    cause.contains("lack trace evidence")
                        || cause.contains("without rationale")
                        || cause.contains("lacks rationale")
                })
        })
}

pub(crate) fn trace_and_verification_block_action(case_file: &ControlCaseFile) -> ControlAction {
    let reason = case_file
        .entropy
        .score(EntropyKind::RepoBlame)
        .and_then(|score| score.top_causes.first())
        .cloned()
        .unwrap_or_else(|| "repo/blame entropy requires trace and verification repair".into());
    ControlAction::BlockProgressUntilTraceAndVerification { reason }
}

#[derive(Debug)]
pub(crate) struct ControlActionCandidate {
    action: ControlAction,
    utility: i32,
    priority: i32,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ControlCalibration {
    action_penalties: BTreeMap<ControlActionKind, i32>,
    target_penalties: BTreeMap<(ControlActionKind, String), i32>,
    action_expected_deltas: BTreeMap<ControlActionKind, Vec<EntropyDelta>>,
    target_expected_deltas: BTreeMap<(ControlActionKind, String), Vec<EntropyDelta>>,
}

impl ControlCalibration {
    fn penalty_for(&self, action: &ControlAction) -> i32 {
        self.action_penalties
            .get(&action.kind())
            .copied()
            .unwrap_or_default()
            + self.target_penalty_for(action)
    }

    fn target_penalty_for(&self, action: &ControlAction) -> i32 {
        control_action_target_agents(action)
            .into_iter()
            .filter_map(|target_agent| {
                self.target_penalties
                    .get(&(action.kind(), target_agent.to_string()))
                    .copied()
            })
            .max()
            .unwrap_or_default()
    }

    fn has_penalties(&self) -> bool {
        self.action_penalties.values().any(|penalty| *penalty > 0)
            || self.target_penalties.values().any(|penalty| *penalty > 0)
    }

    fn expected_delta_adjustment_for(&self, action: &ControlAction) -> Option<&[EntropyDelta]> {
        for target_agent in control_action_target_agents(action) {
            if let Some(deltas) = self
                .target_expected_deltas
                .get(&(action.kind(), target_agent.to_string()))
            {
                return Some(deltas);
            }
        }
        self.action_expected_deltas
            .get(&action.kind())
            .map(Vec::as_slice)
    }
}

pub(crate) fn load_control_calibration(workspace: &Path) -> Result<ControlCalibration, StoreError> {
    let report = load_calibration_report(
        workspace,
        CalibrationQuery {
            limit: 0,
            action: None,
        },
    )?;
    Ok(control_calibration_from_report(&report))
}

pub(crate) fn control_calibration_from_report(report: &CalibrationReport) -> ControlCalibration {
    let mut calibration = ControlCalibration::default();
    for action in &report.actions {
        if action.outcome_count < 3 || calibration_skips_action(action.action) {
            continue;
        }
        let penalty = calibration_penalty(
            action.advice_count,
            action.outcome_count,
            action.unresolved_advice_count,
            action.failed,
            action.unknown,
            calibration_underperformance_error(
                &action.expected_entropy_delta,
                &action.observed_entropy_delta,
            ),
        );
        if penalty > 0 {
            calibration.action_penalties.insert(action.action, penalty);
        }
        if let Some(deltas) = calibrated_expected_deltas_from_history(
            action.action,
            action.outcome_count,
            action.succeeded,
            action.failed,
            &action.expected_entropy_delta,
            &action.observed_entropy_delta,
        ) {
            calibration
                .action_expected_deltas
                .insert(action.action, deltas);
        }
    }
    for target in &report.targets {
        if target.outcome_count < 3 || calibration_skips_action(target.action) {
            continue;
        }
        let penalty = calibration_penalty(
            target.advice_count,
            target.outcome_count,
            target.unresolved_advice_count,
            target.failed,
            target.unknown,
            calibration_underperformance_error(
                &target.expected_entropy_delta,
                &target.observed_entropy_delta,
            ),
        );
        if penalty > 0 {
            calibration
                .target_penalties
                .insert((target.action, target.target_agent.clone()), penalty);
        }
        if let Some(deltas) = calibrated_expected_deltas_from_history(
            target.action,
            target.outcome_count,
            target.succeeded,
            target.failed,
            &target.expected_entropy_delta,
            &target.observed_entropy_delta,
        ) {
            calibration
                .target_expected_deltas
                .insert((target.action, target.target_agent.clone()), deltas);
        }
    }
    calibration
}

pub(crate) fn calibrated_expected_deltas_from_history(
    action: ControlActionKind,
    outcome_count: usize,
    succeeded: usize,
    failed: usize,
    expected_entropy_delta: &[EntropyDelta],
    observed_entropy_delta: &[EntropyDelta],
) -> Option<Vec<EntropyDelta>> {
    if outcome_count < 3 || failed > succeeded || calibration_skips_action(action) {
        return None;
    }
    let outcome_count = i32::try_from(outcome_count).ok()?.max(1);
    let expected_by_kind = entropy_delta_map(expected_entropy_delta);
    let deltas = observed_entropy_delta
        .iter()
        .filter_map(|delta| {
            let average = i32::from(delta.delta) / outcome_count;
            if average >= 0 {
                return None;
            }
            let prior = expected_by_kind
                .get(&delta.kind)
                .copied()
                .map(|expected| expected / outcome_count)
                .unwrap_or(average);
            Some(EntropyDelta {
                kind: delta.kind,
                delta: shrink_calibrated_delta(prior, average, outcome_count).clamp(-80, -5) as i16,
            })
        })
        .collect::<Vec<_>>();
    if deltas.is_empty() {
        None
    } else {
        Some(deltas)
    }
}

pub(crate) fn shrink_calibrated_delta(
    prior: i32,
    observed_average: i32,
    outcome_count: i32,
) -> i32 {
    const PRIOR_WEIGHT: i32 = 3;
    rounded_div(
        observed_average * outcome_count + prior * PRIOR_WEIGHT,
        outcome_count + PRIOR_WEIGHT,
    )
}

pub(crate) fn rounded_div(numerator: i32, denominator: i32) -> i32 {
    if denominator == 0 {
        return 0;
    }
    let half = denominator.abs() / 2;
    if numerator >= 0 {
        (numerator + half) / denominator
    } else {
        (numerator - half) / denominator
    }
}

pub(crate) fn calibration_underperformance_error(
    expected: &[EntropyDelta],
    observed: &[EntropyDelta],
) -> i32 {
    let expected = entropy_delta_map(expected);
    let observed = entropy_delta_map(observed);
    expected
        .keys()
        .chain(observed.keys())
        .copied()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(|kind| {
            let expected_delta = expected.get(&kind).copied().unwrap_or_default();
            let observed_delta = observed.get(&kind).copied().unwrap_or_default();
            if expected_delta < 0 {
                (observed_delta - expected_delta).max(0)
            } else {
                observed_delta.max(0)
            }
        })
        .sum()
}

pub(crate) fn entropy_delta_map(deltas: &[EntropyDelta]) -> BTreeMap<EntropyKind, i32> {
    let mut map = BTreeMap::new();
    for delta in deltas {
        *map.entry(delta.kind).or_default() += i32::from(delta.delta);
    }
    map
}

pub(crate) fn calibration_penalty(
    advice_count: usize,
    outcome_count: usize,
    unresolved_advice_count: usize,
    failed: usize,
    unknown: usize,
    underperformance_error: i32,
) -> i32 {
    let outcome_count = outcome_count as i32;
    let average_error = underperformance_error / outcome_count;
    let failure_penalty = failed as i32 * 700 / outcome_count;
    let unknown_penalty = unknown as i32 * 250 / outcome_count;
    let unresolved_penalty = if advice_count == 0 {
        0
    } else {
        unresolved_advice_count as i32 * 200 / advice_count as i32
    };
    (average_error * 20 + failure_penalty + unknown_penalty + unresolved_penalty).min(1500)
}

pub(crate) fn calibration_skips_action(action: ControlActionKind) -> bool {
    matches!(
        action,
        ControlActionKind::ForceVerification
            | ControlActionKind::BlockProgressUntilTraceAndVerification
            | ControlActionKind::Pause
    )
}

pub(crate) fn utility_ranked_control_action(
    case_file: &ControlCaseFile,
    calibration: &ControlCalibration,
) -> Option<ControlAction> {
    let dominant = dominant_entropy(case_file);
    let mut candidates = Vec::new();
    push_control_candidate(
        &mut candidates,
        ControlAction::ContinueWorking,
        case_file,
        dominant,
        calibration,
    );

    if switch_agent_entropy_allowed(case_file) {
        let unhealthy_agent = agent_for_entropy(case_file, EntropyKind::AgentHealth);
        let Some(target_agent) =
            fallback_agent_for_case_file(case_file, unhealthy_agent.as_deref())
        else {
            return Some(ControlAction::Pause {
                reason: "no adapter capability allows a writable handoff target".into(),
            });
        };
        push_control_candidate(
            &mut candidates,
            ControlAction::SwitchAgent { target_agent },
            case_file,
            dominant,
            calibration,
        );
    }

    if spawn_fresh_entropy_allowed(case_file) {
        let target_agent =
            fallback_agent_for_case_file(case_file, None).unwrap_or_else(|| "claude-code".into());
        push_control_candidate(
            &mut candidates,
            ControlAction::SpawnFreshAgent {
                target_agent: Some(target_agent),
            },
            case_file,
            dominant,
            calibration,
        );
    }

    if retry_agent_entropy_allowed(case_file) {
        push_control_candidate(
            &mut candidates,
            ControlAction::RetryAgent {
                target_agent: agent_for_entropy(case_file, EntropyKind::AgentHealth),
                max_attempts: 1,
            },
            case_file,
            dominant,
            calibration,
        );
    }

    if spawn_judge_entropy_allowed(case_file) {
        push_control_candidate(
            &mut candidates,
            ControlAction::SpawnJudgeAgent {
                target_agent: judge_agent_for_case_file(case_file),
            },
            case_file,
            dominant,
            calibration,
        );
    }

    if run_probe_entropy_allowed(case_file) {
        push_control_candidate(
            &mut candidates,
            ControlAction::RunProbe {
                probe: probe_spec_for_case_file(case_file),
            },
            case_file,
            dominant,
            calibration,
        );
    }

    if case_file
        .entropy
        .score(EntropyKind::UserDecision)
        .is_some_and(|score| score.score >= USER_DECISION_ASK_USER_THRESHOLD)
    {
        push_control_candidate(
            &mut candidates,
            ControlAction::AskUser {
                question: ask_user_question_for_case_file(case_file),
            },
            case_file,
            dominant,
            calibration,
        );
    }

    if send_follow_up_entropy_allowed(case_file) {
        push_control_candidate(
            &mut candidates,
            ControlAction::SendFollowUp { target_agent: None },
            case_file,
            dominant,
            calibration,
        );
    }

    candidates
        .into_iter()
        .max_by_key(|candidate| (candidate.utility, candidate.priority))
        .map(|candidate| candidate.action)
}

pub(crate) fn push_control_candidate(
    candidates: &mut Vec<ControlActionCandidate>,
    action: ControlAction,
    case_file: &ControlCaseFile,
    dominant: Option<EntropyKind>,
    calibration: &ControlCalibration,
) {
    candidates.push(ControlActionCandidate {
        utility: control_action_utility(&action, case_file, dominant, calibration),
        priority: control_action_tie_priority(action.kind()),
        action,
    });
}

pub(crate) fn control_action_utility(
    action: &ControlAction,
    case_file: &ControlCaseFile,
    dominant: Option<EntropyKind>,
    calibration: &ControlCalibration,
) -> i32 {
    expected_entropy_reduction_score(action, case_file, dominant, Some(calibration))
        - control_action_cost(action)
        - calibration.penalty_for(action)
}

pub(crate) fn expected_entropy_reduction_score(
    action: &ControlAction,
    case_file: &ControlCaseFile,
    dominant: Option<EntropyKind>,
    calibration: Option<&ControlCalibration>,
) -> i32 {
    calibrated_expected_entropy_delta_for_control_action(action, case_file, dominant, calibration)
        .into_iter()
        .filter(|delta| delta.delta < 0)
        .map(|delta| i32::from(-delta.delta) * entropy_score_points(case_file, delta.kind))
        .sum()
}

pub(crate) fn entropy_score_points(case_file: &ControlCaseFile, kind: EntropyKind) -> i32 {
    case_file
        .entropy
        .score(kind)
        .map(|score| i32::from(score.score))
        .unwrap_or_default()
}

pub(crate) fn control_action_cost(action: &ControlAction) -> i32 {
    match action {
        ControlAction::ContinueWorking => 0,
        ControlAction::BlockProgressUntilTraceAndVerification { .. } => 0,
        ControlAction::RunProbe { .. } => 100,
        ControlAction::SendFollowUp { .. } => 200,
        ControlAction::SpawnJudgeAgent { .. } => 600,
        ControlAction::RetryAgent { .. } => 1_000,
        ControlAction::SwitchAgent { .. } => 1_000,
        ControlAction::AskUser { .. } => 2_000,
        ControlAction::SpawnFreshAgent { .. } => 3_000,
        ControlAction::ForceVerification { .. } => 0,
        ControlAction::Pause { .. } => 1_000_000,
    }
}

pub(crate) fn control_action_tie_priority(kind: ControlActionKind) -> i32 {
    match kind {
        ControlActionKind::SwitchAgent => 70,
        ControlActionKind::SpawnFreshAgent => 60,
        ControlActionKind::AskUser => 50,
        ControlActionKind::RetryAgent => 40,
        ControlActionKind::ForceVerification => 30,
        ControlActionKind::RunProbe => 29,
        ControlActionKind::BlockProgressUntilTraceAndVerification => 28,
        ControlActionKind::SpawnJudgeAgent => 25,
        ControlActionKind::SendFollowUp => 20,
        ControlActionKind::ContinueWorking => 10,
        ControlActionKind::Pause => 0,
    }
}

pub(crate) fn control_rationale_for_action(
    final_action: &ControlAction,
    case_file: &ControlCaseFile,
    calibration: Option<&ControlCalibration>,
) -> ControlRationale {
    let dominant = dominant_entropy(case_file);
    let selected_action = final_action.kind();
    let expected_entropy_delta = calibrated_expected_entropy_delta_for_control_action(
        final_action,
        case_file,
        dominant,
        calibration,
    );
    let evidence_ids = evidence_ids_for_control_rationale(final_action, case_file, dominant);
    let reason = control_rationale_reason(final_action, case_file, dominant, calibration);

    ControlRationale {
        selected_action,
        dominant_entropy: dominant,
        reason,
        expected_entropy_delta,
        evidence_ids,
        requirement_ids: requirement_ids_for_control_rationale(final_action, case_file, dominant),
    }
}

pub(crate) fn requirement_ids_for_control_rationale(
    final_action: &ControlAction,
    case_file: &ControlCaseFile,
    dominant: Option<EntropyKind>,
) -> Vec<String> {
    let mut requirement_ids = Vec::new();
    for requirement in &case_file.requirements {
        if requirement.source != RequirementSource::ProjectContract {
            continue;
        }
        if action_enforces_project_contract_requirement(
            final_action,
            case_file,
            dominant,
            requirement,
        ) {
            push_unique_string(&mut requirement_ids, &requirement.requirement_id);
        }
    }
    requirement_ids
}

pub(crate) fn action_enforces_project_contract_requirement(
    final_action: &ControlAction,
    case_file: &ControlCaseFile,
    dominant: Option<EntropyKind>,
    requirement: &RequirementNode,
) -> bool {
    let text = requirement.text.to_ascii_lowercase();
    match final_action {
        ControlAction::ForceVerification { .. } => {
            project_contract_is_stale_verification_invariant(&text)
                && (dominant == Some(EntropyKind::Verification)
                    || case_file.verification.status != VerificationStatus::Passed)
        }
        _ => false,
    }
}

pub(crate) fn project_contract_is_stale_verification_invariant(text: &str) -> bool {
    text.contains("do not continue after source/test changes")
        && text.contains("verification is stale")
}

pub(crate) fn dominant_entropy(case_file: &ControlCaseFile) -> Option<EntropyKind> {
    case_file
        .entropy
        .scores
        .iter()
        .filter(|score| score.score > 0)
        .max_by_key(|score| score.score)
        .map(|score| score.kind)
}

pub(crate) fn expected_entropy_delta_for_control_action(
    action: &ControlAction,
    case_file: &ControlCaseFile,
    dominant: Option<EntropyKind>,
) -> Vec<EntropyDelta> {
    match action {
        ControlAction::ForceVerification { .. } => vec![
            EntropyDelta {
                kind: EntropyKind::Verification,
                delta: -55,
            },
            EntropyDelta {
                kind: EntropyKind::RepoBlame,
                delta: -15,
            },
            EntropyDelta {
                kind: EntropyKind::Plan,
                delta: -10,
            },
        ],
        ControlAction::BlockProgressUntilTraceAndVerification { .. } => vec![
            EntropyDelta {
                kind: EntropyKind::RepoBlame,
                delta: -45,
            },
            EntropyDelta {
                kind: EntropyKind::Verification,
                delta: -20,
            },
        ],
        ControlAction::RetryAgent { .. } => vec![EntropyDelta {
            kind: EntropyKind::AgentHealth,
            delta: -35,
        }],
        ControlAction::SwitchAgent { .. } => vec![EntropyDelta {
            kind: EntropyKind::AgentHealth,
            delta: -45,
        }],
        ControlAction::SpawnJudgeAgent { .. } => vec![EntropyDelta {
            kind: EntropyKind::RepoBlame,
            delta: -35,
        }],
        ControlAction::RunProbe { .. } => vec![
            EntropyDelta {
                kind: EntropyKind::Plan,
                delta: -35,
            },
            EntropyDelta {
                kind: EntropyKind::RepoBlame,
                delta: -10,
            },
        ],
        ControlAction::SpawnFreshAgent { .. } => {
            let kind = if spawn_fresh_agent_health_target_is_stronger(case_file, dominant) {
                EntropyKind::AgentHealth
            } else {
                EntropyKind::Context
            };
            vec![EntropyDelta { kind, delta: -50 }]
        }
        ControlAction::AskUser { .. } => vec![EntropyDelta {
            kind: EntropyKind::UserDecision,
            delta: -70,
        }],
        ControlAction::SendFollowUp { .. } => {
            let kind = if send_follow_up_repo_blame_target_is_stronger(case_file) {
                EntropyKind::RepoBlame
            } else {
                EntropyKind::Plan
            };
            vec![EntropyDelta { kind, delta: -25 }]
        }
        ControlAction::ContinueWorking | ControlAction::Pause { .. } => Vec::new(),
    }
}

pub(crate) fn calibrated_expected_entropy_delta_for_control_action(
    action: &ControlAction,
    case_file: &ControlCaseFile,
    dominant: Option<EntropyKind>,
    calibration: Option<&ControlCalibration>,
) -> Vec<EntropyDelta> {
    let base = expected_entropy_delta_for_control_action(action, case_file, dominant);
    let Some(calibration) = calibration else {
        return base;
    };
    let Some(adjustment) = calibration.expected_delta_adjustment_for(action) else {
        return base;
    };
    merge_calibrated_expected_deltas(base, adjustment)
}

pub(crate) fn merge_calibrated_expected_deltas(
    base: Vec<EntropyDelta>,
    adjustment: &[EntropyDelta],
) -> Vec<EntropyDelta> {
    let adjustment_by_kind = adjustment
        .iter()
        .map(|delta| (delta.kind, delta.delta))
        .collect::<BTreeMap<_, _>>();
    base.into_iter()
        .map(|delta| EntropyDelta {
            kind: delta.kind,
            delta: adjustment_by_kind
                .get(&delta.kind)
                .copied()
                .unwrap_or(delta.delta),
        })
        .collect()
}

pub(crate) fn spawn_fresh_agent_health_target_is_stronger(
    case_file: &ControlCaseFile,
    dominant: Option<EntropyKind>,
) -> bool {
    let agent_health = entropy_score_points(case_file, EntropyKind::AgentHealth);
    let context = entropy_score_points(case_file, EntropyKind::Context);
    agent_health >= i32::from(SPAWN_FRESH_AGENT_HEALTH_THRESHOLD)
        && (matches!(dominant, Some(EntropyKind::AgentHealth)) || agent_health >= context)
}

pub(crate) fn send_follow_up_repo_blame_target_is_stronger(case_file: &ControlCaseFile) -> bool {
    let repo_blame = entropy_score_points(case_file, EntropyKind::RepoBlame);
    let plan = entropy_score_points(case_file, EntropyKind::Plan);
    repo_blame >= i32::from(FOLLOW_UP_REPO_BLAME_THRESHOLD)
        && (plan < i32::from(FOLLOW_UP_PLAN_THRESHOLD) || repo_blame >= plan)
}

pub(crate) fn evidence_ids_for_control_rationale(
    action: &ControlAction,
    case_file: &ControlCaseFile,
    dominant: Option<EntropyKind>,
) -> Vec<String> {
    let mut ids = Vec::new();
    let primary_kind = match action {
        ControlAction::ForceVerification { .. } => Some(EntropyKind::Verification),
        ControlAction::BlockProgressUntilTraceAndVerification { .. } => {
            Some(EntropyKind::RepoBlame)
        }
        ControlAction::RunProbe { .. } => Some(EntropyKind::Plan),
        ControlAction::RetryAgent { .. } | ControlAction::SwitchAgent { .. } => {
            Some(EntropyKind::AgentHealth)
        }
        ControlAction::SpawnJudgeAgent { .. } => Some(EntropyKind::RepoBlame),
        ControlAction::SpawnFreshAgent { .. } => Some(dominant.unwrap_or(EntropyKind::Context)),
        ControlAction::AskUser { .. } => Some(EntropyKind::UserDecision),
        ControlAction::SendFollowUp { .. } => Some(dominant.unwrap_or(EntropyKind::Plan)),
        ControlAction::ContinueWorking | ControlAction::Pause { .. } => dominant,
    };

    if let Some(kind) = primary_kind
        && let Some(score) = case_file.entropy.score(kind)
    {
        ids.extend(score.evidence_ids.iter().cloned());
    }
    if ids.is_empty()
        && let Some(kind) = dominant
        && let Some(score) = case_file.entropy.score(kind)
    {
        ids.extend(score.evidence_ids.iter().cloned());
    }
    ids.sort();
    ids.dedup();
    ids
}

pub(crate) fn control_rationale_reason(
    action: &ControlAction,
    case_file: &ControlCaseFile,
    dominant: Option<EntropyKind>,
    calibration: Option<&ControlCalibration>,
) -> String {
    let action_label = control_action_kind_label(action.kind());
    let expected_reduction =
        expected_entropy_reduction_score(action, case_file, dominant, calibration);
    let action_cost = control_action_cost(action);
    let calibration_suffix = control_calibration_rationale_suffix(calibration, action);
    if let ControlAction::ForceVerification { .. } = action
        && let Some(score) = case_file.entropy.score(EntropyKind::Verification)
    {
        return format!(
            "selected {action_label} because verification entropy score {} triggers a hard policy; expected entropy reduction {expected_reduction} before action cost {action_cost}{calibration_suffix}",
            score.score,
        );
    }
    if let ControlAction::BlockProgressUntilTraceAndVerification { .. } = action
        && let Some(score) = case_file.entropy.score(EntropyKind::RepoBlame)
    {
        return format!(
            "selected {action_label} because repo/blame entropy score {} requires trace rationale and verification before more progress; expected entropy reduction {expected_reduction} before action cost {action_cost}{calibration_suffix}",
            score.score,
        );
    }

    match dominant.and_then(|kind| case_file.entropy.score(kind).map(|score| (kind, score))) {
        Some((kind, score)) => format!(
            "selected {action_label} by expected entropy reduction {expected_reduction} minus action cost {action_cost}{calibration_suffix}; dominant {} entropy score {}",
            entropy_kind_label(kind),
            score.score
        ),
        None => format!(
            "selected {action_label} by expected entropy reduction {expected_reduction} minus action cost {action_cost}{calibration_suffix}; no blocking entropy exceeded policy thresholds"
        ),
    }
}

pub(crate) fn control_calibration_rationale_suffix(
    calibration: Option<&ControlCalibration>,
    selected: &ControlAction,
) -> String {
    let Some(calibration) = calibration else {
        return String::new();
    };
    let selected_delta_adjustment = calibration.expected_delta_adjustment_for(selected);
    if !calibration.has_penalties() && selected_delta_adjustment.is_none() {
        return String::new();
    }
    let mut clauses = Vec::new();
    let selected_penalty = calibration.penalty_for(selected);
    if calibration.has_penalties() {
        let mut penalized = calibration
            .action_penalties
            .iter()
            .filter(|(_, penalty)| **penalty > 0)
            .map(|(action, penalty)| format!("{}={penalty}", control_action_kind_label(*action)))
            .collect::<Vec<_>>();
        penalized.extend(
            calibration
                .target_penalties
                .iter()
                .filter(|(_, penalty)| **penalty > 0)
                .map(|((action, target), penalty)| {
                    format!(
                        "{}@{}={penalty}",
                        control_action_kind_label(*action),
                        target
                    )
                }),
        );
        penalized.sort();
        clauses.push(format!(
            "minus calibration penalty {selected_penalty}; calibrated penalties: {}",
            penalized.join(", ")
        ));
    }
    if let Some(deltas) = selected_delta_adjustment {
        clauses.push(format!(
            "calibrated expected deltas: {}",
            calibrated_delta_description(deltas)
        ));
    }
    format!(" {}", clauses.join("; "))
}

pub(crate) fn calibrated_delta_description(deltas: &[EntropyDelta]) -> String {
    let mut descriptions = deltas
        .iter()
        .map(|delta| format!("{}={}", entropy_kind_label(delta.kind), delta.delta))
        .collect::<Vec<_>>();
    descriptions.sort();
    descriptions.join(", ")
}

pub(crate) fn entropy_kind_label(kind: EntropyKind) -> &'static str {
    match kind {
        EntropyKind::Goal => "goal",
        EntropyKind::Context => "context",
        EntropyKind::RepoBlame => "repo/blame",
        EntropyKind::Verification => "verification",
        EntropyKind::Plan => "plan",
        EntropyKind::AgentHealth => "agent-health",
        EntropyKind::UserDecision => "user-decision",
    }
}

pub(crate) fn ask_user_question_for_case_file(case_file: &ControlCaseFile) -> String {
    let cause = case_file
        .entropy
        .score(EntropyKind::UserDecision)
        .and_then(|score| score.top_causes.first())
        .map(String::as_str)
        .unwrap_or("a user-authority decision is blocking safe progress");
    format!(
        "User authorization is required before continuing: {cause}. Provide the required decision or input."
    )
}

pub(crate) fn agent_for_entropy(case_file: &ControlCaseFile, kind: EntropyKind) -> Option<String> {
    let score = case_file.entropy.score(kind)?;
    let evidence_agent = score.evidence_ids.iter().rev().find_map(|evidence_id| {
        case_file
            .evidence
            .iter()
            .find(|item| item.id == *evidence_id)
            .and_then(|item| item.agent.clone())
    });
    evidence_agent.or_else(|| {
        score.top_causes.iter().rev().find_map(|cause| {
            case_file
                .active_agents
                .iter()
                .find(|agent| cause.starts_with(&format!("{agent} ")))
                .cloned()
        })
    })
}

pub(crate) fn fallback_agent_for_case_file(
    case_file: &ControlCaseFile,
    failed_agent: Option<&str>,
) -> Option<String> {
    ["claude-code", "opencode", "codex", "pi"]
        .into_iter()
        .find(|agent| {
            Some(*agent) != failed_agent && adapter_can_receive_writable_handoff(case_file, agent)
        })
        .map(str::to_string)
}

pub(crate) fn judge_agent_for_case_file(case_file: &ControlCaseFile) -> Option<String> {
    ["claude-code", "opencode", "codex", "pi"]
        .into_iter()
        .find(|agent| adapter_can_receive_readonly_judge(case_file, agent))
        .map(str::to_string)
}

pub(crate) fn adapter_can_receive_writable_handoff(
    case_file: &ControlCaseFile,
    agent: &str,
) -> bool {
    adapter_capabilities_for_case_file(case_file, agent)
        .is_some_and(adapter_capability_allows_writable_handoff)
}

pub(crate) fn adapter_can_receive_readonly_judge(case_file: &ControlCaseFile, agent: &str) -> bool {
    adapter_capabilities_for_case_file(case_file, agent)
        .is_some_and(adapter_capability_allows_readonly_judge)
}

pub(crate) fn adapter_capability_allows_writable_handoff(
    capabilities: &AdapterCapabilities,
) -> bool {
    capabilities.enabled
        && capabilities.supports_workspace_write_mode
        && !capabilities.requires_external_sandbox
}

pub(crate) fn adapter_capability_allows_readonly_judge(capabilities: &AdapterCapabilities) -> bool {
    capabilities.enabled && capabilities.can_inject_context && capabilities.supports_readonly_mode
}

pub(crate) fn adapter_capabilities_for_case_file<'a>(
    case_file: &'a ControlCaseFile,
    agent: &str,
) -> Option<&'a AdapterCapabilities> {
    AgentKind::from_str(agent)
        .ok()
        .and_then(|kind| case_file.adapter_capabilities.get(agent_kind_label(kind)))
}

pub(crate) fn unsafe_writable_handoff_reason(case_file: &ControlCaseFile, agent: &str) -> String {
    match adapter_capabilities_for_case_file(case_file, agent) {
        Some(capabilities) if !capabilities.enabled => {
            format!("adapter capabilities mark {agent} as disabled")
        }
        Some(capabilities) if capabilities.requires_external_sandbox => format!(
            "adapter capabilities require an external sandbox before writable handoff to {agent}"
        ),
        Some(capabilities) if !capabilities.supports_workspace_write_mode => {
            format!("adapter capabilities do not support workspace-write mode for {agent}")
        }
        Some(_) => format!("adapter capabilities do not allow writable handoff to {agent}"),
        None => format!("adapter capabilities are unknown for {agent}"),
    }
}

pub(crate) fn control_packet_for_action(
    action: &ControlAction,
    case_file: &ControlCaseFile,
) -> ControlPacket {
    let target_agent = target_agent_for_action(action, case_file);
    let (urgency, title, summary, instructions) = match action {
        ControlAction::ForceVerification { .. } => {
            let mut instructions = if case_file.verification.recommended_commands.is_empty() {
                vec![PacketInstruction {
                    priority: PacketInstructionPriority::Must,
                    text: "Run the smallest verifier that covers the changed behavior before making more code edits."
                        .into(),
                }]
            } else {
                case_file
                    .verification
                    .recommended_commands
                    .iter()
                    .map(|command| PacketInstruction {
                        priority: PacketInstructionPriority::Must,
                        text: format!("Run `{command}` before making more code edits."),
                    })
                    .collect()
            };
            if let Some(failure_class) = case_file.verification.latest_failure_class {
                let mut text = format!(
                    "Latest verifier failure class: {}.",
                    verification_failure_class_label(failure_class)
                );
                if let Some(guidance) = verification_failure_class_packet_guidance(failure_class) {
                    text.push(' ');
                    text.push_str(guidance);
                }
                instructions.push(PacketInstruction {
                    priority: PacketInstructionPriority::Must,
                    text,
                });
            }
            if let Some(instruction) = acceptance_coverage_packet_instruction(&case_file.verification)
            {
                instructions.push(instruction);
            }
            if verification_entropy_mentions_completion_claim(case_file) {
                instructions.push(PacketInstruction {
                    priority: PacketInstructionPriority::Must,
                    text: "Do not claim completion again until a passing verifier result is recorded after the completion claim.".into(),
                });
            }
            if verification_entropy_mentions_unresolved_subagents(case_file) {
                instructions.push(PacketInstruction {
                    priority: PacketInstructionPriority::Must,
                    text: "Do not claim completion until all spawned workers have terminal outcomes such as joined_with_summary, cancelled_with_reason, timed_out, superseded, or failed.".into(),
                });
            }
            if verification_entropy_mentions_intended_environment_validation(case_file) {
                instructions.push(intended_environment_validation_packet_instruction(case_file));
            }
            if verification_entropy_mentions_test_oracle_authority(case_file) {
                instructions.push(PacketInstruction {
                    priority: PacketInstructionPriority::Must,
                    text: "Do not treat green tests as closure after a test oracle change. Cite spec authority and collect independent behavior evidence for changed expectations, assertions, snapshots, fixtures, skips, or deletions.".into(),
                });
            }
            if verification_entropy_mentions_repeated_failure_signature(case_file) {
                instructions.push(PacketInstruction {
                    priority: PacketInstructionPriority::Must,
                    text: "Stop editing until you state a failure hypothesis or run an isolation probe: the same verifier failure signature has recurred after edits.".into(),
                });
            }
            if plan_entropy_mentions_bug_fix_pre_edit_gap(case_file) {
                instructions.push(PacketInstruction {
                    priority: PacketInstructionPriority::Must,
                    text: "For this bug-fix task, reproduce or localize the failure with a verifier, repo/log/runtime probe, or explicit diagnosis before making more edits.".into(),
                });
            }
            instructions.push(PacketInstruction {
                priority: PacketInstructionPriority::Must,
                text: "Record verifier command/result. If it fails, classify the failure and edit only after the likely cause is named.".into(),
            });
            if case_file
                .entropy
                .score(EntropyKind::RepoBlame)
                .is_some_and(|score| score.score >= 75)
            {
                instructions.push(PacketInstruction {
                    priority: PacketInstructionPriority::Must,
                    text: "Before claiming completion, record trace rationale for every dirty hunk or revert unjustified changes.".into(),
                });
            }
            (
                PacketUrgency::Urgent,
                "Verification required".to_string(),
                verification_packet_summary(case_file),
                instructions,
            )
        }
        ControlAction::BlockProgressUntilTraceAndVerification { reason } => (
            PacketUrgency::Urgent,
            "Trace and verification required".to_string(),
            format!(
                "Dirty changes cannot safely continue until trace rationale and verification evidence are repaired: {reason}."
            ),
            vec![
                PacketInstruction {
                    priority: PacketInstructionPriority::Must,
                    text: "Do not make additional code edits until the cited dirty change is traceable or reverted.".into(),
                },
                PacketInstruction {
                    priority: PacketInstructionPriority::Must,
                    text: "Record trace rationale linked to the relevant user request, design decision, failing verifier, or recovery action for every dirty hunk that should stay.".into(),
                },
                PacketInstruction {
                    priority: PacketInstructionPriority::Must,
                    text: "After trace repair or revert, run the relevant verifier before claiming completion.".into(),
                },
            ],
        ),
        ControlAction::RetryAgent { max_attempts, .. } => (
            PacketUrgency::Urgent,
            "Loop-breaking retry required".to_string(),
            "A repeated failure pattern requires one changed recovery step.".to_string(),
            vec![
                PacketInstruction {
                    priority: PacketInstructionPriority::Must,
                    text: "Do not repeat the same failing command or tool call until you have changed the diagnosis or inputs.".into(),
                },
                PacketInstruction {
                    priority: PacketInstructionPriority::Must,
                    text: "State the loop signature, inspect the cited evidence, and take one different recovery step.".into(),
                },
                PacketInstruction {
                    priority: PacketInstructionPriority::Should,
                    text: "If the retry still fails, stop and let the monitor switch or spawn a fresh agent.".into(),
                },
                PacketInstruction {
                    priority: PacketInstructionPriority::Must,
                    text: format!("Retry at most {max_attempts} time(s) under this packet."),
                },
            ],
        ),
        ControlAction::RunProbe { probe } => (
            PacketUrgency::FollowUp,
            "Local probe required".to_string(),
            "A monitor-owned probe must record local evidence before user interruption, retry, or handoff.".to_string(),
            run_probe_packet_instructions(probe),
        ),
        ControlAction::SpawnFreshAgent { .. } => (
            PacketUrgency::Context,
            "Fresh agent handoff required".to_string(),
            "Take over from the bounded case file because the current session context is unreliable.".to_string(),
            vec![PacketInstruction {
                priority: PacketInstructionPriority::Must,
                text: "State the current goal, active memory constraints, recent trace, and verification state before editing.".into(),
            }],
        ),
        ControlAction::SpawnJudgeAgent { .. } => (
            PacketUrgency::Context,
            "Read-only judge review required".to_string(),
            "Suspicious or insufficiently traced changes need read-only review.".to_string(),
            vec![
                PacketInstruction {
                    priority: PacketInstructionPriority::Must,
                    text: "Act as a read-only judge: inspect the cited repo/blame evidence, dirty hunks, trace rationale, and verification status.".into(),
                },
                PacketInstruction {
                    priority: PacketInstructionPriority::Must,
                    text: "Do not edit files, run fix-up commands, or mutate the worktree.".into(),
                },
                PacketInstruction {
                    priority: PacketInstructionPriority::Must,
                    text: "Return whether each suspicious change should stay, be reverted, receive trace rationale, or be sent back for implementation repair.".into(),
                },
            ],
        ),
        ControlAction::SendFollowUp { .. } => (
            PacketUrgency::FollowUp,
            "Continue with bounded next step".to_string(),
            if case_file
                .entropy
                .score(EntropyKind::RepoBlame)
                .is_some_and(|score| score.score >= 75)
            {
                "Dirty hunks lack complete trace rationale.".to_string()
            } else {
                "A bounded next action can move the task without a user decision.".to_string()
            },
            if context_entropy_mentions_rejected_alternative(case_file) {
                vec![
                    PacketInstruction {
                        priority: PacketInstructionPriority::Must,
                        text: "Do not implement the rejected alternative unless the user explicitly authorizes reopening that decision.".into(),
                    },
                    PacketInstruction {
                        priority: PacketInstructionPriority::Must,
                        text: "Revise the plan to honor rejected-alternative memory and cite the replacement approach.".into(),
                    },
                ]
            } else if plan_entropy_mentions_subagent_wip_cap(case_file) {
                vec![
                    PacketInstruction {
                        priority: PacketInstructionPriority::Must,
                        text: "Do not spawn additional subagents: the subagent WIP cap is reached.".into(),
                    },
                    PacketInstruction {
                        priority: PacketInstructionPriority::Must,
                        text: "Join or cancel spawned workers and integrate terminal summaries before starting more fan-out.".into(),
                    },
                ]
            } else if plan_entropy_mentions_overlapping_subagent_path_ownership(case_file) {
                vec![
                    PacketInstruction {
                        priority: PacketInstructionPriority::Must,
                        text: "Do not spawn more subagents: overlapping subagent path ownership is unresolved.".into(),
                    },
                    PacketInstruction {
                        priority: PacketInstructionPriority::Must,
                        text: "Join, cancel, or reassign current workers so each active worker has disjoint worker paths before more fan-out.".into(),
                    },
                ]
            } else if plan_entropy_mentions_routine_agent_question(case_file) {
                vec![
                    PacketInstruction {
                        priority: PacketInstructionPriority::Must,
                        text: "Do not ask the user for routine sequencing, file choice, testing, or debugging decisions.".into(),
                    },
                    PacketInstruction {
                        priority: PacketInstructionPriority::Must,
                        text: "Run the cheapest local probe or take the obvious next step, then report the evidence.".into(),
                    },
                ]
            } else if plan_entropy_mentions_bug_fix_pre_edit_gap(case_file) {
                vec![PacketInstruction {
                    priority: PacketInstructionPriority::Must,
                    text: "For this bug-fix task, reproduce or localize the failure with a verifier, repo/log/runtime probe, or explicit diagnosis before making more edits.".into(),
                }]
            } else if case_file
                .entropy
                .score(EntropyKind::RepoBlame)
                .is_some_and(|score| score.score >= 75)
            {
                vec![
                    PacketInstruction {
                        priority: PacketInstructionPriority::Must,
                        text: "Record trace rationale for every dirty hunk, or revert unjustified changes before continuing.".into(),
                    },
                    PacketInstruction {
                        priority: PacketInstructionPriority::Must,
                        text: "Do not add new code changes until the current dirty hunks are traceable.".into(),
                    },
                ]
            } else {
                vec![PacketInstruction {
                    priority: PacketInstructionPriority::Must,
                    text: "Inspect the cited evidence, take one implementation step that advances the current goal, and do not ask the user unless a hard user decision is required.".into(),
                }]
            },
        ),
        ControlAction::ContinueWorking => (
            PacketUrgency::FollowUp,
            "Continue working".to_string(),
            "No monitor gate is blocking the current task.".to_string(),
            vec![PacketInstruction {
                priority: PacketInstructionPriority::Should,
                text: "Continue the current task, keep verification current, and report the next objective result.".into(),
            }],
        ),
        ControlAction::SwitchAgent { .. } => (
            PacketUrgency::Context,
            "Switch agent".to_string(),
            "Take over because the current session should not keep control.".to_string(),
            vec![PacketInstruction {
                priority: PacketInstructionPriority::Must,
                text: "State the current goal, active memory constraints, recent trace, and verification state before editing.".into(),
            }],
        ),
        ControlAction::AskUser { question } => (
            PacketUrgency::Urgent,
            "User decision required".to_string(),
            question.clone(),
            vec![PacketInstruction {
                priority: PacketInstructionPriority::Must,
                text: "Ask exactly the bounded user question and wait for the answer.".into(),
            }],
        ),
        ControlAction::Pause { reason } => (
            PacketUrgency::Urgent,
            "Monitor paused".to_string(),
            reason.clone(),
            vec![PacketInstruction {
                priority: PacketInstructionPriority::Must,
                text: "Do not continue until the monitor receives a valid next action.".into(),
            }],
        ),
    };

    let evidence_refs = case_file
        .entropy
        .scores
        .iter()
        .flat_map(|score| score.evidence_ids.iter().cloned())
        .collect::<Vec<_>>();

    let preconditions = packet_preconditions_for_case_file(case_file, &target_agent);
    let mut forbidden = vec![
        "Do not ask the user whether to continue obvious work.".into(),
        "Do not edit unrelated files.".into(),
    ];
    if matches!(
        action,
        ControlAction::BlockProgressUntilTraceAndVerification { .. }
    ) {
        forbidden
            .push("Do not make unrelated edits while trace/verification repair is pending.".into());
    }
    if matches!(action, ControlAction::SpawnJudgeAgent { .. }) {
        forbidden.push("Do not edit files or mutate the worktree during judge review.".into());
        forbidden.push("Do not run destructive commands or apply patches.".into());
    }
    if matches!(action, ControlAction::RunProbe { .. }) {
        forbidden.push("Do not make broad code edits until the probe result is recorded.".into());
    }

    let success_criteria = if matches!(action, ControlAction::RunProbe { .. }) {
        vec![
            "Probe command, observation, or inspection result is recorded.".into(),
            "The next bounded action is justified by that evidence.".into(),
        ]
    } else if matches!(
        action,
        ControlAction::SpawnFreshAgent { .. } | ControlAction::SwitchAgent { .. }
    ) {
        vec![
            "Receiving agent states current goal, memory constraints, recent trace, verification state, and next action.".into(),
            "Any new file change has trace rationale and a verification plan.".into(),
        ]
    } else if matches!(
        action,
        ControlAction::BlockProgressUntilTraceAndVerification { .. }
    ) {
        vec![
            "Every dirty hunk that remains has trace rationale.".into(),
            "Unjustified dirty hunks are reverted or quarantined.".into(),
            "Relevant verification is run after trace repair.".into(),
        ]
    } else {
        vec!["Required action is completed with recorded evidence or a concrete blocker.".into()]
    };

    ControlPacket {
        packet_id: format!("packet-{}", current_id_fragment()),
        target_agent,
        urgency,
        title,
        summary,
        instructions,
        evidence_refs,
        forbidden,
        success_criteria,
        preconditions,
    }
}

pub(crate) fn verification_packet_summary(case_file: &ControlCaseFile) -> String {
    if verification_entropy_mentions_completion_claim(case_file) {
        "The monitor found an agent completion claim without objective verification evidence."
            .into()
    } else {
        "Verification evidence is missing, stale, or failed for the current work.".into()
    }
}

pub(crate) fn run_probe_packet_instructions(probe: &ProbeSpec) -> Vec<PacketInstruction> {
    let probe_text = match probe {
        ProbeSpec::LocalEvidence {
            target: Some(target),
        } if target == "routine_next_step" => {
            "The monitor will run a monitor-owned local evidence probe for the routine sequencing question.".to_string()
        }
        ProbeSpec::LocalEvidence {
            target: Some(target),
        } if target == "bug_reproduction_or_localization" => {
            "The monitor will record bug reproduction or localization evidence before more edits proceed.".to_string()
        }
        ProbeSpec::LocalEvidence { target: Some(target) } => {
            format!("The monitor will run a monitor-owned local evidence probe for `{target}` before asking the user or broadening the task.")
        }
        ProbeSpec::LocalEvidence { target: None } => {
            "The monitor will run a monitor-owned local evidence probe before asking the user or broadening the task.".into()
        }
        ProbeSpec::RuntimeValidation { surface, target } => target
            .as_deref()
            .map(|target| {
                format!(
                    "{} intended-environment evidence for `{target}` must be recorded by the monitor, using {}.",
                    surface.label(),
                    surface.evidence_phrase()
                )
            })
            .unwrap_or_else(|| {
                format!(
                    "{} intended-environment evidence must be recorded by the monitor, using {}.",
                    surface.label(),
                    surface.evidence_phrase()
                )
            }),
        ProbeSpec::BrowserValidation { target } => target
            .as_deref()
            .map(|target| format!("Legacy browser_validation probe for `{target}` is browser-only. Prefer runtime_validation with surface=web_ui or another affected runtime surface."))
            .unwrap_or_else(|| "Legacy browser_validation probe is browser-only. Prefer runtime_validation with surface=web_ui or another affected runtime surface.".into()),
        ProbeSpec::RepoInspection { target } => target
            .as_deref()
            .map(|target| format!("The monitor will run repo inspection for `{target}` and record the result."))
            .unwrap_or_else(|| "The monitor will run repo inspection and record the result.".into()),
        ProbeSpec::TargetedTest { command } => {
            format!("The monitor will run the configured targeted verifier exactly as selected: `{command}`.")
        }
    };
    vec![
        PacketInstruction {
            priority: PacketInstructionPriority::Must,
            text: "Do not ask the user for sequencing, file choice, testing, or debugging decisions while local evidence can answer.".into(),
        },
        PacketInstruction {
            priority: PacketInstructionPriority::Must,
            text: probe_text,
        },
        PacketInstruction {
            priority: PacketInstructionPriority::Must,
            text: "Wait for the recorded probe result before broad code edits; use that result to justify the next bounded action.".into(),
        },
    ]
}

pub(crate) fn verification_entropy_mentions_completion_claim(case_file: &ControlCaseFile) -> bool {
    case_file
        .entropy
        .score(EntropyKind::Verification)
        .is_some_and(|score| {
            score.top_causes.iter().any(|cause| {
                let cause = cause.to_ascii_lowercase();
                cause.contains("completion") && cause.contains("verification")
            })
        })
}

pub(crate) fn verification_entropy_mentions_unresolved_subagents(
    case_file: &ControlCaseFile,
) -> bool {
    case_file
        .entropy
        .score(EntropyKind::Verification)
        .is_some_and(|score| {
            score.top_causes.iter().any(|cause| {
                let cause = cause.to_ascii_lowercase();
                cause.contains("completion") && cause.contains("spawned worker")
            })
        })
}

pub(crate) fn verification_entropy_mentions_intended_environment_validation(
    case_file: &ControlCaseFile,
) -> bool {
    case_file
        .entropy
        .score(EntropyKind::Verification)
        .is_some_and(|score| {
            score.top_causes.iter().any(|cause| {
                let cause = cause.to_ascii_lowercase();
                cause.contains("intended-environment validation")
            })
        })
}

pub(crate) fn intended_environment_validation_packet_instruction(
    case_file: &ControlCaseFile,
) -> PacketInstruction {
    let surfaces = intended_environment_validation_surfaces(case_file);
    let text = match surfaces.as_slice() {
        [] => "For the affected runtime surface, record intended-environment validation with platform-appropriate runtime or e2e evidence. A build alone does not close this obligation.".to_string(),
        [surface] => format!(
            "For the {}, record intended-environment validation with {}. A build alone does not close this obligation.",
            surface.change_label(),
            surface.packet_evidence_phrase()
        ),
        _ => {
            let details = surfaces
                .iter()
                .map(|surface| {
                    format!(
                        "{}: {}",
                        surface.change_label(),
                        surface.packet_evidence_phrase()
                    )
                })
                .collect::<Vec<_>>()
                .join("; ");
            format!(
                "For each affected runtime surface, record intended-environment validation ({details}). A build alone does not close this obligation."
            )
        }
    };
    PacketInstruction {
        priority: PacketInstructionPriority::Must,
        text,
    }
}

pub(crate) fn intended_environment_validation_surfaces(
    case_file: &ControlCaseFile,
) -> Vec<ValidationSurface> {
    let mut surfaces = Vec::new();
    if let Some(score) = case_file.entropy.score(EntropyKind::Verification) {
        for surface in ordered_validation_surfaces() {
            if score.top_causes.iter().any(|cause| {
                let cause = cause.to_ascii_lowercase();
                cause.contains(&surface.change_label().to_ascii_lowercase())
                    && cause.contains("intended-environment validation")
            }) {
                push_validation_surface(&mut surfaces, surface);
            }
        }
    }
    surfaces
}

pub(crate) fn verification_entropy_mentions_test_oracle_authority(
    case_file: &ControlCaseFile,
) -> bool {
    case_file
        .entropy
        .score(EntropyKind::Verification)
        .is_some_and(|score| {
            score.top_causes.iter().any(|cause| {
                let cause = cause.to_ascii_lowercase();
                cause.contains("test oracle") && cause.contains("authority")
            })
        })
}

pub(crate) fn verification_entropy_mentions_repeated_failure_signature(
    case_file: &ControlCaseFile,
) -> bool {
    case_file
        .entropy
        .score(EntropyKind::Verification)
        .is_some_and(|score| {
            score.top_causes.iter().any(|cause| {
                let cause = cause.to_ascii_lowercase();
                cause.contains("same verifier failure signature")
                    && cause.contains("without a new hypothesis")
            })
        })
}

pub(crate) fn plan_entropy_mentions_subagent_wip_cap(case_file: &ControlCaseFile) -> bool {
    case_file
        .entropy
        .score(EntropyKind::Plan)
        .is_some_and(|score| {
            score
                .top_causes
                .iter()
                .any(|cause| cause.to_ascii_lowercase().contains("subagent wip cap"))
        })
}

pub(crate) fn plan_entropy_mentions_overlapping_subagent_path_ownership(
    case_file: &ControlCaseFile,
) -> bool {
    case_file
        .entropy
        .score(EntropyKind::Plan)
        .is_some_and(|score| {
            score.top_causes.iter().any(|cause| {
                cause
                    .to_ascii_lowercase()
                    .contains("overlapping subagent path ownership")
            })
        })
}

pub(crate) fn plan_entropy_mentions_routine_agent_question(case_file: &ControlCaseFile) -> bool {
    case_file
        .entropy
        .score(EntropyKind::Plan)
        .is_some_and(|score| {
            score.top_causes.iter().any(|cause| {
                let cause = cause.to_ascii_lowercase();
                cause.contains("routine user question") && cause.contains("local probes")
            })
        })
}

pub(crate) fn plan_entropy_mentions_bug_fix_pre_edit_gap(case_file: &ControlCaseFile) -> bool {
    case_file
        .entropy
        .score(EntropyKind::Plan)
        .is_some_and(|score| {
            score.top_causes.iter().any(|cause| {
                let cause = cause.to_ascii_lowercase();
                cause.contains("bug-fix edit")
                    && cause.contains("reproduction or localization evidence")
            })
        })
}

pub(crate) fn plan_entropy_mentions_repeated_inspection_loop(case_file: &ControlCaseFile) -> bool {
    case_file
        .entropy
        .score(EntropyKind::Plan)
        .is_some_and(|score| {
            score
                .top_causes
                .iter()
                .any(|cause| cause.to_ascii_lowercase().contains("repeatedly inspected"))
        })
}

pub(crate) fn context_entropy_mentions_rejected_alternative(case_file: &ControlCaseFile) -> bool {
    case_file
        .entropy
        .score(EntropyKind::Context)
        .is_some_and(|score| {
            score
                .top_causes
                .iter()
                .any(|cause| cause.to_ascii_lowercase().contains("rejected alternative"))
        })
}

pub(crate) fn target_agent_for_action(
    action: &ControlAction,
    case_file: &ControlCaseFile,
) -> String {
    match action {
        ControlAction::RetryAgent {
            target_agent: Some(agent),
            ..
        }
        | ControlAction::SpawnFreshAgent {
            target_agent: Some(agent),
        }
        | ControlAction::SendFollowUp {
            target_agent: Some(agent),
        } => agent.clone(),
        ControlAction::SpawnFreshAgent { target_agent: None }
        | ControlAction::SpawnJudgeAgent { target_agent: None }
        | ControlAction::RetryAgent {
            target_agent: None, ..
        }
        | ControlAction::SendFollowUp { target_agent: None } => case_file
            .active_agents
            .first()
            .cloned()
            .unwrap_or_else(|| "codex".into()),
        ControlAction::SpawnJudgeAgent {
            target_agent: Some(agent),
        } => agent.clone(),
        ControlAction::SwitchAgent { target_agent } => target_agent.clone(),
        _ => case_file
            .active_agents
            .first()
            .cloned()
            .unwrap_or_else(|| "codex".into()),
    }
}

pub(crate) fn acceptance_coverage_packet_instruction(
    verification: &VerificationSummary,
) -> Option<PacketInstruction> {
    let coverage = verification
        .acceptance_coverage
        .iter()
        .find(|coverage| coverage.status != AcceptanceCoverageStatus::Covered)?;
    let label = match coverage.status {
        AcceptanceCoverageStatus::Covered => return None,
        AcceptanceCoverageStatus::Stale => "Stale acceptance criterion",
        AcceptanceCoverageStatus::Failed => "Failed acceptance criterion",
        AcceptanceCoverageStatus::Unverified => "Unverified acceptance criterion",
        AcceptanceCoverageStatus::Unmapped => "Unmapped acceptance criterion",
    };
    let suffix = match coverage.status {
        AcceptanceCoverageStatus::Unmapped => {
            "Add or identify a mapped verifier before claiming completion."
        }
        AcceptanceCoverageStatus::Unverified => {
            "Run the mapped verifier before claiming completion."
        }
        AcceptanceCoverageStatus::Stale => {
            "Rerun the mapped verifier because later changes made it stale."
        }
        AcceptanceCoverageStatus::Failed => "Fix the failing mapped verifier before continuing.",
        AcceptanceCoverageStatus::Covered => return None,
    };
    Some(PacketInstruction {
        priority: PacketInstructionPriority::Must,
        text: format!("{label}: {}. {suffix}", coverage.criterion),
    })
}

pub(crate) fn handoff_packet_for_agent(
    target_agent: AgentKind,
    case_file: &ControlCaseFile,
) -> ControlPacket {
    let target_agent_label = agent_kind_label(target_agent).to_string();
    let mut packet = control_packet_for_action(
        &ControlAction::SpawnFreshAgent {
            target_agent: Some(target_agent_label),
        },
        case_file,
    );
    packet.title = "Fresh agent handoff".into();
    packet.summary = format!(
        "Fresh agent handoff. Current verification status: {}. Use bounded evidence before editing.",
        verification_status_label(case_file.verification.status)
    );
    packet.instructions = vec![
        PacketInstruction {
            priority: PacketInstructionPriority::Must,
            text: "Read this packet, case file evidence, belief state, durable design memory, and recent trace before editing.".into(),
        },
        PacketInstruction {
            priority: PacketInstructionPriority::Must,
            text: belief_instruction_text(case_file),
        },
        PacketInstruction {
            priority: PacketInstructionPriority::Must,
            text: verification_instruction_text(case_file),
        },
        PacketInstruction {
            priority: PacketInstructionPriority::Should,
            text: memory_instruction_text(case_file),
        },
        PacketInstruction {
            priority: PacketInstructionPriority::Should,
            text: trace_instruction_text(case_file),
        },
        PacketInstruction {
            priority: PacketInstructionPriority::Should,
            text: format!(
                "Use `agent-monitor blame --workspace={} --file=<path> [--line=<n>]` when a changed file needs provenance.",
                case_file.workspace
            ),
        },
    ];
    packet.success_criteria = vec![
        "Fresh agent can state current goal, memory constraints, recent trace, and verification status.".into(),
        "Fresh agent either continues the obvious next step or records a concrete blocker.".into(),
        "Any new file change is traceable with rationale and later verification.".into(),
    ];
    packet
}

pub(crate) fn verification_status_label(status: VerificationStatus) -> &'static str {
    match status {
        VerificationStatus::Passed => "passed",
        VerificationStatus::Failed => "failed",
        VerificationStatus::Stale => "stale",
        VerificationStatus::NotRun => "not_run",
    }
}

pub(crate) fn verification_instruction_text(case_file: &ControlCaseFile) -> String {
    let mut text = format!(
        "Current verification status is {}.",
        verification_status_label(case_file.verification.status)
    );
    if let Some(command) = &case_file.verification.latest_failing_command {
        text.push_str(" Latest failing verifier: `");
        text.push_str(command);
        text.push_str("`.");
    }
    if let Some(failure_class) = case_file.verification.latest_failure_class {
        text.push_str(" Latest failure class: ");
        text.push_str(verification_failure_class_label(failure_class));
        text.push('.');
        if let Some(guidance) = verification_failure_class_packet_guidance(failure_class) {
            text.push(' ');
            text.push_str(guidance);
        }
    }
    if let Some(command) = &case_file.verification.latest_passing_command {
        text.push_str(" Latest passing verifier: `");
        text.push_str(command);
        text.push_str("`.");
    }
    if !case_file.verification.recommended_commands.is_empty() {
        text.push_str(" Recommended verifier: `");
        text.push_str(&case_file.verification.recommended_commands[0]);
        text.push_str("`.");
    }
    if !case_file
        .verification
        .uncovered_acceptance_criteria
        .is_empty()
    {
        text.push_str(" Uncovered acceptance criterion: ");
        text.push_str(&case_file.verification.uncovered_acceptance_criteria[0]);
        text.push('.');
    }
    text
}

pub(crate) fn verification_failure_class_packet_guidance(
    failure_class: VerificationFailureClass,
) -> Option<&'static str> {
    match failure_class {
        VerificationFailureClass::Compile => {
            Some("Treat this as a build/typecheck blocker before behavioral debugging.")
        }
        VerificationFailureClass::Assertion => {
            Some("Inspect the failing assertion before broadening the patch.")
        }
        VerificationFailureClass::Environment => {
            Some("Diagnose service/config availability before switching agents or changing logic.")
        }
        VerificationFailureClass::CoverageGap => {
            Some("Identify or add the relevant targeted verifier before claiming completion.")
        }
        VerificationFailureClass::Timeout => Some(
            "Treat verifier execution as uncertain; narrow or rerun the verifier before editing.",
        ),
        VerificationFailureClass::Deterministic
        | VerificationFailureClass::Flaky
        | VerificationFailureClass::Unknown => None,
    }
}

pub(crate) fn belief_instruction_text(case_file: &ControlCaseFile) -> String {
    let Some(belief) = case_file.belief_state.hypotheses.first() else {
        return "No active failure hypothesis is present; form one from evidence before broad edits."
            .into();
    };

    let mut text = format!(
        "Top failure hypothesis: {} (p={}%, confidence={}%): {}",
        failure_hypothesis_label(belief.kind),
        belief.estimated_probability,
        belief.confidence,
        belief.rationale
    );
    if !belief.missing_evidence.is_empty() {
        text.push_str(". Missing evidence: ");
        text.push_str(&belief.missing_evidence.join(" | "));
    }
    text
}

pub(crate) fn failure_hypothesis_label(kind: FailureHypothesisKind) -> &'static str {
    match kind {
        FailureHypothesisKind::RequirementClosureGap => "requirement closure gap",
        FailureHypothesisKind::RequirementScopeGap => "requirement scope gap",
        FailureHypothesisKind::StaleVerification => "stale verification",
        FailureHypothesisKind::WeakTestOracle => "weak test oracle",
        FailureHypothesisKind::SubagentLifecycleGap => "subagent lifecycle gap",
        FailureHypothesisKind::ProcessConformanceGap => "process conformance gap",
        FailureHypothesisKind::ContextLoss => "context loss",
        FailureHypothesisKind::RepoAttributionGap => "repo attribution gap",
        FailureHypothesisKind::AgentLoop => "agent loop",
        FailureHypothesisKind::OperationalInstability => "operational instability",
        FailureHypothesisKind::UserAuthorityGap => "user authority gap",
    }
}

pub(crate) fn memory_instruction_text(case_file: &ControlCaseFile) -> String {
    let durable_claims = case_file
        .durable_memory
        .iter()
        .take(3)
        .map(|memory| memory.claim.as_str())
        .collect::<Vec<_>>();
    let candidate_claims = case_file
        .memory_candidates
        .iter()
        .take(3)
        .map(|memory| memory.claim.as_str())
        .collect::<Vec<_>>();
    if durable_claims.is_empty() && candidate_claims.is_empty() {
        "No durable design memory candidates are present in this case file; preserve existing project instructions.".into()
    } else if candidate_claims.is_empty() {
        format!(
            "Preserve active durable memory: {}",
            durable_claims.join(" | ")
        )
    } else if durable_claims.is_empty() {
        format!(
            "No active durable memory is present. Treat these as unverified memory candidates, not facts: {}",
            candidate_claims.join(" | ")
        )
    } else {
        format!(
            "Preserve active durable memory: {}. Unverified memory candidates, not facts: {}",
            durable_claims.join(" | "),
            candidate_claims.join(" | ")
        )
    }
}

pub(crate) fn trace_instruction_text(case_file: &ControlCaseFile) -> String {
    let changes = case_file
        .evidence
        .iter()
        .filter(|item| item.kind == "FileChange" || item.kind == "RepoDiff")
        .take(5)
        .map(|item| item.summary.as_str())
        .collect::<Vec<_>>();
    if changes.is_empty() {
        "No recent traced file changes are present; inspect the repo before editing.".into()
    } else {
        format!("Recent traced changes: {}", changes.join(" | "))
    }
}

pub(crate) fn packet_preconditions_for_case_file(
    case_file: &ControlCaseFile,
    target_agent: &str,
) -> PacketPreconditions {
    PacketPreconditions {
        git_head: current_git_head(Path::new(&case_file.workspace)),
        worktree: Some(case_file.workspace.clone()),
        adapter: Some(target_agent.to_string()),
        run_id: latest_evidence_value_for_agent(case_file, target_agent, |event| {
            event.run_id.as_deref()
        }),
        agent_session_id: latest_evidence_value_for_agent(case_file, target_agent, |event| {
            event.agent_session_id.as_deref()
        })
        .or_else(|| {
            latest_evidence_value_for_agent(case_file, target_agent, |event| {
                event.session.as_deref()
            })
        }),
    }
}

pub(crate) fn latest_evidence_value_for_agent<F>(
    case_file: &ControlCaseFile,
    target_agent: &str,
    mut extract: F,
) -> Option<String>
where
    F: FnMut(&EvidenceItem) -> Option<&str>,
{
    let target_agent = normalize_agent_label(target_agent);
    case_file
        .evidence
        .iter()
        .rev()
        .filter(|evidence| {
            evidence
                .agent
                .as_deref()
                .is_some_and(|agent| normalize_agent_label(agent) == target_agent)
        })
        .find_map(|evidence| extract(evidence).filter(|value| !value.is_empty()))
        .map(str::to_string)
}

pub(crate) fn render_control_packet(packet: &ControlPacket) -> String {
    let mut output = String::new();
    if let Ok(agent) = AgentKind::from_str(&packet.target_agent) {
        output.push_str("# ");
        output.push_str(adapter_packet_heading(agent, packet.urgency));
        output.push_str("\n\n");
        output.push_str("Target agent: ");
        output.push_str(agent_kind_label(agent));
        output.push_str("\n\n");
        output.push_str("Action: ");
        output.push_str(&packet.title);
        output.push_str("\n\n");
    } else {
        output.push_str("# ");
        output.push_str(&packet.title);
        output.push_str("\n\n");
    }
    output.push_str("Urgency: ");
    output.push_str(packet.urgency.as_str());
    output.push_str("\n\n");
    output.push_str("Objective: ");
    output.push_str(&packet.summary);
    output.push_str("\n\n## Instructions\n\n");
    for instruction in &packet.instructions {
        output.push_str("- ");
        output.push_str(instruction.priority.label());
        output.push_str(": ");
        output.push_str(&instruction.text);
        output.push('\n');
    }
    if packet_has_preconditions(&packet.preconditions) {
        output.push_str("\n## Preconditions\n\n");
        output.push_str("If any precondition no longer matches current workspace/session, do not act on this packet; report stale packet to the monitor.\n\n");
        if let Some(adapter) = &packet.preconditions.adapter {
            output.push_str("- Adapter: ");
            output.push_str(adapter);
            output.push('\n');
        }
        if let Some(run_id) = &packet.preconditions.run_id {
            output.push_str("- Run id: ");
            output.push_str(run_id);
            output.push('\n');
        }
        if let Some(agent_session_id) = &packet.preconditions.agent_session_id {
            output.push_str("- Agent session id: ");
            output.push_str(agent_session_id);
            output.push('\n');
        }
        if let Some(worktree) = &packet.preconditions.worktree {
            output.push_str("- Worktree: ");
            output.push_str(worktree);
            output.push('\n');
        }
        if let Some(git_head) = &packet.preconditions.git_head {
            output.push_str("- Git HEAD: ");
            output.push_str(git_head);
            output.push('\n');
        }
    }
    if !packet.evidence_refs.is_empty() {
        output.push_str("\n## Evidence\n\n");
        for evidence in &packet.evidence_refs {
            output.push_str("- ");
            output.push_str(evidence);
            output.push('\n');
        }
    }
    if !packet.forbidden.is_empty() {
        output.push_str("\n## Forbidden\n\n");
        for item in &packet.forbidden {
            output.push_str("- ");
            output.push_str(item);
            output.push('\n');
        }
    }
    if !packet.success_criteria.is_empty() {
        output.push_str("\n## Success Criteria\n\n");
        for item in &packet.success_criteria {
            output.push_str("- ");
            output.push_str(item);
            output.push('\n');
        }
    }
    output
}

pub(crate) fn validate_control_packet_is_clean(packet: &ControlPacket) -> Result<(), StoreError> {
    for (field, value) in control_packet_text_fields(packet) {
        if packet_text_is_tainted(&value) {
            return Err(StoreError::SecretLikePacket { field });
        }
    }
    Ok(())
}

pub(crate) fn packet_evidence_refs(packet: &ControlPacket) -> Vec<&str> {
    packet
        .evidence_refs
        .iter()
        .map(|evidence| evidence.trim())
        .filter(|evidence| !evidence.is_empty())
        .collect()
}

pub(crate) fn case_file_known_evidence_ids(case_file: &ControlCaseFile) -> HashSet<&str> {
    let mut known_ids = case_file
        .evidence
        .iter()
        .map(|evidence| evidence.id.as_str())
        .collect::<HashSet<_>>();

    for requirement in &case_file.requirements {
        if let Some(source_event_id) = requirement.source_event_id.as_deref() {
            known_ids.insert(source_event_id);
        }
        if let Some(verification_id) = requirement.latest_verification_evidence_id.as_deref() {
            known_ids.insert(verification_id);
        }
        known_ids.extend(requirement.evidence_ids.iter().map(String::as_str));
        known_ids.extend(
            requirement
                .evidence_refs
                .iter()
                .map(|evidence| evidence.evidence_id.as_str()),
        );
    }

    known_ids
}

pub(crate) fn packet_has_preconditions(preconditions: &PacketPreconditions) -> bool {
    preconditions.adapter.is_some()
        || preconditions.run_id.is_some()
        || preconditions.agent_session_id.is_some()
        || preconditions.worktree.is_some()
        || preconditions.git_head.is_some()
}

pub(crate) fn control_packet_text_fields(packet: &ControlPacket) -> Vec<(String, String)> {
    let mut fields = vec![
        ("packet_id".into(), packet.packet_id.clone()),
        ("target_agent".into(), packet.target_agent.clone()),
        ("title".into(), packet.title.clone()),
        ("summary".into(), packet.summary.clone()),
    ];
    if let Some(git_head) = &packet.preconditions.git_head {
        fields.push(("preconditions.git_head".into(), git_head.clone()));
    }
    if let Some(worktree) = &packet.preconditions.worktree {
        fields.push(("preconditions.worktree".into(), worktree.clone()));
    }
    if let Some(adapter) = &packet.preconditions.adapter {
        fields.push(("preconditions.adapter".into(), adapter.clone()));
    }
    if let Some(run_id) = &packet.preconditions.run_id {
        fields.push(("preconditions.run_id".into(), run_id.clone()));
    }
    if let Some(agent_session_id) = &packet.preconditions.agent_session_id {
        fields.push((
            "preconditions.agent_session_id".into(),
            agent_session_id.clone(),
        ));
    }
    fields.extend(
        packet
            .instructions
            .iter()
            .enumerate()
            .map(|(index, instruction)| {
                (format!("instructions[{index}]"), instruction.text.clone())
            }),
    );
    fields.extend(
        packet
            .evidence_refs
            .iter()
            .enumerate()
            .map(|(index, evidence)| (format!("evidence_refs[{index}]"), evidence.clone())),
    );
    fields.extend(
        packet
            .forbidden
            .iter()
            .enumerate()
            .map(|(index, item)| (format!("forbidden[{index}]"), item.clone())),
    );
    fields.extend(
        packet
            .success_criteria
            .iter()
            .enumerate()
            .map(|(index, item)| (format!("success_criteria[{index}]"), item.clone())),
    );
    fields
}

pub(crate) fn adapter_packet_heading(agent: AgentKind, urgency: PacketUrgency) -> &'static str {
    match agent {
        AgentKind::Codex => match urgency {
            PacketUrgency::Urgent | PacketUrgency::Verification => "CAM BLOCKING NOTE",
            PacketUrgency::FollowUp | PacketUrgency::Context => "CAM FOLLOW-UP PACKET",
        },
        AgentKind::ClaudeCode => "CLAUDE CODE HOOK PACKET",
        AgentKind::OpenCode => "OPENCODE PLUGIN PACKET",
        AgentKind::Pi => "PI SUPERVISOR PACKET",
    }
}

pub(crate) fn default_adapter_enabled() -> bool {
    true
}

pub(crate) fn normalize_agent_label(value: &str) -> String {
    value.trim().to_ascii_lowercase().replace('_', "-")
}

pub(crate) fn safe_slug(value: &str) -> String {
    let slug = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string();
    if slug.is_empty() { "item".into() } else { slug }
}

pub(crate) fn worktree_lock_path(store_root: &Path, worktree: &str) -> PathBuf {
    store_root.join("locks").join("worktrees").join(format!(
        "{}.json",
        safe_slug(&normalize_path_text(worktree))
    ))
}

pub(crate) fn read_worktree_lock(path: &Path) -> Result<WorktreeLock, StoreError> {
    let content = fs::read_to_string(path).map_err(|source| StoreError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    serde_json::from_str(&content).map_err(|source| StoreError::Decode {
        path: path.to_path_buf(),
        line: 1,
        source,
    })
}
