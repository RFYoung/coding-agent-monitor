use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::{
    AcceptanceCoverageStatus, CaseFileReplay, DashboardSnapshot, EntropyKind, EntropyVector, Event,
    EventKind, RequirementNode, VerificationStatus, VerificationSummary,
};

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CompletionCertificateStatus {
    Eligible,
    #[default]
    Blocked,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct CompletionRepoAnchor {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_head: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_dirty: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CompletionWorkerGap {
    pub agent: String,
    pub count: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CompletionIncident {
    pub kind: String,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub missing_evidence: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CompletionTestOracleChange {
    pub file: String,
    pub authorized: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CompletionCertificate {
    pub status: CompletionCertificateStatus,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scoped_requirement_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub closed_requirement_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unresolved_requirement_ids: Vec<String>,
    #[serde(default)]
    pub verification_status: VerificationStatus,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub verifier_commands: Vec<String>,
    #[serde(default)]
    pub current_repo_anchor: CompletionRepoAnchor,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unresolved_workers: Vec<CompletionWorkerGap>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unresolved_incidents: Vec<CompletionIncident>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub test_oracle_changes: Vec<CompletionTestOracleChange>,
    pub confidence: u8,
}

impl Default for CompletionCertificate {
    fn default() -> Self {
        Self {
            status: CompletionCertificateStatus::Blocked,
            scoped_requirement_ids: Vec::new(),
            closed_requirement_ids: Vec::new(),
            unresolved_requirement_ids: Vec::new(),
            verification_status: VerificationStatus::NotRun,
            verifier_commands: Vec::new(),
            current_repo_anchor: CompletionRepoAnchor::default(),
            unresolved_workers: Vec::new(),
            unresolved_incidents: Vec::new(),
            test_oracle_changes: Vec::new(),
            confidence: 0,
        }
    }
}

pub(crate) fn build_completion_certificate(
    snapshot: &DashboardSnapshot,
    replay: &CaseFileReplay,
    verification: &VerificationSummary,
    requirements: &[RequirementNode],
    entropy: &EntropyVector,
) -> CompletionCertificate {
    let scoped_requirement_ids = requirements
        .iter()
        .map(|requirement| requirement.requirement_id.clone())
        .collect::<Vec<_>>();
    let closed_requirement_ids = requirements
        .iter()
        .filter(|requirement| requirement.status == AcceptanceCoverageStatus::Covered)
        .map(|requirement| requirement.requirement_id.clone())
        .collect::<Vec<_>>();
    let unresolved_requirement_ids = requirements
        .iter()
        .filter(|requirement| requirement.status != AcceptanceCoverageStatus::Covered)
        .map(|requirement| requirement.requirement_id.clone())
        .collect::<Vec<_>>();

    let mut unresolved_incidents = Vec::new();
    if requirements.is_empty() {
        unresolved_incidents.push(CompletionIncident {
            kind: "requirement_scope".into(),
            summary: "completion scope has no extracted requirements".into(),
            evidence_ids: Vec::new(),
            missing_evidence: vec![
                "extracted acceptance criteria or durable scoped requirements".into(),
            ],
        });
    }
    if !unresolved_requirement_ids.is_empty() {
        unresolved_incidents.push(CompletionIncident {
            kind: "requirement_closure".into(),
            summary: format!(
                "requirement closure is incomplete for {} scoped requirement(s)",
                unresolved_requirement_ids.len()
            ),
            evidence_ids: unresolved_requirement_evidence_ids(requirements),
            missing_evidence: vec!["fresh evidence closing every scoped requirement".into()],
        });
    }

    if verification_should_block_completion(verification, requirements, snapshot) {
        unresolved_incidents.push(CompletionIncident {
            kind: "verification".into(),
            summary: format!(
                "verification certificate is not fresh and passing: {:?}",
                verification.status
            ),
            evidence_ids: verification_incident_evidence_ids(entropy),
            missing_evidence: vec![
                "fresh passing verifier evidence for the current repo anchor".into(),
            ],
        });
    }

    let unresolved_workers = unresolved_worker_gaps(snapshot);
    if !unresolved_workers.is_empty() {
        unresolved_incidents.push(CompletionIncident {
            kind: "subagent_lifecycle".into(),
            summary: format!(
                "completion has {} unresolved spawned worker(s)",
                unresolved_workers
                    .iter()
                    .map(|worker| worker.count)
                    .sum::<usize>()
            ),
            evidence_ids: unresolved_workers
                .iter()
                .filter_map(|worker| worker.evidence_id.clone())
                .collect(),
            missing_evidence: vec![
                "joined_with_summary, cancelled_with_reason, timed_out, superseded, or failed worker outcome".into(),
            ],
        });
    }

    let test_oracle_changes = test_oracle_changes(snapshot);
    let unauthorized_oracle_changes = test_oracle_changes
        .iter()
        .filter(|change| !change.authorized)
        .collect::<Vec<_>>();
    if !unauthorized_oracle_changes.is_empty() {
        unresolved_incidents.push(CompletionIncident {
            kind: "test_oracle_authority".into(),
            summary: format!(
                "test oracle authority is missing for {} change(s)",
                unauthorized_oracle_changes.len()
            ),
            evidence_ids: unauthorized_oracle_changes
                .iter()
                .filter_map(|change| change.evidence_id.clone())
                .collect(),
            missing_evidence: vec![
                "spec authority and independent behavior evidence for test oracle changes".into(),
            ],
        });
    }

    unresolved_incidents.extend(entropy_completion_incidents(entropy));

    let status = if unresolved_incidents.is_empty() {
        CompletionCertificateStatus::Eligible
    } else {
        CompletionCertificateStatus::Blocked
    };

    CompletionCertificate {
        status,
        scoped_requirement_ids,
        closed_requirement_ids,
        unresolved_requirement_ids,
        verification_status: verification.status,
        verifier_commands: completion_verifier_commands(verification),
        current_repo_anchor: CompletionRepoAnchor {
            git_head: replay.git_head.clone(),
            git_branch: replay.git_branch.clone(),
            git_dirty: replay.git_dirty,
        },
        unresolved_workers,
        unresolved_incidents,
        test_oracle_changes,
        confidence: certificate_confidence(status),
    }
}

fn unresolved_requirement_evidence_ids(requirements: &[RequirementNode]) -> Vec<String> {
    let mut ids = Vec::new();
    for requirement in requirements
        .iter()
        .filter(|requirement| requirement.status != AcceptanceCoverageStatus::Covered)
    {
        if let Some(source_event_id) = requirement.source_event_id.as_deref() {
            push_unique(&mut ids, source_event_id);
        }
        for evidence_id in &requirement.evidence_ids {
            push_unique(&mut ids, evidence_id);
        }
    }
    ids
}

fn verification_should_block_completion(
    verification: &VerificationSummary,
    requirements: &[RequirementNode],
    snapshot: &DashboardSnapshot,
) -> bool {
    if verification.status == VerificationStatus::Passed {
        return false;
    }
    !requirements.is_empty()
        || !verification.changed_source_files.is_empty()
        || snapshot.recent_events.iter().any(|event| {
            event
                .content
                .as_deref()
                .is_some_and(looks_like_completion_claim)
        })
}

fn verification_incident_evidence_ids(entropy: &EntropyVector) -> Vec<String> {
    entropy
        .score(EntropyKind::Verification)
        .map(|score| score.evidence_ids.clone())
        .unwrap_or_default()
}

fn unresolved_worker_gaps(snapshot: &DashboardSnapshot) -> Vec<CompletionWorkerGap> {
    let mut workers = BTreeMap::<String, CompletionWorkerGap>::new();

    for (index, event) in snapshot.recent_events.iter().enumerate() {
        let evidence_id = event_evidence_id(event, index);
        let entry = workers
            .entry(event.agent.clone())
            .or_insert_with(|| CompletionWorkerGap {
                agent: event.agent.clone(),
                count: 0,
                evidence_id: None,
            });

        if event_records_subagent_spawn(event) {
            entry.count += 1;
            entry.evidence_id = Some(evidence_id);
        } else if event_records_subagent_terminal(event) {
            entry.count = entry.count.saturating_sub(1);
            if entry.count == 0 {
                entry.evidence_id = Some(evidence_id);
            }
        }
    }

    workers
        .into_values()
        .filter(|worker| worker.count > 0)
        .collect()
}

fn test_oracle_changes(snapshot: &DashboardSnapshot) -> Vec<CompletionTestOracleChange> {
    snapshot
        .recent_events
        .iter()
        .enumerate()
        .filter_map(|(index, event)| {
            let file = event.file.as_deref()?;
            if !is_test_oracle_file(file) || !event_records_authority_sensitive_test_change(event) {
                return None;
            }
            Some(CompletionTestOracleChange {
                file: file.into(),
                authorized: event_records_test_oracle_authority(event),
                evidence_id: Some(event_evidence_id(event, index)),
            })
        })
        .collect()
}

fn entropy_completion_incidents(entropy: &EntropyVector) -> Vec<CompletionIncident> {
    let mut incidents = Vec::new();
    for kind in [
        EntropyKind::Verification,
        EntropyKind::Goal,
        EntropyKind::Plan,
        EntropyKind::AgentHealth,
        EntropyKind::UserDecision,
    ] {
        let Some(score) = entropy.score(kind) else {
            continue;
        };
        if score.score < 80 {
            continue;
        }
        for cause in &score.top_causes {
            if cause_mentions_completion_block(cause) {
                incidents.push(CompletionIncident {
                    kind: entropy_kind_label(kind).into(),
                    summary: cause.clone(),
                    evidence_ids: score.evidence_ids.clone(),
                    missing_evidence: score.missing_evidence.clone(),
                });
            }
        }
    }
    incidents
}

fn cause_mentions_completion_block(cause: &str) -> bool {
    let text = cause.to_lowercase();
    text.contains("completion")
        || text.contains("verification")
        || text.contains("test oracle")
        || text.contains("spawned worker")
        || text.contains("unresolved ambiguity")
}

fn completion_verifier_commands(verification: &VerificationSummary) -> Vec<String> {
    let mut commands = Vec::new();
    if let Some(command) = verification.latest_passing_command.as_deref() {
        push_unique(&mut commands, command);
    }
    if let Some(command) = verification.latest_failing_command.as_deref() {
        push_unique(&mut commands, command);
    }
    for command in &verification.recommended_commands {
        push_unique(&mut commands, command);
    }
    commands
}

fn certificate_confidence(status: CompletionCertificateStatus) -> u8 {
    match status {
        CompletionCertificateStatus::Eligible => 90,
        CompletionCertificateStatus::Blocked => 70,
    }
}

fn event_records_subagent_spawn(event: &Event) -> bool {
    let text = event_text(event);
    text.contains("spawn_agent")
        || text.contains("spawned subagent")
        || text.contains("subagent started")
        || text.contains("tool command: task")
        || (event.kind == EventKind::ToolCall && text.contains("task "))
}

fn event_records_subagent_terminal(event: &Event) -> bool {
    let text = event_text(event);
    text.contains("joined_with_summary")
        || text.contains("cancelled_with_reason")
        || text.contains("timed_out")
        || text.contains("superseded")
        || text.contains("subagent stopped")
        || text.contains("subagent failed")
        || (event.kind == EventKind::ToolResult && text.contains("wait_agent"))
}

fn event_records_authority_sensitive_test_change(event: &Event) -> bool {
    let text = event_text(event);
    [
        "expected", "assert", "snapshot", "fixture", "golden", "baseline", "skip", "delete",
        "remove", "oracle",
    ]
    .iter()
    .any(|marker| text.contains(marker))
}

fn event_records_test_oracle_authority(event: &Event) -> bool {
    let text = event_text(event);
    [
        "spec authority",
        "accepted requirement",
        "authorized requirement",
        "user-authorized",
        "product requirement",
        "old oracle invalid",
        "changed requirement",
        "independent behavior evidence",
    ]
    .iter()
    .any(|marker| text.contains(marker))
}

fn is_test_oracle_file(path: &str) -> bool {
    let lower = normalized_path(path);
    lower.starts_with("test/")
        || lower.starts_with("tests/")
        || lower.contains("/test/")
        || lower.contains("/tests/")
        || lower.contains("fixture")
        || lower.contains("snapshot")
        || lower.ends_with(".snap")
}

fn looks_like_completion_claim(content: &str) -> bool {
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

fn event_text(event: &Event) -> String {
    let mut text = String::new();
    if let Some(command) = event.command.as_deref() {
        text.push_str(command);
        text.push('\n');
    }
    if let Some(content) = event.content.as_deref() {
        text.push_str(content);
        text.push('\n');
    }
    if let Some(rationale) = event.rationale.as_deref() {
        text.push_str(rationale);
    }
    text.to_lowercase()
}

fn normalized_path(path: &str) -> String {
    path.replace('\\', "/").to_lowercase()
}

fn event_evidence_id(event: &Event, index: usize) -> String {
    event
        .event_id
        .clone()
        .unwrap_or_else(|| format!("event-{}", index + 1))
}

fn entropy_kind_label(kind: EntropyKind) -> &'static str {
    match kind {
        EntropyKind::Goal => "goal",
        EntropyKind::Context => "context",
        EntropyKind::RepoBlame => "repo_blame",
        EntropyKind::Verification => "verification",
        EntropyKind::Plan => "plan",
        EntropyKind::AgentHealth => "agent_health",
        EntropyKind::UserDecision => "user_decision",
    }
}

fn push_unique(values: &mut Vec<String>, value: &str) {
    if !value.trim().is_empty() && !values.iter().any(|existing| existing == value) {
        values.push(value.to_string());
    }
}
