use crate::{
    ActionOutcome, AdviceRun, ControlAction, ControlActionKind, EntropyDelta, EntropyKind, Event,
    EventKind, OutcomeStatus, ProbeSpec, ProjectConfig, ProjectConfigError, ProjectStore,
    RepoAuditError, RuntimeValidationSurface, StoreError, VerificationRunStatus, VerifyError,
    current_id_fragment, current_utc_timestamp, read_all_jsonl, record_repo_audit_history,
    run_verifier, safe_slug,
};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProbeRun {
    pub probe_run_id: String,
    pub advice_id: String,
    pub probe: ProbeSpec,
    pub status: OutcomeStatus,
    pub started_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum ProbeError {
    #[error("project store: {0}")]
    Store(#[from] StoreError),
    #[error("no advice records are available for probe execution")]
    NoAdvice,
    #[error("latest advice is not run_probe: {advice_id} is {action:?}")]
    LatestAdviceNotRunProbe {
        advice_id: String,
        action: ControlActionKind,
    },
    #[error("project config: {0}")]
    Config(#[from] ProjectConfigError),
    #[error("verifier probe: {0}")]
    Verify(#[from] VerifyError),
    #[error("targeted_test command is not configured as a verifier: {command}")]
    TargetedTestCommandNotConfigured { command: String },
    #[error("monitor-owned probe execution does not support {kind} yet")]
    UnsupportedProbe { kind: &'static str },
}

pub fn run_probe(workspace: impl AsRef<Path>) -> Result<ProbeRun, ProbeError> {
    let workspace = workspace.as_ref();
    let mut store = ProjectStore::open(workspace)?;
    let advice_records = read_all_jsonl::<AdviceRun>(&store.root().join("advice.jsonl"))?;
    let advice = advice_records.last().ok_or(ProbeError::NoAdvice)?;
    let probe = match &advice.final_action {
        ControlAction::RunProbe { probe } => probe.clone(),
        action => {
            return Err(ProbeError::LatestAdviceNotRunProbe {
                advice_id: advice.advice_id.clone(),
                action: action.kind(),
            });
        }
    };

    let run = match &probe {
        ProbeSpec::RepoInspection { target } => Ok(run_repo_inspection_probe(
            workspace,
            advice,
            probe.clone(),
            target.as_deref(),
        )),
        ProbeSpec::TargetedTest { command } => {
            run_targeted_test_probe(workspace, &store, advice, probe.clone(), command)
        }
        ProbeSpec::LocalEvidence { target } => Ok(run_local_evidence_probe(
            workspace,
            &store,
            advice,
            probe.clone(),
            target.as_deref(),
        )),
        ProbeSpec::RuntimeValidation { surface, target } => run_runtime_validation_probe(
            workspace,
            &store,
            advice,
            probe.clone(),
            *surface,
            target.as_deref(),
        ),
        ProbeSpec::BrowserValidation { .. } => {
            return Err(ProbeError::UnsupportedProbe {
                kind: "browser_validation",
            });
        }
    }?;

    store.append_probe_run(&run)?;
    if !probe_outcome_already_recorded(&store, &advice.advice_id)? {
        store.append_action_outcome(&probe_outcome_for_run(advice, &run))?;
    }
    Ok(run)
}

fn run_local_evidence_probe(
    workspace: &Path,
    store: &ProjectStore,
    advice: &AdviceRun,
    probe: ProbeSpec,
    target: Option<&str>,
) -> ProbeRun {
    let prefix = target
        .map(|target| format!("local evidence probe for `{target}`"))
        .unwrap_or_else(|| "local evidence probe".into());
    run_evidence_probe(workspace, store, advice, probe, &prefix)
}

fn run_runtime_validation_probe(
    workspace: &Path,
    store: &ProjectStore,
    advice: &AdviceRun,
    probe: ProbeSpec,
    surface: RuntimeValidationSurface,
    target: Option<&str>,
) -> Result<ProbeRun, ProbeError> {
    let started_at = current_utc_timestamp().unwrap_or_else(|| "1970-01-01T00:00:00Z".into());
    let Some(verifier_id) = configured_runtime_validation_verifier_id(store, surface)? else {
        let completed_at = current_utc_timestamp();
        let marker = runtime_validation_verifier_marker(surface);
        let mut summary = format!(
            "runtime validation unsupported for {}: no configured verifier with acceptance_patterns containing `{marker}`",
            surface.label()
        );
        if let Some(target) = target {
            summary.push_str("; target: ");
            summary.push_str(target);
        }
        return Ok(ProbeRun {
            probe_run_id: format!("probe-run-{}", current_id_fragment()),
            advice_id: advice.advice_id.clone(),
            probe,
            status: OutcomeStatus::Unknown,
            started_at,
            completed_at,
            summary,
            evidence_ids: Vec::new(),
            note: Some(format!(
                "configure a verifier with acceptance_patterns including `{marker}` before treating {} runtime validation as successful",
                surface.label()
            )),
        });
    };

    let verifier_run = run_verifier(workspace, &verifier_id)?;
    let completed_at = current_utc_timestamp();
    let status = match verifier_run.status {
        VerificationRunStatus::Passed => OutcomeStatus::Succeeded,
        VerificationRunStatus::Failed | VerificationRunStatus::TimedOut => OutcomeStatus::Failed,
    };
    let status_label = match verifier_run.status {
        VerificationRunStatus::Passed => "passed",
        VerificationRunStatus::Failed => "failed",
        VerificationRunStatus::TimedOut => "timed out",
    };
    let mut summary = format!(
        "runtime validation {status_label} for {} via verifier `{}`: `{}`",
        surface.label(),
        verifier_id,
        verifier_run.command
    );
    if let Some(target) = target {
        summary.push_str("; target: ");
        summary.push_str(target);
    }

    Ok(ProbeRun {
        probe_run_id: format!("probe-run-{}", current_id_fragment()),
        advice_id: advice.advice_id.clone(),
        probe,
        status,
        started_at,
        completed_at,
        summary,
        evidence_ids: vec![verifier_run.verifier_run_id],
        note: verifier_run.failure_class.map(|failure_class| {
            format!(
                "verifier failure class: {}",
                failure_class_label(failure_class)
            )
        }),
    })
}

fn run_evidence_probe(
    workspace: &Path,
    store: &ProjectStore,
    advice: &AdviceRun,
    probe: ProbeSpec,
    summary_prefix: &str,
) -> ProbeRun {
    let started_at = current_utc_timestamp().unwrap_or_else(|| "1970-01-01T00:00:00Z".into());
    let recent = recent_local_evidence(store);
    let repo_result = record_repo_audit_history(workspace);
    let completed_at = current_utc_timestamp();
    let mut evidence_ids = recent
        .iter()
        .filter_map(|event| event.event_id.clone())
        .collect::<Vec<_>>();
    let mut summary = format!(
        "{} observed {} recent evidence event(s)",
        summary_prefix,
        recent.len(),
    );
    if let Some(event) = recent.last()
        && let Some(label) = event_probe_label(event)
    {
        summary.push_str("; latest: ");
        summary.push_str(&label);
    }

    let mut note = None;
    match repo_result {
        Ok(report) => {
            summary.push_str(&format!(
                "; repo inspection found {} changed file(s), {} untraced, {} missing rationale",
                report.changes.len(),
                report.untraced_count,
                report.unexplained_count
            ));
            for change in report.changes {
                push_unique(
                    &mut evidence_ids,
                    format!("repo-audit-{}", safe_slug(&change.path)),
                );
            }
        }
        Err(error) => {
            summary.push_str("; repo inspection unavailable");
            note = Some(error.to_string());
        }
    }

    ProbeRun {
        probe_run_id: format!("probe-run-{}", current_id_fragment()),
        advice_id: advice.advice_id.clone(),
        probe,
        status: OutcomeStatus::Succeeded,
        started_at,
        completed_at,
        summary,
        evidence_ids,
        note,
    }
}

fn recent_local_evidence(store: &ProjectStore) -> Vec<Event> {
    read_all_jsonl::<Event>(&store.root().join("events.jsonl"))
        .unwrap_or_default()
        .into_iter()
        .filter(local_evidence_event_is_salient)
        .rev()
        .take(5)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

fn local_evidence_event_is_salient(event: &Event) -> bool {
    matches!(
        event.kind,
        EventKind::UserInstruction
            | EventKind::ModelMessage
            | EventKind::ToolCall
            | EventKind::ToolResult
            | EventKind::CommandResult
            | EventKind::TestResult
            | EventKind::RepoDiff
            | EventKind::FileChange
            | EventKind::VerificationClaim
            | EventKind::InterventionResult
    )
}

fn event_probe_label(event: &Event) -> Option<String> {
    event
        .file
        .as_deref()
        .or(event.command.as_deref())
        .or(event.content.as_deref())
        .map(|value| value.trim())
        .filter(|value| !value.is_empty())
        .map(|value| {
            let value = value.chars().take(120).collect::<String>();
            format!("{} {}", event_kind_probe_label(&event.kind), value)
        })
}

fn event_kind_probe_label(kind: &EventKind) -> &'static str {
    match kind {
        EventKind::ModelMessage => "model_message",
        EventKind::DesignThought => "design_thought",
        EventKind::FileChange => "file_change",
        EventKind::CommandOutput => "command_output",
        EventKind::CommandResult => "command_result",
        EventKind::ToolCall => "tool_call",
        EventKind::ToolResult => "tool_result",
        EventKind::TestResult => "test_result",
        EventKind::RepoDiff => "repo_diff",
        EventKind::UserInstruction => "user_instruction",
        EventKind::HandoffSummary => "handoff_summary",
        EventKind::AgentHealth => "agent_health",
        EventKind::VerificationClaim => "verification_claim",
        EventKind::InterventionResult => "intervention_result",
    }
}

fn run_targeted_test_probe(
    workspace: &Path,
    store: &ProjectStore,
    advice: &AdviceRun,
    probe: ProbeSpec,
    command: &str,
) -> Result<ProbeRun, ProbeError> {
    let verifier_id = configured_verifier_id_for_command(store, command)?;
    let started_at = current_utc_timestamp().unwrap_or_else(|| "1970-01-01T00:00:00Z".into());
    let verifier_run = run_verifier(workspace, &verifier_id)?;
    let completed_at = current_utc_timestamp();
    let status = match verifier_run.status {
        VerificationRunStatus::Passed => OutcomeStatus::Succeeded,
        VerificationRunStatus::Failed | VerificationRunStatus::TimedOut => OutcomeStatus::Failed,
    };
    let summary = match verifier_run.status {
        VerificationRunStatus::Passed => format!(
            "targeted test probe passed via verifier `{}`: `{}`",
            verifier_id, verifier_run.command
        ),
        VerificationRunStatus::Failed => format!(
            "targeted test probe failed via verifier `{}`: `{}`",
            verifier_id, verifier_run.command
        ),
        VerificationRunStatus::TimedOut => format!(
            "targeted test probe timed out via verifier `{}`: `{}`",
            verifier_id, verifier_run.command
        ),
    };

    Ok(ProbeRun {
        probe_run_id: format!("probe-run-{}", current_id_fragment()),
        advice_id: advice.advice_id.clone(),
        probe,
        status,
        started_at,
        completed_at,
        summary,
        evidence_ids: vec![verifier_run.verifier_run_id],
        note: verifier_run.failure_class.map(|failure_class| {
            format!(
                "verifier failure class: {}",
                failure_class_label(failure_class)
            )
        }),
    })
}

fn configured_verifier_id_for_command(
    store: &ProjectStore,
    command: &str,
) -> Result<String, ProbeError> {
    let command = command.trim();
    let config = ProjectConfig::load(store.root())?;
    config
        .verifiers
        .iter()
        .find(|verifier| verifier.command.trim() == command)
        .map(|verifier| verifier.id.clone())
        .ok_or_else(|| ProbeError::TargetedTestCommandNotConfigured {
            command: command.into(),
        })
}

fn configured_runtime_validation_verifier_id(
    store: &ProjectStore,
    surface: RuntimeValidationSurface,
) -> Result<Option<String>, ProbeError> {
    let marker = runtime_validation_verifier_marker(surface);
    let config = ProjectConfig::load(store.root())?;
    Ok(config
        .verifiers
        .iter()
        .find(|verifier| {
            verifier
                .acceptance_patterns
                .iter()
                .any(|pattern| pattern.trim().eq_ignore_ascii_case(&marker))
        })
        .map(|verifier| verifier.id.clone()))
}

fn runtime_validation_verifier_marker(surface: RuntimeValidationSurface) -> String {
    format!("runtime_validation:{}", surface.kind_label())
}

fn failure_class_label(failure_class: crate::VerificationFailureClass) -> &'static str {
    match failure_class {
        crate::VerificationFailureClass::Compile => "compile",
        crate::VerificationFailureClass::Assertion => "assertion",
        crate::VerificationFailureClass::Environment => "environment",
        crate::VerificationFailureClass::CoverageGap => "coverage_gap",
        crate::VerificationFailureClass::Timeout => "timeout",
        crate::VerificationFailureClass::Deterministic => "deterministic",
        crate::VerificationFailureClass::Flaky => "flaky",
        crate::VerificationFailureClass::Unknown => "unknown",
    }
}

fn push_unique(values: &mut Vec<String>, value: String) {
    if !values.contains(&value) {
        values.push(value);
    }
}

fn run_repo_inspection_probe(
    workspace: &Path,
    advice: &AdviceRun,
    probe: ProbeSpec,
    target: Option<&str>,
) -> ProbeRun {
    let started_at = current_utc_timestamp().unwrap_or_else(|| "1970-01-01T00:00:00Z".into());
    let result = record_repo_audit_history(workspace);
    let completed_at = current_utc_timestamp();
    let (status, summary, evidence_ids, note) = match result {
        Ok(report) => {
            let changed_files = report
                .changes
                .iter()
                .map(|change| change.path.as_str())
                .collect::<Vec<_>>();
            let evidence_ids = report
                .changes
                .iter()
                .map(|change| format!("repo-audit-{}", safe_slug(&change.path)))
                .collect::<Vec<_>>();
            let mut summary = format!(
                "repo inspection found {} changed file(s), {} untraced, {} missing rationale",
                report.changes.len(),
                report.untraced_count,
                report.unexplained_count
            );
            if let Some(target) = target {
                summary.push_str("; target: ");
                summary.push_str(target);
            }
            if !changed_files.is_empty() {
                summary.push_str("; files: ");
                summary.push_str(&changed_files.join(", "));
            }
            (OutcomeStatus::Succeeded, summary, evidence_ids, None)
        }
        Err(error) => (
            OutcomeStatus::Failed,
            probe_failure_summary(target, &error),
            Vec::new(),
            Some(error.to_string()),
        ),
    };

    ProbeRun {
        probe_run_id: format!("probe-run-{}", current_id_fragment()),
        advice_id: advice.advice_id.clone(),
        probe,
        status,
        started_at,
        completed_at,
        summary,
        evidence_ids,
        note,
    }
}

fn probe_failure_summary(target: Option<&str>, error: &RepoAuditError) -> String {
    let mut summary = format!("repo inspection failed: {error}");
    if let Some(target) = target {
        summary.push_str("; target: ");
        summary.push_str(target);
    }
    summary
}

fn probe_outcome_already_recorded(
    store: &ProjectStore,
    advice_id: &str,
) -> Result<bool, StoreError> {
    Ok(
        read_all_jsonl::<ActionOutcome>(&store.root().join("outcomes.jsonl"))?
            .into_iter()
            .any(|outcome| {
                outcome.advice_id == advice_id && outcome.action == ControlActionKind::RunProbe
            }),
    )
}

fn probe_outcome_for_run(advice: &AdviceRun, run: &ProbeRun) -> ActionOutcome {
    let mut evidence_ids = vec![run.probe_run_id.clone()];
    for evidence_id in &run.evidence_ids {
        if !evidence_ids.contains(evidence_id) {
            evidence_ids.push(evidence_id.clone());
        }
    }
    ActionOutcome {
        outcome_id: format!("outcome-{}", run.probe_run_id),
        advice_id: advice.advice_id.clone(),
        action: ControlActionKind::RunProbe,
        status: run.status,
        expected_entropy_delta: if advice.control_rationale.expected_entropy_delta.is_empty() {
            vec![EntropyDelta {
                kind: EntropyKind::Plan,
                delta: -20,
            }]
        } else {
            advice.control_rationale.expected_entropy_delta.clone()
        },
        observed_entropy_delta: vec![EntropyDelta {
            kind: EntropyKind::Plan,
            delta: match run.status {
                OutcomeStatus::Succeeded => -20,
                OutcomeStatus::Failed | OutcomeStatus::Unknown => 0,
            },
        }],
        observed_entropy_delta_evidence: Vec::new(),
        evidence_ids,
        requirement_ids: advice.control_rationale.requirement_ids.clone(),
        note: Some(run.summary.clone()),
    }
}
