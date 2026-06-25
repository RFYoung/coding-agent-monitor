//! Dashboard rendering: build terminal-friendly rows and detail lines from a monitor snapshot.

use crate::*;

pub(crate) struct DashboardRowsInput<'a> {
    pub(crate) events: &'a [Event],
    pub(crate) interventions: &'a [Intervention],
    pub(crate) verifier_runs: &'a [VerifierRun],
    pub(crate) probe_runs: &'a [ProbeRun],
    pub(crate) repo_hunks: &'a [RepoHunkHistoryEntry],
    pub(crate) repo_hunk_files: &'a [RepoHunkFileSummary],
    pub(crate) requirements: &'a [RequirementNode],
    pub(crate) requirement_proofs: &'a [RequirementProofStep],
    pub(crate) dev_history: &'a [DevHistoryReport],
    pub(crate) decision_trails: &'a [DecisionTrail],
    pub(crate) worktree_lock_events: &'a [WorktreeLockEvent],
}

pub(crate) fn dashboard_rows(input: DashboardRowsInput<'_>) -> Vec<DashboardRow> {
    let mut rows = Vec::with_capacity(
        input.events.len()
            + input.interventions.len()
            + input.verifier_runs.len()
            + input.probe_runs.len()
            + input.repo_hunks.len()
            + input.repo_hunk_files.len()
            + input.requirements.len()
            + input
                .dev_history
                .iter()
                .map(|report| report.findings.len())
                .sum::<usize>()
            + input.decision_trails.len()
            + input.worktree_lock_events.len(),
    );
    for event in input.events {
        rows.push(DashboardRow {
            number: rows.len() + 1,
            kind: DashboardRowKind::Event,
            severity: DashboardSeverity::Healthy,
            agent: Some(event.agent.clone()),
            protocol: format!("{:?}", event.kind),
            summary: event_summary(event),
            detail: serde_json::to_string_pretty(event).unwrap_or_else(|_| String::new()),
        });
    }
    for intervention in input.interventions {
        let severity = match intervention.kind {
            InterventionKind::PrematureStop | InterventionKind::ServiceFailure => {
                DashboardSeverity::Warning
            }
            InterventionKind::AgentDegraded | InterventionKind::SuspiciousChange => {
                DashboardSeverity::Critical
            }
        };
        rows.push(DashboardRow {
            number: rows.len() + 1,
            kind: DashboardRowKind::Intervention,
            severity,
            agent: intervention.agent.clone(),
            protocol: format!("{:?}", intervention.kind),
            summary: intervention.reason.clone(),
            detail: serde_json::to_string_pretty(intervention).unwrap_or_else(|_| String::new()),
        });
    }
    for run in input.verifier_runs {
        let severity = match run.status {
            VerificationRunStatus::Passed => DashboardSeverity::Healthy,
            VerificationRunStatus::Failed | VerificationRunStatus::TimedOut => {
                DashboardSeverity::Warning
            }
        };
        rows.push(DashboardRow {
            number: rows.len() + 1,
            kind: DashboardRowKind::VerifierRun,
            severity,
            agent: None,
            protocol: "verifier".into(),
            summary: verifier_run_summary(run),
            detail: serde_json::to_string_pretty(run).unwrap_or_else(|_| String::new()),
        });
    }
    for run in input.probe_runs {
        rows.push(DashboardRow {
            number: rows.len() + 1,
            kind: DashboardRowKind::ProbeRun,
            severity: probe_run_severity(run),
            agent: None,
            protocol: "probe".into(),
            summary: probe_run_summary(run),
            detail: serde_json::to_string_pretty(run).unwrap_or_else(|_| String::new()),
        });
    }
    for file in input.repo_hunk_files {
        let severity = match file.worst_trace_status {
            RepoTraceStatus::Traced => DashboardSeverity::Healthy,
            RepoTraceStatus::MissingRationale | RepoTraceStatus::Untraced => {
                DashboardSeverity::Warning
            }
        };
        rows.push(DashboardRow {
            number: rows.len() + 1,
            kind: DashboardRowKind::RepoHunkFile,
            severity,
            agent: None,
            protocol: "repo-hunk-file".into(),
            summary: repo_hunk_file_summary(file),
            detail: serde_json::to_string_pretty(file).unwrap_or_else(|_| String::new()),
        });
    }
    for hunk in input.repo_hunks {
        let severity = match hunk.trace_status {
            RepoTraceStatus::Traced => DashboardSeverity::Healthy,
            RepoTraceStatus::MissingRationale | RepoTraceStatus::Untraced => {
                DashboardSeverity::Warning
            }
        };
        rows.push(DashboardRow {
            number: rows.len() + 1,
            kind: DashboardRowKind::RepoHunk,
            severity,
            agent: None,
            protocol: "repo-hunk".into(),
            summary: repo_hunk_history_summary(hunk),
            detail: serde_json::to_string_pretty(hunk).unwrap_or_else(|_| String::new()),
        });
    }
    for requirement in input.requirements {
        let latest_proof = latest_requirement_proof(requirement, input.requirement_proofs);
        rows.push(DashboardRow {
            number: rows.len() + 1,
            kind: DashboardRowKind::Requirement,
            severity: requirement_row_severity(requirement, latest_proof),
            agent: None,
            protocol: "requirement".into(),
            summary: requirement_summary(requirement, latest_proof),
            detail: requirement_dashboard_detail(requirement, input.requirement_proofs),
        });
    }
    for report in input.dev_history {
        for finding in &report.findings {
            rows.push(DashboardRow {
                number: rows.len() + 1,
                kind: DashboardRowKind::DevHistory,
                severity: dev_history_finding_severity(finding),
                agent: None,
                protocol: "dev-history".into(),
                summary: dev_history_finding_summary(finding),
                detail: dev_history_finding_detail(report, finding),
            });
        }
    }
    for trail in input.decision_trails {
        rows.push(DashboardRow {
            number: rows.len() + 1,
            kind: DashboardRowKind::DecisionTrail,
            severity: decision_trail_row_severity(trail),
            agent: Some(trail.packet.target_agent.clone()),
            protocol: "decision-trail".into(),
            summary: decision_trail_summary(trail),
            detail: serde_json::to_string_pretty(trail).unwrap_or_else(|_| String::new()),
        });
    }
    for event in input.worktree_lock_events {
        rows.push(DashboardRow {
            number: rows.len() + 1,
            kind: DashboardRowKind::WorktreeLock,
            severity: worktree_lock_event_severity(event),
            agent: Some(event.lock.owner_agent.clone()),
            protocol: "worktree-lock".into(),
            summary: worktree_lock_event_summary(event),
            detail: serde_json::to_string_pretty(event).unwrap_or_else(|_| String::new()),
        });
    }
    rows
}

pub(crate) fn worktree_lock_event_summary(event: &WorktreeLockEvent) -> String {
    match event.requested_owner.as_deref() {
        Some(requested_owner) => format!(
            "{}: {} owns {}; requested {}",
            event.kind, event.lock.owner_agent, event.lock.worktree, requested_owner
        ),
        None => format!(
            "{}: {} owns {}",
            event.kind, event.lock.owner_agent, event.lock.worktree
        ),
    }
}

pub(crate) fn worktree_lock_event_severity(event: &WorktreeLockEvent) -> DashboardSeverity {
    match event.kind.as_str() {
        "conflict" | "expired" => DashboardSeverity::Warning,
        _ => DashboardSeverity::Healthy,
    }
}

pub(crate) fn decision_trail_summary(trail: &DecisionTrail) -> String {
    let outcome_count = trail.outcomes.len();
    let outcome_word = if outcome_count == 1 {
        "outcome"
    } else {
        "outcomes"
    };
    format!(
        "{} -> {} via {}: {}, {} {}",
        control_action_kind_label(trail.advice.final_action.kind()),
        trail.packet.target_agent,
        trail.packet.packet_id,
        dispatch_status_label(trail.dispatch.status),
        outcome_count,
        outcome_word
    )
}

pub(crate) fn decision_trail_row_severity(trail: &DecisionTrail) -> DashboardSeverity {
    if trail.dispatch.status == DispatchStatus::Failed
        || trail
            .outcomes
            .iter()
            .any(|outcome| outcome.status == OutcomeStatus::Failed)
    {
        DashboardSeverity::Warning
    } else {
        DashboardSeverity::Healthy
    }
}

pub(crate) fn dispatch_status_label(status: DispatchStatus) -> &'static str {
    match status {
        DispatchStatus::OutboxWritten => "outbox_written",
        DispatchStatus::SuppressedDuplicate => "suppressed_duplicate",
        DispatchStatus::Failed => "failed",
    }
}

#[derive(Serialize)]
pub(crate) struct DevHistoryFindingDashboardDetail<'a> {
    workspace: &'a str,
    generated_at: &'a str,
    sources: &'a [DevHistorySourceReport],
    finding: &'a DevHistoryFinding,
}

pub(crate) fn dev_history_finding_detail(
    report: &DevHistoryReport,
    finding: &DevHistoryFinding,
) -> String {
    serde_json::to_string_pretty(&DevHistoryFindingDashboardDetail {
        workspace: &report.workspace,
        generated_at: &report.generated_at,
        sources: &report.sources,
        finding,
    })
    .unwrap_or_else(|_| String::new())
}

pub(crate) fn dev_history_finding_summary(finding: &DevHistoryFinding) -> String {
    format!("{}: {}", finding.kind, finding.summary)
}

pub(crate) fn dev_history_finding_severity(finding: &DevHistoryFinding) -> DashboardSeverity {
    match finding.severity.as_str() {
        "critical" => DashboardSeverity::Critical,
        "warning" => DashboardSeverity::Warning,
        _ => DashboardSeverity::Healthy,
    }
}

#[derive(Serialize)]
pub(crate) struct RequirementDashboardDetail<'a> {
    requirement: &'a RequirementNode,
    proofs: Vec<&'a RequirementProofStep>,
}

pub(crate) fn requirement_dashboard_detail(
    requirement: &RequirementNode,
    requirement_proofs: &[RequirementProofStep],
) -> String {
    let proofs = requirement_proofs
        .iter()
        .filter(|proof| proof.requirement_id == requirement.requirement_id)
        .collect::<Vec<_>>();
    serde_json::to_string_pretty(&RequirementDashboardDetail {
        requirement,
        proofs,
    })
    .unwrap_or_else(|_| String::new())
}

pub(crate) fn latest_requirement_proof<'a>(
    requirement: &RequirementNode,
    requirement_proofs: &'a [RequirementProofStep],
) -> Option<&'a RequirementProofStep> {
    requirement_proofs
        .iter()
        .find(|proof| proof.requirement_id == requirement.requirement_id)
}

pub(crate) fn requirement_summary(
    requirement: &RequirementNode,
    latest_proof: Option<&RequirementProofStep>,
) -> String {
    if let Some(proof) = latest_proof {
        return format!(
            "{} proof {}: {}",
            acceptance_coverage_status_label(requirement.status),
            proof.proof_strength.score,
            requirement.text
        );
    }
    format!(
        "{}: {}",
        acceptance_coverage_status_label(requirement.status),
        requirement.text
    )
}

pub(crate) fn requirement_row_severity(
    requirement: &RequirementNode,
    latest_proof: Option<&RequirementProofStep>,
) -> DashboardSeverity {
    let status_severity = requirement_status_severity(requirement.status);
    if status_severity == DashboardSeverity::Healthy
        && latest_proof
            .map(|proof| proof.proof_strength.score < 50)
            .unwrap_or(true)
    {
        return DashboardSeverity::Warning;
    }
    status_severity
}

pub(crate) fn requirement_status_severity(status: AcceptanceCoverageStatus) -> DashboardSeverity {
    match status {
        AcceptanceCoverageStatus::Covered => DashboardSeverity::Healthy,
        AcceptanceCoverageStatus::Failed
        | AcceptanceCoverageStatus::Stale
        | AcceptanceCoverageStatus::Unverified
        | AcceptanceCoverageStatus::Unmapped => DashboardSeverity::Warning,
    }
}

pub(crate) fn acceptance_coverage_status_label(status: AcceptanceCoverageStatus) -> &'static str {
    match status {
        AcceptanceCoverageStatus::Covered => "covered",
        AcceptanceCoverageStatus::Stale => "stale",
        AcceptanceCoverageStatus::Failed => "failed",
        AcceptanceCoverageStatus::Unverified => "unverified",
        AcceptanceCoverageStatus::Unmapped => "unmapped",
    }
}

pub(crate) fn repo_hunk_history_summary(hunk: &RepoHunkHistoryEntry) -> String {
    format!(
        "{} hunk {}: {}, {} matching trace(s)",
        hunk.path,
        hunk.hunk_index,
        repo_trace_status_label(hunk.trace_status),
        hunk.matching_trace_count
    )
}

pub(crate) fn repo_hunk_file_summary(file: &RepoHunkFileSummary) -> String {
    format!(
        "{}: {} hunk(s), worst {}, latest {}, {} matching trace(s)",
        file.path,
        file.entry_count,
        repo_trace_status_label(file.worst_trace_status),
        repo_trace_status_label(file.latest_trace_status),
        file.matching_trace_count
    )
}

pub(crate) fn repo_trace_status_label(status: RepoTraceStatus) -> &'static str {
    match status {
        RepoTraceStatus::Traced => "traced",
        RepoTraceStatus::MissingRationale => "missing rationale",
        RepoTraceStatus::Untraced => "untraced",
    }
}

pub(crate) fn verifier_run_summary(run: &VerifierRun) -> String {
    let verifier = run.verifier_id.as_deref().unwrap_or("<unknown>");
    match run.failure_class {
        Some(failure_class) => format!(
            "{verifier}: {:?}, {} ({})",
            run.status,
            verification_failure_class_label(failure_class),
            run.command
        ),
        None => format!("{verifier}: {:?} ({})", run.status, run.command),
    }
}

pub(crate) fn probe_run_summary(run: &ProbeRun) -> String {
    format!(
        "{}: {}, {}",
        probe_spec_kind_label(&run.probe),
        outcome_status_label(run.status),
        run.summary
    )
}

pub(crate) fn probe_run_severity(run: &ProbeRun) -> DashboardSeverity {
    match run.status {
        OutcomeStatus::Succeeded => DashboardSeverity::Healthy,
        OutcomeStatus::Failed | OutcomeStatus::Unknown => DashboardSeverity::Warning,
    }
}

pub(crate) fn probe_spec_kind_label(probe: &ProbeSpec) -> String {
    match probe {
        ProbeSpec::LocalEvidence { .. } => "local_evidence".into(),
        ProbeSpec::RuntimeValidation { surface, .. } => {
            format!("runtime_validation:{}", surface.kind_label())
        }
        ProbeSpec::BrowserValidation { .. } => "browser_validation".into(),
        ProbeSpec::RepoInspection { .. } => "repo_inspection".into(),
        ProbeSpec::TargetedTest { .. } => "targeted_test".into(),
    }
}

pub(crate) fn outcome_status_label(status: OutcomeStatus) -> &'static str {
    match status {
        OutcomeStatus::Succeeded => "succeeded",
        OutcomeStatus::Failed => "failed",
        OutcomeStatus::Unknown => "unknown",
    }
}

pub(crate) fn verification_failure_class_label(
    failure_class: VerificationFailureClass,
) -> &'static str {
    match failure_class {
        VerificationFailureClass::Deterministic => "deterministic failure",
        VerificationFailureClass::Flaky => "flaky failure",
        VerificationFailureClass::Environment => "environment failure",
        VerificationFailureClass::Compile => "compile failure",
        VerificationFailureClass::Assertion => "assertion failure",
        VerificationFailureClass::CoverageGap => "coverage-gap failure",
        VerificationFailureClass::Timeout => "timeout failure",
        VerificationFailureClass::Unknown => "unknown failure",
    }
}

pub(crate) fn event_summary(event: &Event) -> String {
    if let Some(file) = &event.file {
        if let Some(rationale) = &event.rationale {
            return format!("{file}: {rationale}");
        }
        return file.clone();
    }
    event.content.clone().unwrap_or_default()
}

pub(crate) fn parse_row_kind(value: &str) -> Result<DashboardRowKind, DashboardFilterError> {
    match value {
        "event" => Ok(DashboardRowKind::Event),
        "intervention" => Ok(DashboardRowKind::Intervention),
        "verifier" | "verifier_run" | "verifier-run" => Ok(DashboardRowKind::VerifierRun),
        "probe" | "probe_run" | "probe-run" => Ok(DashboardRowKind::ProbeRun),
        "repo_hunk_file" | "repo-hunk-file" | "hunk_file" | "hunk-file" => {
            Ok(DashboardRowKind::RepoHunkFile)
        }
        "repo_hunk" | "repo-hunk" | "hunk" => Ok(DashboardRowKind::RepoHunk),
        "requirement" | "requirements" | "req" => Ok(DashboardRowKind::Requirement),
        "dev_history" | "dev-history" | "history" => Ok(DashboardRowKind::DevHistory),
        "decision_trail" | "decision-trail" | "decision" | "advice" => {
            Ok(DashboardRowKind::DecisionTrail)
        }
        "worktree_lock" | "worktree-lock" | "lock_event" | "lock-event" | "lock" => {
            Ok(DashboardRowKind::WorktreeLock)
        }
        _ => Err(DashboardFilterError::InvalidValue {
            field: "kind".into(),
            value: value.into(),
        }),
    }
}

pub(crate) fn parse_dashboard_severity(
    value: &str,
) -> Result<DashboardSeverity, DashboardFilterError> {
    match value {
        "healthy" => Ok(DashboardSeverity::Healthy),
        "warning" => Ok(DashboardSeverity::Warning),
        "critical" => Ok(DashboardSeverity::Critical),
        _ => Err(DashboardFilterError::InvalidValue {
            field: "severity".into(),
            value: value.into(),
        }),
    }
}

pub(crate) fn looks_like_premature_stop(content: &str) -> bool {
    let text = content.to_lowercase();
    [
        "good point to stop",
        "ask the user",
        "ask user",
        "do the remaining",
        "remaining jobs",
        "should i continue",
    ]
    .iter()
    .any(|signal| text.contains(signal))
}

pub(crate) fn event_asks_routine_user_question(event: &Event, content: &str) -> bool {
    if event.kind == EventKind::UserInstruction
        || user_decision_cause_for_event(event, content).is_some()
    {
        return false;
    }
    let text = content.to_lowercase();
    [
        "should i run",
        "should i inspect",
        "should i check",
        "should i debug",
        "should i look",
        "should i continue",
        "which file should i",
        "which files should i",
        "which migration",
        "which batch",
        "what should i do next",
        "do you want me to continue",
        "do you want me to run",
    ]
    .iter()
    .any(|signal| text.contains(signal))
}

pub(crate) fn looks_like_unverified_completion(content: &str) -> bool {
    let text = content.to_lowercase();
    let admits_unverified = [
        "did not run tests",
        "haven't run tests",
        "have not run tests",
        "not run tests",
        "without running tests",
        "unable to run tests",
    ]
    .iter()
    .any(|signal| text.contains(signal));
    looks_like_completion_claim(content) && admits_unverified
}

pub(crate) fn looks_like_completion_claim(content: &str) -> bool {
    let text = content.to_lowercase();
    [
        "implementation complete",
        "task complete",
        "work complete",
        "changes complete",
        "finished",
        "ready for review",
        "ready to review",
        "ready for handoff",
        "ready to hand off",
    ]
    .iter()
    .any(|signal| text.contains(signal))
        || text
            .split(|character: char| !character.is_ascii_alphanumeric())
            .any(|word| word == "done")
}

pub(crate) fn truncate_evidence(content: &str) -> String {
    const MAX_CHARS: usize = 240;
    let trimmed = content.trim();
    if trimmed.chars().count() <= MAX_CHARS {
        return trimmed.to_string();
    }
    let prefix = trimmed.chars().take(MAX_CHARS - 1).collect::<String>();
    format!("{}…", prefix.trim_end())
}

pub(crate) fn classify_command_failure_layer(event: &Event, content: &str) -> FailureLayer {
    if let Some(layer) = classify_service_failure_layer(content) {
        return layer;
    }

    let text = failure_layer_text(event, content);
    if contains_failure_signal(
        &text,
        &[
            "command not found",
            "is not recognized as",
            "no such file or directory",
            "cannot find path",
            "enoent",
            "spawn ",
            "exec format error",
            "permission denied",
            "operation not permitted",
            "access is denied",
        ],
    ) {
        return FailureLayer::ShellRuntime;
    }
    if contains_failure_signal(
        &text,
        &[
            "not a git repository",
            "merge conflict",
            "unmerged files",
            "index.lock",
            "working tree",
            "working directory",
            "repository state",
        ],
    ) {
        return FailureLayer::RepoState;
    }
    if event
        .command
        .as_deref()
        .is_some_and(is_verification_command)
    {
        return FailureLayer::TestRuntime;
    }
    if content.trim().is_empty() {
        return FailureLayer::Unknown;
    }
    FailureLayer::TaskLogic
}

pub(crate) fn classify_service_failure_layer(content: &str) -> Option<FailureLayer> {
    let text = content.to_lowercase();
    if contains_failure_signal(
        &text,
        &[
            "rate limit",
            "rate_limit",
            "too many requests",
            "429",
            "quota exceeded",
            "overloaded",
        ],
    ) {
        return Some(FailureLayer::RateLimit);
    }
    if contains_failure_signal(
        &text,
        &[
            "authentication",
            "authorization",
            "unauthorized",
            "forbidden",
            "invalid api key",
            "invalid token",
            "401",
            "403",
        ],
    ) {
        return Some(FailureLayer::Auth);
    }
    if contains_failure_signal(
        &text,
        &[
            "connection reset",
            "connection refused",
            "connection aborted",
            "network",
            "dns",
            "econnreset",
            "etimedout",
            "timed out",
            "timeout",
        ],
    ) {
        return Some(FailureLayer::Transport);
    }
    if contains_failure_signal(
        &text,
        &[
            "context length exceeded",
            "context limit",
            "context window",
            "token limit",
            "service unavailable",
            "upstream",
            "provider unavailable",
            "internal server error",
        ],
    ) || contains_5xx_status(&text)
    {
        return Some(FailureLayer::Provider);
    }
    None
}

pub(crate) fn failure_layer_text(event: &Event, content: &str) -> String {
    let mut text = String::new();
    if let Some(command) = event.command.as_deref() {
        text.push_str(command);
        text.push('\n');
    }
    text.push_str(content);
    text.to_lowercase()
}

pub(crate) fn contains_failure_signal(text: &str, signals: &[&str]) -> bool {
    signals.iter().any(|signal| text.contains(signal))
}

pub(crate) fn contains_5xx_status(text: &str) -> bool {
    ["500", "502", "503", "504"]
        .iter()
        .any(|status| text.contains(status))
}

pub(crate) fn looks_like_service_failure(content: &str) -> bool {
    classify_service_failure_layer(content).is_some()
}

pub(crate) fn looks_like_permission_denial(content: &str) -> bool {
    let text = content.to_lowercase();
    [
        "permission denied",
        "operation not permitted",
        "access is denied",
        "sandbox denied",
        "approval denied",
        "pretooluse denied",
    ]
    .iter()
    .any(|signal| text.contains(signal))
}

pub(crate) fn looks_like_permission_request(content: &str) -> bool {
    let text = content.to_lowercase();
    [
        "permission requested",
        "approval requested",
        "permission pending",
        "approval pending",
        "permission deferred",
    ]
    .iter()
    .any(|signal| text.contains(signal))
}

pub(crate) fn permission_lifecycle_is_blocked(content: &str) -> bool {
    looks_like_permission_denial(content) || looks_like_permission_request(content)
}

pub(crate) fn looks_like_forgetting_design_memory(content: &str) -> bool {
    let text = content.to_lowercase();
    [
        "do not remember the design",
        "don't remember the design",
        "forgot the design",
        "forgot the user wanted",
        "lost the context",
        "lost design memory",
    ]
    .iter()
    .any(|signal| text.contains(signal))
}

pub(crate) fn looks_like_context_compaction(content: &str) -> bool {
    let text = content.to_lowercase();
    [
        "context compaction",
        "context compacted",
        "transcript compacted",
        "transcript summarized",
        "conversation summarized",
        "context summarized",
        "context window reset",
        "context window was reset",
    ]
    .iter()
    .any(|signal| text.contains(signal))
}

pub(crate) fn looks_like_session_error(content: &str) -> bool {
    let text = content.to_lowercase();
    [
        "session error",
        "session failed",
        "session crashed",
        "agent process crashed",
        "process crashed",
        "agent session failed",
    ]
    .iter()
    .any(|signal| text.contains(signal))
}

pub(crate) fn looks_like_session_idle_or_stop(content: &str) -> bool {
    let text = content.to_lowercase();
    [
        "session idle",
        "session stopped",
        "session stop",
        "session ended",
        "session finished",
    ]
    .iter()
    .any(|signal| text.contains(signal))
}

pub(crate) fn user_decision_cause(content: &str) -> Option<&'static str> {
    let text = content.to_lowercase();

    if [
        "need credentials",
        "needs credentials",
        "requires credentials",
        "need the user's api key",
        "need your api key",
        "api key from the user",
        "access token from the user",
        "login required",
        "credential required",
    ]
    .iter()
    .any(|signal| text.contains(signal))
    {
        return Some("credentials or secret access are required");
    }

    if [
        "delete the production",
        "delete production",
        "drop database",
        "drop the database",
        "production database",
        "destructive action",
        "deploy to production",
        "production migration",
        "external side effect",
        "call the external",
        "external billing api",
    ]
    .iter()
    .any(|signal| text.contains(signal))
    {
        return Some("destructive or external side-effect consent is required");
    }

    if [
        "spend money",
        "incur cost",
        "paid api",
        "billing api",
        "charge the account",
        "requires payment",
    ]
    .iter()
    .any(|signal| text.contains(signal))
    {
        return Some("spending or billing authorization is required");
    }

    if [
        "product decision",
        "user preference",
        "which api design",
        "choose between",
        "ambiguous requirement",
        "irreversible api",
        "irreversible ux",
    ]
    .iter()
    .any(|signal| text.contains(signal))
    {
        return Some("a product or preference decision is required");
    }

    None
}

pub(crate) fn user_decision_cause_for_event(event: &Event, content: &str) -> Option<&'static str> {
    event
        .command
        .as_deref()
        .and_then(destructive_command_user_decision_cause)
        .or_else(|| user_decision_cause(content))
}

pub(crate) fn destructive_command_user_decision_cause(command: &str) -> Option<&'static str> {
    let command = normalize_command_signature(command).to_lowercase();
    let tokens = command.split_whitespace().collect::<Vec<_>>();

    if tokens.first() == Some(&"git")
        && ((tokens.get(1) == Some(&"reset") && tokens.contains(&"--hard"))
            || (tokens.get(1) == Some(&"clean")
                && tokens
                    .iter()
                    .skip(2)
                    .any(|token| git_clean_token_has_force_flag(token))))
    {
        return Some("destructive command requires explicit user authorization");
    }

    if tokens
        .first()
        .is_some_and(|token| *token == "rm" || *token == "del")
        && shell_delete_tokens_are_recursive_force(&tokens)
    {
        return Some("destructive command requires explicit user authorization");
    }

    if command.contains("remove-item")
        && command.contains("-recurse")
        && (command.contains("-force") || command.contains(" -r"))
    {
        return Some("destructive command requires explicit user authorization");
    }

    if command.contains("drop database")
        || command.contains("terraform destroy")
        || command.contains("kubectl delete")
        || command.contains("aws ") && command.contains(" delete-")
        || command.contains("az ") && command.contains(" delete")
        || command.contains("gcloud ") && command.contains(" delete")
    {
        return Some("destructive command requires explicit user authorization");
    }

    None
}

pub(crate) fn shell_delete_tokens_are_recursive_force(tokens: &[&str]) -> bool {
    let mut recursive = false;
    let mut force = false;

    for token in tokens.iter().skip(1) {
        if token.starts_with("--") {
            recursive |= matches!(*token, "--recursive" | "--recurse");
            force |= *token == "--force";
            continue;
        }

        if let Some(short) = token.strip_prefix('-') {
            recursive |= short.contains('r') || short.contains('R');
            force |= short.contains('f');
        }

        if token.starts_with('/') {
            recursive |= token.eq_ignore_ascii_case("/s");
            force |= token.eq_ignore_ascii_case("/q") || token.eq_ignore_ascii_case("/f");
        }
    }

    recursive && force
}

pub(crate) fn git_clean_token_has_force_flag(token: &str) -> bool {
    token == "--force"
        || token == "-f"
        || (token.starts_with('-')
            && !token.starts_with("--")
            && token.chars().skip(1).any(|ch| ch == 'f'))
}
