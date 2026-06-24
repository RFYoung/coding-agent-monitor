use crate::{
    AcceptanceCoverageStatus, ActionOutcome, AdviceRun, CompletionCertificate,
    CompletionCertificateStatus, CompletionIncident, CompletionRepoAnchor, ControlActionKind,
    ControlCaseFile, OutcomeStatus, RepoHunkHistoryEntry, RepoTraceStatus,
    RequirementEvidenceNecessity, RequirementEvidenceRef, RequirementEvidenceRole, RequirementNode,
    RequirementSource, StoreError, TraceEntry, VerificationStatus, read_all_jsonl,
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::Path;

const REQUIREMENT_PROOF_MAX_REPO_HUNKS: usize = 5;
const REQUIREMENT_PROOF_MAX_TRACE_REFS: usize = 5;
const REQUIREMENT_PROOF_MAX_CONTROL_REFS: usize = 5;
const REQUIREMENT_PROOF_MAX_OUTCOME_REFS: usize = 5;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RequirementGraphQuery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<AcceptanceCoverageStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requirement_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_proof_score: Option<u8>,
    pub limit: usize,
}

impl Default for RequirementGraphQuery {
    fn default() -> Self {
        Self {
            status: None,
            requirement_id: None,
            text: None,
            max_proof_score: None,
            limit: 25,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RequirementGraphReport {
    pub workspace: String,
    pub case_file_count: usize,
    pub requirement_count: usize,
    pub requirements: Vec<RequirementNode>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub proofs: Vec<RequirementProofStep>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CompletionCertificateReport {
    pub workspace: String,
    pub case_file_count: usize,
    pub requirement_count: usize,
    pub certificate: CompletionCertificate,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requirement_proofs: Vec<RequirementProofStep>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub proof_gaps: Vec<CompletionRequirementProofGap>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CompletionRequirementProofGap {
    pub requirement_id: String,
    pub proof_score: u8,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gaps: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RequirementProofStep {
    pub requirement_id: String,
    #[serde(default)]
    pub source: RequirementSource,
    pub case_file_id: String,
    pub built_at: String,
    pub text: String,
    pub status: AcceptanceCoverageStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_status: Option<crate::VerificationStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_event_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence_refs: Vec<RequirementEvidenceRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub verifier_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub verifier_commands: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_verification_evidence_id: Option<String>,
    #[serde(default)]
    pub proof_strength: RequirementProofStrength,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trace_refs: Vec<RequirementTraceProofRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub repo_hunks: Vec<RequirementRepoHunkProofRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub control_refs: Vec<RequirementControlProofRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outcome_refs: Vec<RequirementOutcomeProofRef>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct RequirementProofStrength {
    pub score: u8,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub signals: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gaps: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RequirementTraceProofRef {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_id: Option<String>,
    pub file: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_end: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rationale: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub related_event_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requirement_ids: Vec<String>,
    #[serde(default)]
    pub necessity: RequirementEvidenceNecessity,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RequirementRepoHunkProofRef {
    pub history_id: String,
    pub path: String,
    pub hunk_index: usize,
    pub trace_status: RepoTraceStatus,
    pub matching_trace_count: usize,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trace_event_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub related_event_ids: Vec<String>,
    #[serde(default)]
    pub necessity: RequirementEvidenceNecessity,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RequirementControlProofRef {
    pub advice_id: String,
    pub action: ControlActionKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dispatch_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_agent: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub packet_evidence_refs: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requirement_ids: Vec<String>,
    #[serde(default)]
    pub necessity: RequirementEvidenceNecessity,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RequirementOutcomeProofRef {
    pub outcome_id: String,
    pub advice_id: String,
    pub action: ControlActionKind,
    pub status: OutcomeStatus,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requirement_ids: Vec<String>,
    #[serde(default)]
    pub necessity: RequirementEvidenceNecessity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

pub fn load_requirement_graph(
    workspace: impl AsRef<Path>,
    query: RequirementGraphQuery,
) -> Result<RequirementGraphReport, StoreError> {
    let workspace = workspace.as_ref();
    load_requirement_graph_from_store_root(
        workspace.join(".agent-monitor"),
        workspace.display().to_string(),
        query,
    )
}

pub fn load_completion_certificate_report(
    workspace: impl AsRef<Path>,
    query: RequirementGraphQuery,
) -> Result<CompletionCertificateReport, StoreError> {
    let workspace = workspace.as_ref();
    load_completion_certificate_report_from_store_root(
        workspace.join(".agent-monitor"),
        workspace.display().to_string(),
        query,
    )
}

pub(crate) fn load_completion_certificate_report_from_store_root(
    store_root: impl AsRef<Path>,
    workspace: String,
    query: RequirementGraphQuery,
) -> Result<CompletionCertificateReport, StoreError> {
    let store_root = store_root.as_ref();
    let graph = load_requirement_graph_from_store_root(store_root, workspace.clone(), query)?;
    let case_files = read_all_jsonl::<ControlCaseFile>(&store_root.join("case-files.jsonl"))?;
    let latest_case_file = case_files.last();
    let latest_proofs = latest_proofs_for_requirements(&graph);
    let proof_gaps = completion_requirement_proof_gaps(&latest_proofs);
    let certificate = completion_certificate_from_requirement_report(
        &graph,
        latest_case_file,
        &latest_proofs,
        &proof_gaps,
    );

    Ok(CompletionCertificateReport {
        workspace,
        case_file_count: graph.case_file_count,
        requirement_count: graph.requirement_count,
        certificate,
        requirement_proofs: latest_proofs,
        proof_gaps,
    })
}

pub(crate) fn load_requirement_graph_from_store_root(
    store_root: impl AsRef<Path>,
    workspace: String,
    query: RequirementGraphQuery,
) -> Result<RequirementGraphReport, StoreError> {
    let store_root = store_root.as_ref();
    let case_files = read_all_jsonl::<ControlCaseFile>(&store_root.join("case-files.jsonl"))?;
    let traces = read_all_jsonl::<TraceEntry>(&store_root.join("trace.jsonl"))?;
    let repo_hunks = read_all_jsonl::<RepoHunkHistoryEntry>(&store_root.join("repo-hunks.jsonl"))?;
    let advice = read_all_jsonl::<AdviceRun>(&store_root.join("advice.jsonl"))?;
    let outcomes = read_all_jsonl::<ActionOutcome>(&store_root.join("outcomes.jsonl"))?;
    let case_file_count = case_files.len();
    let mut seen = HashSet::new();
    let text_filter = query.text.as_ref().map(|text| text.to_ascii_lowercase());
    let requirement_id = query
        .requirement_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty());
    let mut requirements = Vec::new();

    for case_file in case_files.iter().rev() {
        for requirement in &case_file.requirements {
            if !seen.insert(requirement.requirement_id.clone()) {
                continue;
            }
            if requirement_id.is_some_and(|id| requirement.requirement_id != id) {
                continue;
            }
            if query
                .status
                .is_some_and(|status| requirement.status != status)
            {
                continue;
            }
            if text_filter.as_ref().is_some_and(|filter| {
                !requirement.text.to_ascii_lowercase().contains(filter)
                    && !requirement
                        .requirement_id
                        .to_ascii_lowercase()
                        .contains(filter)
            }) {
                continue;
            }
            if query.max_proof_score.is_some_and(|max| {
                requirement_proof_step(
                    case_file,
                    requirement,
                    &traces,
                    &repo_hunks,
                    &advice,
                    &outcomes,
                )
                .proof_strength
                .score
                    > max
            }) {
                continue;
            }
            requirements.push(requirement.clone());
        }
    }

    let requirement_count = requirements.len();
    requirements.truncate(query.limit);
    let returned_ids = requirements
        .iter()
        .map(|requirement| requirement.requirement_id.clone())
        .collect::<HashSet<_>>();
    let proofs = requirement_proofs_for_ids(
        &case_files,
        &traces,
        &repo_hunks,
        &advice,
        &outcomes,
        &returned_ids,
        query.limit,
    );

    Ok(RequirementGraphReport {
        workspace,
        case_file_count,
        requirement_count,
        requirements,
        proofs,
    })
}

fn latest_proofs_for_requirements(report: &RequirementGraphReport) -> Vec<RequirementProofStep> {
    let mut latest = Vec::new();
    for requirement in &report.requirements {
        if let Some(proof) = report
            .proofs
            .iter()
            .find(|proof| proof.requirement_id == requirement.requirement_id)
        {
            latest.push(proof.clone());
        }
    }
    latest
}

fn completion_requirement_proof_gaps(
    proofs: &[RequirementProofStep],
) -> Vec<CompletionRequirementProofGap> {
    proofs
        .iter()
        .filter(|proof| !proof.proof_strength.gaps.is_empty())
        .map(|proof| CompletionRequirementProofGap {
            requirement_id: proof.requirement_id.clone(),
            proof_score: proof.proof_strength.score,
            gaps: proof.proof_strength.gaps.clone(),
        })
        .collect()
}

fn completion_certificate_from_requirement_report(
    graph: &RequirementGraphReport,
    latest_case_file: Option<&ControlCaseFile>,
    latest_proofs: &[RequirementProofStep],
    proof_gaps: &[CompletionRequirementProofGap],
) -> CompletionCertificate {
    let scoped_requirement_ids = graph
        .requirements
        .iter()
        .map(|requirement| requirement.requirement_id.clone())
        .collect::<Vec<_>>();
    let closed_requirement_ids = graph
        .requirements
        .iter()
        .filter(|requirement| requirement_closed_for_completion(requirement, latest_proofs))
        .map(|requirement| requirement.requirement_id.clone())
        .collect::<Vec<_>>();
    let unresolved_requirement_ids = graph
        .requirements
        .iter()
        .filter(|requirement| !requirement_closed_for_completion(requirement, latest_proofs))
        .map(|requirement| requirement.requirement_id.clone())
        .collect::<Vec<_>>();

    let mut unresolved_incidents = Vec::new();
    if graph.requirements.is_empty() {
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
            evidence_ids: graph
                .requirements
                .iter()
                .filter(|requirement| requirement.status != AcceptanceCoverageStatus::Covered)
                .flat_map(|requirement| requirement.evidence_ids.clone())
                .collect(),
            missing_evidence: vec!["fresh evidence closing every scoped requirement".into()],
        });
    }
    if !proof_gaps.is_empty() {
        unresolved_incidents.push(CompletionIncident {
            kind: "proof_gap".into(),
            summary: format!(
                "proof gap remains for {} scoped requirement(s)",
                proof_gaps.len()
            ),
            evidence_ids: latest_proofs
                .iter()
                .flat_map(|proof| proof.evidence_ids.clone())
                .collect(),
            missing_evidence: proof_gaps.iter().flat_map(|gap| gap.gaps.clone()).collect(),
        });
    }

    let verification_status =
        completion_report_verification_status(&graph.requirements, latest_case_file);

    if let Some(case_file) = latest_case_file {
        unresolved_incidents.extend(
            case_file
                .completion_certificate
                .unresolved_incidents
                .iter()
                .filter(|incident| {
                    matches!(
                        incident.kind.as_str(),
                        "subagent_lifecycle" | "test_oracle_authority"
                    ) || (incident.kind == "verification"
                        && verification_status != VerificationStatus::Passed)
                })
                .cloned(),
        );
    }

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
        verification_status,
        verifier_commands: completion_report_verifier_commands(&graph.requirements),
        current_repo_anchor: latest_case_file
            .map(|case_file| CompletionRepoAnchor {
                git_head: case_file.replay.git_head.clone(),
                git_branch: case_file.replay.git_branch.clone(),
                git_dirty: case_file.replay.git_dirty,
            })
            .unwrap_or_default(),
        unresolved_workers: latest_case_file
            .map(|case_file| case_file.completion_certificate.unresolved_workers.clone())
            .unwrap_or_default(),
        unresolved_incidents,
        test_oracle_changes: latest_case_file
            .map(|case_file| case_file.completion_certificate.test_oracle_changes.clone())
            .unwrap_or_default(),
        confidence: if status == CompletionCertificateStatus::Eligible {
            90
        } else {
            70
        },
    }
}

fn completion_report_verification_status(
    requirements: &[RequirementNode],
    latest_case_file: Option<&ControlCaseFile>,
) -> VerificationStatus {
    if latest_case_file
        .is_some_and(|case_file| case_file.verification.status == VerificationStatus::Passed)
    {
        return VerificationStatus::Passed;
    }
    if requirements.is_empty() {
        return VerificationStatus::NotRun;
    }
    if requirements
        .iter()
        .any(|requirement| requirement.status == AcceptanceCoverageStatus::Failed)
    {
        return VerificationStatus::Failed;
    }
    if requirements
        .iter()
        .any(|requirement| requirement.status == AcceptanceCoverageStatus::Stale)
    {
        return VerificationStatus::Stale;
    }
    if requirements
        .iter()
        .all(|requirement| requirement.status == AcceptanceCoverageStatus::Covered)
    {
        return VerificationStatus::Passed;
    }
    VerificationStatus::NotRun
}

fn requirement_closed_for_completion(
    requirement: &RequirementNode,
    latest_proofs: &[RequirementProofStep],
) -> bool {
    match requirement.status {
        AcceptanceCoverageStatus::Covered => true,
        AcceptanceCoverageStatus::Failed | AcceptanceCoverageStatus::Stale => false,
        AcceptanceCoverageStatus::Unverified | AcceptanceCoverageStatus::Unmapped => latest_proofs
            .iter()
            .find(|proof| proof.requirement_id == requirement.requirement_id)
            .is_some_and(|proof| {
                proof.proof_strength.gaps.is_empty() && proof.proof_strength.score >= 80
            }),
    }
}

fn completion_report_verifier_commands(requirements: &[RequirementNode]) -> Vec<String> {
    let mut commands = Vec::new();
    for requirement in requirements {
        for command in &requirement.verifier_commands {
            push_unique(&mut commands, command);
        }
    }
    commands
}

fn requirement_proofs_for_ids(
    case_files: &[ControlCaseFile],
    traces: &[TraceEntry],
    repo_hunks: &[RepoHunkHistoryEntry],
    advice: &[AdviceRun],
    outcomes: &[ActionOutcome],
    requirement_ids: &HashSet<String>,
    limit: usize,
) -> Vec<RequirementProofStep> {
    let mut proofs = Vec::new();
    if requirement_ids.is_empty() || limit == 0 {
        return proofs;
    }

    for case_file in case_files.iter().rev() {
        for requirement in &case_file.requirements {
            if !requirement_ids.contains(&requirement.requirement_id) {
                continue;
            }
            proofs.push(requirement_proof_step(
                case_file,
                requirement,
                traces,
                repo_hunks,
                advice,
                outcomes,
            ));
            if proofs.len() >= limit {
                return proofs;
            }
        }
    }

    proofs
}

fn requirement_proof_step(
    case_file: &ControlCaseFile,
    requirement: &RequirementNode,
    traces: &[TraceEntry],
    repo_hunks: &[RepoHunkHistoryEntry],
    advice: &[AdviceRun],
    outcomes: &[ActionOutcome],
) -> RequirementProofStep {
    let trace_refs = trace_refs_for_requirement(requirement, traces);
    let repo_hunks = repo_hunk_refs_for_requirement(requirement, repo_hunks, &trace_refs);
    let control_refs = control_refs_for_requirement(requirement, &case_file.case_file_id, advice);
    let outcome_refs = outcome_refs_for_requirement(requirement, &control_refs, outcomes);
    let proof_strength = requirement_proof_strength(
        requirement,
        &trace_refs,
        &repo_hunks,
        &control_refs,
        &outcome_refs,
    );

    RequirementProofStep {
        requirement_id: requirement.requirement_id.clone(),
        source: requirement.source,
        case_file_id: case_file.case_file_id.clone(),
        built_at: case_file.built_at.clone(),
        text: requirement.text.clone(),
        status: requirement.status,
        latest_status: requirement.latest_status,
        source_event_id: requirement.source_event_id.clone(),
        evidence_ids: requirement.evidence_ids.clone(),
        evidence_refs: requirement.evidence_refs.clone(),
        verifier_ids: requirement.verifier_ids.clone(),
        verifier_commands: requirement.verifier_commands.clone(),
        latest_verification_evidence_id: requirement.latest_verification_evidence_id.clone(),
        proof_strength,
        trace_refs,
        repo_hunks,
        control_refs,
        outcome_refs,
    }
}

fn requirement_proof_strength(
    requirement: &RequirementNode,
    trace_refs: &[RequirementTraceProofRef],
    repo_hunks: &[RequirementRepoHunkProofRef],
    control_refs: &[RequirementControlProofRef],
    outcome_refs: &[RequirementOutcomeProofRef],
) -> RequirementProofStrength {
    let mut strength = RequirementProofStrength::default();

    if requirement.source_event_id.is_some() || !requirement.evidence_ids.is_empty() {
        add_proof_signal(&mut strength, 10, "source_evidence");
    } else {
        add_proof_gap(&mut strength, "no_source_evidence");
    }

    if requirement.latest_verification_evidence_id.is_some()
        && requirement.latest_status != Some(crate::VerificationStatus::NotRun)
    {
        add_proof_signal(&mut strength, 10, "latest_verification_evidence");
    }
    if requirement.latest_status == Some(crate::VerificationStatus::Passed) {
        add_proof_signal(&mut strength, 10, "latest_verification_passed");
    }

    let necessary_trace_refs = trace_refs
        .iter()
        .filter(|trace| trace.necessity == RequirementEvidenceNecessity::Necessary)
        .collect::<Vec<_>>();
    if trace_refs.is_empty() {
        add_proof_gap(&mut strength, "no_trace_refs");
    } else if necessary_trace_refs.is_empty() {
        add_proof_gap(&mut strength, "no_necessary_trace_refs");
    } else if necessary_trace_refs.iter().any(|trace| {
        trace
            .rationale
            .as_deref()
            .is_some_and(|text| !text.trim().is_empty())
    }) {
        add_proof_signal(&mut strength, 20, "direct_trace_rationale");
    } else {
        add_proof_signal(&mut strength, 10, "direct_trace_ref");
        add_proof_gap(&mut strength, "trace_refs_without_rationale");
    }

    let necessary_repo_hunks = repo_hunks
        .iter()
        .filter(|hunk| hunk.necessity == RequirementEvidenceNecessity::Necessary)
        .collect::<Vec<_>>();
    if repo_hunks.is_empty() {
        add_proof_gap(&mut strength, "no_repo_hunk_refs");
    } else if necessary_repo_hunks.is_empty() {
        add_proof_gap(&mut strength, "no_necessary_repo_hunk_refs");
    } else if necessary_repo_hunks
        .iter()
        .any(|hunk| hunk.trace_status == RepoTraceStatus::Traced)
    {
        add_proof_signal(&mut strength, 20, "repo_hunk_traced");
    } else if necessary_repo_hunks
        .iter()
        .any(|hunk| hunk.trace_status == RepoTraceStatus::MissingRationale)
    {
        add_proof_signal(&mut strength, 10, "repo_hunk_missing_rationale");
        add_proof_gap(&mut strength, "repo_hunk_missing_rationale");
    } else {
        add_proof_gap(&mut strength, "repo_hunk_untraced");
    }

    let necessary_control_refs = control_refs
        .iter()
        .filter(|control| control.necessity == RequirementEvidenceNecessity::Necessary)
        .collect::<Vec<_>>();
    if control_refs.is_empty() {
        add_proof_gap(&mut strength, "no_control_refs");
    } else if necessary_control_refs.is_empty() {
        add_proof_gap(&mut strength, "no_necessary_control_refs");
    } else {
        add_proof_signal(&mut strength, 15, "monitor_control_decision");
    }

    let necessary_outcome_refs = outcome_refs
        .iter()
        .filter(|outcome| outcome.necessity == RequirementEvidenceNecessity::Necessary)
        .collect::<Vec<_>>();
    if outcome_refs.is_empty() {
        add_proof_gap(&mut strength, "no_outcome_refs");
    } else if necessary_outcome_refs.is_empty() {
        add_proof_gap(&mut strength, "no_necessary_outcome_refs");
    } else if necessary_outcome_refs
        .iter()
        .any(|outcome| outcome.status == OutcomeStatus::Succeeded)
    {
        add_proof_signal(&mut strength, 20, "successful_outcome");
    } else if necessary_outcome_refs
        .iter()
        .any(|outcome| outcome.status == OutcomeStatus::Failed)
    {
        add_proof_gap(&mut strength, "failed_outcome");
    } else {
        add_proof_signal(&mut strength, 5, "non_terminal_outcome");
        add_proof_gap(&mut strength, "no_successful_outcome");
    }

    strength
}

fn add_proof_signal(strength: &mut RequirementProofStrength, score: u8, signal: &str) {
    strength.score = strength.score.saturating_add(score).min(100);
    push_unique(&mut strength.signals, signal);
}

fn add_proof_gap(strength: &mut RequirementProofStrength, gap: &str) {
    push_unique(&mut strength.gaps, gap);
}

fn trace_refs_for_requirement(
    requirement: &RequirementNode,
    traces: &[TraceEntry],
) -> Vec<RequirementTraceProofRef> {
    let evidence = requirement_evidence_profile(requirement);
    if evidence.all.is_empty() {
        return Vec::new();
    }

    let mut refs = Vec::new();
    for trace in traces.iter().rev() {
        let Some(necessity) = trace_match_necessity(&requirement.requirement_id, trace, &evidence)
        else {
            continue;
        };
        refs.push(RequirementTraceProofRef {
            event_id: trace.event_id.clone(),
            file: trace.file.clone(),
            agent: if trace.agent.trim().is_empty() {
                None
            } else {
                Some(trace.agent.clone())
            },
            session: trace.session.clone(),
            line: trace.line,
            line_end: trace.line_end,
            rationale: trace.rationale.clone(),
            related_event_ids: trace
                .related_event_ids
                .iter()
                .filter(|event_id| evidence.all.contains(*event_id))
                .cloned()
                .collect(),
            requirement_ids: trace
                .requirement_ids
                .iter()
                .filter(|requirement_id| *requirement_id == &requirement.requirement_id)
                .cloned()
                .collect(),
            necessity,
        });
        if refs.len() >= REQUIREMENT_PROOF_MAX_TRACE_REFS {
            break;
        }
    }

    refs
}

fn control_refs_for_requirement(
    requirement: &RequirementNode,
    case_file_id: &str,
    advice: &[AdviceRun],
) -> Vec<RequirementControlProofRef> {
    let evidence = requirement_evidence_profile(requirement);
    let mut refs = Vec::new();
    for advice in advice.iter().rev() {
        let advice_evidence = advice_evidence_ids(advice);
        let case_file_matches = advice.case_file_id == case_file_id;
        let requirement_match = advice
            .control_rationale
            .requirement_ids
            .iter()
            .any(|id| id == &requirement.requirement_id);
        let evidence_match = evidence_set_match_necessity(&advice_evidence, &evidence);
        if !case_file_matches && evidence_match.is_none() && !requirement_match {
            continue;
        }
        if case_file_matches
            && evidence_match.is_none()
            && !requirement_match
            && !advice_evidence.is_empty()
        {
            continue;
        }
        let necessity = if requirement_match {
            RequirementEvidenceNecessity::Necessary
        } else {
            evidence_match.unwrap_or(RequirementEvidenceNecessity::Necessary)
        };
        refs.push(RequirementControlProofRef {
            advice_id: advice.advice_id.clone(),
            action: advice.final_action.kind(),
            dispatch_id: non_empty_option(&advice.dispatch_result.dispatch_id),
            target_agent: non_empty_option(&advice.packet.target_agent),
            evidence_ids: filter_known_ids(&advice.control_rationale.evidence_ids, &evidence.all),
            packet_evidence_refs: filter_known_ids(&advice.packet.evidence_refs, &evidence.all),
            requirement_ids: filter_requirement_ids(
                &advice.control_rationale.requirement_ids,
                &requirement.requirement_id,
            ),
            necessity,
        });
        if refs.len() >= REQUIREMENT_PROOF_MAX_CONTROL_REFS {
            break;
        }
    }
    refs
}

fn outcome_refs_for_requirement(
    requirement: &RequirementNode,
    control_refs: &[RequirementControlProofRef],
    outcomes: &[ActionOutcome],
) -> Vec<RequirementOutcomeProofRef> {
    let evidence = requirement_evidence_profile(requirement);
    let mut refs = Vec::new();
    for outcome in outcomes.iter().rev() {
        let control_match = control_refs
            .iter()
            .filter(|control| control.advice_id == outcome.advice_id)
            .map(|control| control.necessity)
            .reduce(merge_necessity);
        let evidence_match = evidence_values_match_necessity(&outcome.evidence_ids, &evidence);
        let requirement_match = outcome
            .requirement_ids
            .iter()
            .any(|id| id == &requirement.requirement_id);
        let requirement_necessity =
            requirement_match.then_some(RequirementEvidenceNecessity::Necessary);
        let Some(necessity) = merge_optional_necessity(
            merge_optional_necessity(control_match, evidence_match),
            requirement_necessity,
        ) else {
            continue;
        };
        refs.push(RequirementOutcomeProofRef {
            outcome_id: outcome.outcome_id.clone(),
            advice_id: outcome.advice_id.clone(),
            action: outcome.action,
            status: outcome.status,
            evidence_ids: filter_known_ids(&outcome.evidence_ids, &evidence.all),
            requirement_ids: filter_requirement_ids(
                &outcome.requirement_ids,
                &requirement.requirement_id,
            ),
            necessity,
            note: outcome.note.clone(),
        });
        if refs.len() >= REQUIREMENT_PROOF_MAX_OUTCOME_REFS {
            break;
        }
    }
    refs
}

fn advice_evidence_ids(advice: &AdviceRun) -> HashSet<String> {
    let mut ids = HashSet::new();
    ids.extend(advice.control_rationale.evidence_ids.iter().cloned());
    ids.extend(advice.packet.evidence_refs.iter().cloned());
    ids
}

fn filter_known_ids(values: &[String], evidence_ids: &HashSet<String>) -> Vec<String> {
    values
        .iter()
        .filter(|id| evidence_ids.contains(*id))
        .cloned()
        .collect()
}

fn filter_requirement_ids(values: &[String], requirement_id: &str) -> Vec<String> {
    values
        .iter()
        .filter(|id| id.as_str() == requirement_id)
        .cloned()
        .collect()
}

fn non_empty_option(value: &str) -> Option<String> {
    if value.trim().is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn repo_hunk_refs_for_requirement(
    requirement: &RequirementNode,
    repo_hunks: &[RepoHunkHistoryEntry],
    trace_refs: &[RequirementTraceProofRef],
) -> Vec<RequirementRepoHunkProofRef> {
    let evidence = requirement_evidence_profile(requirement);
    if evidence.all.is_empty() && trace_refs.is_empty() {
        return Vec::new();
    }

    let mut refs = Vec::new();
    for hunk in repo_hunks.iter().rev() {
        let mut trace_event_ids = Vec::new();
        let mut related_event_ids = Vec::new();
        let mut hunk_necessity = None;
        for trace_ref in &hunk.matching_trace_refs {
            let Some(necessity) = merge_optional_necessity(
                trace_ref_match_necessity(trace_ref, &evidence),
                trace_ref_match_requirement_trace_necessity(trace_ref, trace_refs),
            ) else {
                continue;
            };
            hunk_necessity = Some(
                merge_optional_necessity(hunk_necessity, Some(necessity)).unwrap_or(necessity),
            );
            if let Some(event_id) = trace_ref.event_id.as_deref() {
                push_unique(&mut trace_event_ids, event_id);
            }
            for related_event_id in &trace_ref.related_event_ids {
                if evidence.all.contains(related_event_id) {
                    push_unique(&mut related_event_ids, related_event_id);
                }
            }
        }
        let Some(necessity) = hunk_necessity else {
            continue;
        };
        if trace_event_ids.is_empty() && related_event_ids.is_empty() {
            continue;
        }
        refs.push(RequirementRepoHunkProofRef {
            history_id: hunk.history_id.clone(),
            path: hunk.path.clone(),
            hunk_index: hunk.hunk_index,
            trace_status: hunk.trace_status,
            matching_trace_count: hunk.matching_trace_count,
            trace_event_ids,
            related_event_ids,
            necessity,
        });
        if refs.len() >= REQUIREMENT_PROOF_MAX_REPO_HUNKS {
            break;
        }
    }

    refs
}

fn trace_ref_match_requirement_trace_necessity(
    trace_ref: &crate::RepoHunkTraceRef,
    trace_refs: &[RequirementTraceProofRef],
) -> Option<RequirementEvidenceNecessity> {
    let mut matched = None;
    for requirement_trace in trace_refs {
        let Some(event_id) = requirement_trace.event_id.as_deref() else {
            continue;
        };
        let event_matches = trace_ref.event_id.as_deref() == Some(event_id)
            || trace_ref
                .related_event_ids
                .iter()
                .any(|related| related == event_id);
        if event_matches {
            matched = merge_optional_necessity(matched, Some(requirement_trace.necessity));
        }
    }
    matched
}

#[derive(Debug, Clone, Default)]
struct RequirementEvidenceProfile {
    all: HashSet<String>,
    necessary: HashSet<String>,
    correlated: HashSet<String>,
}

fn requirement_evidence_profile(requirement: &RequirementNode) -> RequirementEvidenceProfile {
    let mut profile = RequirementEvidenceProfile::default();
    if let Some(source_event_id) = &requirement.source_event_id {
        profile.all.insert(source_event_id.clone());
    }
    profile.all.extend(requirement.evidence_ids.iter().cloned());
    if let Some(latest_verification_evidence_id) = &requirement.latest_verification_evidence_id {
        profile.all.insert(latest_verification_evidence_id.clone());
    }

    if requirement.evidence_refs.is_empty() {
        profile.necessary = profile.all.clone();
        return profile;
    }

    for evidence in &requirement.evidence_refs {
        profile.all.insert(evidence.evidence_id.clone());
        let target = if evidence.necessity == RequirementEvidenceNecessity::Correlated
            || matches!(
                evidence.role,
                RequirementEvidenceRole::RequirementSource
                    | RequirementEvidenceRole::DurableMemorySource
            ) {
            &mut profile.correlated
        } else {
            &mut profile.necessary
        };
        target.insert(evidence.evidence_id.clone());
    }

    profile
}

fn trace_ref_match_necessity(
    trace_ref: &crate::RepoHunkTraceRef,
    evidence: &RequirementEvidenceProfile,
) -> Option<RequirementEvidenceNecessity> {
    let mut matches = Vec::new();
    if let Some(event_id) = trace_ref.event_id.as_deref() {
        matches.push(event_id);
    }
    matches.extend(trace_ref.related_event_ids.iter().map(String::as_str));
    match_necessity_for_ids(matches, evidence)
}

fn trace_match_necessity(
    requirement_id: &str,
    trace: &TraceEntry,
    evidence: &RequirementEvidenceProfile,
) -> Option<RequirementEvidenceNecessity> {
    if trace.requirement_ids.iter().any(|id| id == requirement_id) {
        return Some(RequirementEvidenceNecessity::Necessary);
    }
    let mut matches = Vec::new();
    if let Some(event_id) = trace.event_id.as_deref() {
        matches.push(event_id);
    }
    matches.extend(trace.related_event_ids.iter().map(String::as_str));
    match_necessity_for_ids(matches, evidence)
}

fn evidence_set_match_necessity(
    ids: &HashSet<String>,
    evidence: &RequirementEvidenceProfile,
) -> Option<RequirementEvidenceNecessity> {
    match_necessity_for_ids(ids.iter().map(String::as_str), evidence)
}

fn evidence_values_match_necessity(
    ids: &[String],
    evidence: &RequirementEvidenceProfile,
) -> Option<RequirementEvidenceNecessity> {
    match_necessity_for_ids(ids.iter().map(String::as_str), evidence)
}

fn match_necessity_for_ids<'a>(
    ids: impl IntoIterator<Item = &'a str>,
    evidence: &RequirementEvidenceProfile,
) -> Option<RequirementEvidenceNecessity> {
    let mut matched = None;
    for id in ids {
        if evidence.necessary.contains(id) {
            return Some(RequirementEvidenceNecessity::Necessary);
        }
        if evidence.correlated.contains(id) || evidence.all.contains(id) {
            matched = Some(RequirementEvidenceNecessity::Correlated);
        }
    }
    matched
}

fn merge_optional_necessity(
    left: Option<RequirementEvidenceNecessity>,
    right: Option<RequirementEvidenceNecessity>,
) -> Option<RequirementEvidenceNecessity> {
    match (left, right) {
        (Some(left), Some(right)) => Some(merge_necessity(left, right)),
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    }
}

fn merge_necessity(
    left: RequirementEvidenceNecessity,
    right: RequirementEvidenceNecessity,
) -> RequirementEvidenceNecessity {
    if left == RequirementEvidenceNecessity::Necessary
        || right == RequirementEvidenceNecessity::Necessary
    {
        RequirementEvidenceNecessity::Necessary
    } else {
        RequirementEvidenceNecessity::Correlated
    }
}

fn push_unique(values: &mut Vec<String>, value: &str) {
    if !values.iter().any(|existing| existing == value) {
        values.push(value.to_string());
    }
}
