//! Coding-agent monitor: an event-sourced control plane that supervises coding
//! agents, preserves project intent, and keeps agent-generated changes
//! traceable. See `CLAUDE.md` for the operating contract.
//!
//! The control loop is `observe -> normalize -> build case file -> score
//! entropy -> select action -> validate -> compile packet -> execute -> measure`.
//! This crate root holds that loop's orchestration (case-file building,
//! requirement mapping, control-action validation, `advise_workspace`, handoff,
//! and decision trails); the supporting layers live in dedicated modules:
//!
//! - `model`: domain data types shared across the control plane.
//! - `store`: the append-only `.agent-monitor` JSONL persistence surface.
//! - `durable_memory` / `entropy`: memory loading and entropy scoring inputs.
//! - `control_calibration`: deterministic action selection and outcome calibration.
//! - `repo_audit`: git blame and change-audit plumbing.
//! - `jsonl` / `monitor` / `wrap`: streaming entrypoint, facade, and
//!   wrapped-agent supervision.
//! - `dashboard`: snapshot rendering. `util`: cross-cutting helpers.
//!
//! Adapter, advisor, config, requirement, and verifier concerns each have their
//! own module alongside these.

use serde::Serialize;
use serde::de::DeserializeOwned;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::str::FromStr;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

mod adapter_ingest;
mod adapters;
mod advisor_boundary;
mod advisor_client;
mod agent_review;
mod belief;
mod calibration;
mod completion;
mod config;
mod control_calibration;
mod dashboard;
mod dev_history;
mod durable_memory;
mod entropy;
mod events;
mod injection;
mod jsonl;
mod model;
mod monitor;
mod outcome_recording;
mod outcomes;
mod packet_dispatch;
mod probe;
mod redaction;
mod repo_audit;
mod repo_history;
mod requirements;
mod store;
mod util;
mod validation_surface;
mod verifier;
mod wrap;

pub use adapter_ingest::{
    AdapterHookDecision, AdapterHookResponse, adapter_hook_response, normalize_adapter_event,
};
use adapter_ingest::{
    adapter_event_name, adapter_ignored_event, adapter_ingest_warning_event,
    normalize_adapter_events,
};
use adapters::adapter_capabilities_from_config;
pub use adapters::{
    AgentKind, ParseAgentKindError, adapter_capabilities_for, adapter_capabilities_for_config,
    agent_kind_label,
};
pub use advisor_boundary::validate_advisor_decision;
use advisor_boundary::{bound_case_file_for_advisor, control_action_target_agents};
#[cfg(test)]
pub(crate) use advisor_client::post_json_http;
use advisor_client::request_advisor_decision;
pub use agent_review::{
    AgentReviewAction, AgentReviewFinding, AgentReviewReport, AgentReviewStatus,
    DemoWorkspaceError, RunningAgent, RunningProcess, create_demo_workspace, detect_running_agents,
    detect_running_agents_from_system, judge_snapshot,
};
use belief::build_belief_state;
pub use belief::{BeliefState, FailureHypothesisBelief, FailureHypothesisKind};
pub use calibration::{
    ActionCalibration, ActionTargetCalibration, CalibrationOutcomeSample, CalibrationQuery,
    CalibrationReport, load_calibration_report,
};
use completion::build_completion_certificate;
pub use completion::{
    CompletionCertificate, CompletionCertificateStatus, CompletionIncident, CompletionRepoAnchor,
    CompletionTestOracleChange, CompletionWorkerGap,
};
pub use config::{
    AdapterConfig, AdapterOverride, AdvisorConfig, AdvisorCredentialSource,
    AdvisorEndpointConfigUpdate, AdvisorProviderConfig, AdvisorProviderKind, LocalAgentConfig,
    LocalAgentConfigImportOptions, LocalClaudeCodeConfig, LocalCodexConfig, PolicyConfig,
    ProjectConfig, ProjectConfigError, ProjectConfigWriteError, RuntimeAuthConfig,
    RuntimeAuthStyle, SecurityConfig, VerificationScope, VerifierConfig,
    import_coding_plan_advisor_credentials, import_local_agent_configs,
    write_adapter_runtime_auth_config, write_advisor_endpoint_config, write_verifier_config,
};
pub(crate) use control_calibration::*;
pub(crate) use dashboard::*;
pub use dev_history::{
    DevHistoryAnalysisOptions, DevHistoryCount, DevHistoryError, DevHistoryFinding,
    DevHistoryRawCopyError, DevHistoryRawExportFile, DevHistoryRawExportIncluded,
    DevHistoryRawExportOptions, DevHistoryRawExportReport, DevHistoryRawMatchingRules,
    DevHistoryReport, DevHistorySourceReport, analyze_local_dev_history, export_raw_dev_history,
};
pub(crate) use durable_memory::*;
pub(crate) use entropy::*;
pub use events::{CapturedStream, Event, EventKind, command_output_event, command_result_event};
pub use injection::{
    InjectionFile, InjectionInstallError, InjectionPlan, InstallMode, injection_plan_for,
    injection_plan_for_workspace, install_agent_injection, install_injection_plan,
};
pub use jsonl::*;
pub use model::*;
pub use monitor::*;
pub use outcome_recording::record_event_outcome_for_latest_advice;
use outcome_recording::{record_immediate_outcome_for_advice, record_timed_out_handoff_outcomes};
pub use outcomes::{
    ActionOutcome, DecisionTrail, EntropyDelta, EntropyDeltaEvidence, OutcomeStatus,
};
use packet_dispatch::duplicate_urgent_packet_dispatch;
pub use probe::{ProbeError, ProbeRun, run_probe};
pub use redaction::RedactionStatus;
use redaction::{
    event_redaction_status, packet_text_is_tainted, sanitize_evidence_summary,
    storage_redacted_event, storage_redacted_trace, strongest_redaction_status,
};
pub use repo_audit::*;
pub use repo_history::{
    RepoHunkFileSummary, RepoHunkHistoryQuery, RepoHunkHistoryReport, load_repo_hunk_history,
    summarize_repo_hunk_files,
};
pub use requirements::{
    CompletionCertificateReport, CompletionRequirementProofGap, RequirementControlProofRef,
    RequirementGraphQuery, RequirementGraphReport, RequirementOutcomeProofRef,
    RequirementProofStep, RequirementProofStrength, RequirementRepoHunkProofRef,
    RequirementTraceProofRef, load_completion_certificate_report, load_requirement_graph,
};
pub use store::*;
pub(crate) use util::*;
use validation_surface::{
    ProjectValidationProfile, ValidationSurface, is_ui_validation_relevant_file,
    ordered_validation_surfaces, push_validation_surface, validation_surfaces_for_command,
    validation_surfaces_for_event,
};
pub use verifier::run_verifier;
pub use wrap::*;

pub fn build_control_case_file(
    workspace: impl AsRef<Path>,
    snapshot: &DashboardSnapshot,
) -> ControlCaseFile {
    build_control_case_file_with_config(workspace, snapshot, &ProjectConfig::default())
}

pub fn record_trace_entry(
    workspace: impl AsRef<Path>,
    mut entry: TraceEntry,
) -> Result<TraceEntry, StoreError> {
    if entry.time.is_none() {
        entry.time = current_utc_timestamp();
    }
    if entry.agent.trim().is_empty() {
        entry.agent = "monitor".into();
    }
    let entry = storage_redacted_trace(&entry);
    let mut store = ProjectStore::open(workspace)?;
    store.append_trace(&entry)?;
    Ok(entry)
}

pub fn promote_memory_candidate(
    workspace: impl AsRef<Path>,
    memory_id: &str,
    source: MemorySource,
) -> Result<MemoryCandidate, MemoryPromotionError> {
    if !source.is_trusted_promotion_source() {
        return Err(MemoryPromotionError::UntrustedSource {
            memory_source: source,
        });
    }

    let workspace = workspace.as_ref();
    let mut store = ProjectStore::open(workspace)?;
    let snapshot = DashboardSnapshot::load(store.root(), 500)?;
    let config = ProjectConfig::load(store.root())?;
    let case_file = build_control_case_file_with_config(workspace, &snapshot, &config);
    let candidate = case_file
        .memory_candidates
        .into_iter()
        .find(|candidate| candidate.memory_id == memory_id)
        .ok_or_else(|| MemoryPromotionError::CandidateNotFound {
            memory_id: memory_id.to_string(),
        })?;

    if let Some(existing) = latest_memory_record_for_id(workspace, memory_id) {
        return Err(MemoryPromotionError::AlreadyGoverned {
            memory_id: memory_id.to_string(),
            status: existing.status,
        });
    }

    if packet_text_is_tainted(&candidate.claim)
        || candidate
            .evidence_ids
            .iter()
            .any(|evidence_id| packet_text_is_tainted(evidence_id))
    {
        return Err(MemoryPromotionError::TaintedClaim {
            memory_id: memory_id.to_string(),
        });
    }

    if let Some(existing) = conflicting_active_memory_for_candidate(workspace, &candidate) {
        return Err(MemoryPromotionError::ConflictingClaim {
            memory_id: memory_id.to_string(),
            existing_memory_id: existing.memory_id,
        });
    }

    let promoted = MemoryCandidate {
        memory_id: candidate.memory_id,
        scope: candidate.scope,
        claim: candidate.claim,
        status: MemoryStatus::Active,
        source,
        evidence_ids: candidate.evidence_ids,
        confidence: promotion_confidence(candidate.confidence, source),
    };
    store.append_memory(&promoted)?;
    Ok(promoted)
}

fn latest_memory_record_for_id(workspace: &Path, memory_id: &str) -> Option<MemoryCandidate> {
    let path = workspace.join(".agent-monitor/memories.jsonl");
    read_durable_memory_records(&path)
        .0
        .into_iter()
        .filter(|(_, memory)| memory.memory_id == memory_id)
        .max_by_key(|(sequence, _)| *sequence)
        .map(|(_, memory)| memory)
}

fn conflicting_active_memory_for_candidate(
    workspace: &Path,
    candidate: &MemoryCandidate,
) -> Option<MemoryCandidate> {
    let path = workspace.join(".agent-monitor/memories.jsonl");
    latest_active_durable_memory_records(&path)
        .into_iter()
        .map(|(_, memory)| memory)
        .find(|memory| {
            memory.memory_id != candidate.memory_id && memory_claims_conflict(memory, candidate)
        })
}

fn promotion_confidence(candidate_confidence: u8, source: MemorySource) -> u8 {
    let floor = match source {
        MemorySource::VerifiedResult => 95,
        MemorySource::User | MemorySource::ManualReview => 90,
        MemorySource::AgentClaim => 0,
    };
    candidate_confidence.max(floor).min(100)
}

pub fn build_control_case_file_with_config(
    workspace: impl AsRef<Path>,
    snapshot: &DashboardSnapshot,
    config: &ProjectConfig,
) -> ControlCaseFile {
    // Keep this assembly order evidence-first: objective repo/store evidence is
    // loaded before entropy scoring, and advisor-visible capabilities are
    // derived only after config-level redaction/validation has run.
    let workspace = workspace.as_ref();
    let intent_events = case_file_intent_events(workspace, snapshot);
    let repo_audit = load_repo_audit(workspace).ok();
    let mut evidence = evidence_from_snapshot(snapshot);
    if let Some(repo_audit) = &repo_audit {
        evidence.extend(evidence_from_repo_audit(repo_audit));
    }
    let memory_candidates = memory_candidates_from_snapshot(snapshot);
    let durable_memory_load = load_durable_memory(workspace);
    evidence.extend(durable_memory_load.warnings);
    let project_contract_scope = project_contract_requirements(workspace);
    evidence.extend(evidence_from_project_contract_requirements(
        &project_contract_scope,
    ));
    let verification = verification_summary(
        snapshot,
        &intent_events,
        &config.verifiers,
        &config.policy,
        repo_audit.as_ref(),
    );
    let task = task_summary_from_intent_events(&intent_events, &verification);
    let validation_profile = ProjectValidationProfile::discover(workspace, &config.verifiers);
    let entropy = score_entropy(EntropyScoringInput {
        snapshot,
        intent_events: &intent_events,
        task: &task,
        verification: &verification,
        durable_memory: &durable_memory_load.memories,
        repo_audit: repo_audit.as_ref(),
        validation_profile: &validation_profile,
        policy: &config.policy,
        security: &config.security,
    });
    let durable_memory = durable_memory_load.memories;
    let latest_verification_status = verification.status;
    let adapter_capabilities = adapter_capabilities_from_config(&config.adapters);
    let (allowed_actions, forbidden_actions) =
        control_action_bounds_for_capabilities(&adapter_capabilities);
    let mut requirements =
        requirement_nodes_from_verification(&intent_events, snapshot, &verification);
    requirements.extend(requirement_nodes_from_durable_memory(&durable_memory));
    requirements.extend(requirement_nodes_from_project_contract(
        &project_contract_scope,
        snapshot,
        &config.verifiers,
        verification.status,
    ));
    let replay = case_file_replay(workspace, snapshot);
    let completion_certificate =
        build_completion_certificate(snapshot, &replay, &verification, &requirements, &entropy);
    let belief_state = build_belief_state(
        &entropy,
        &verification,
        &requirements,
        &completion_certificate,
    );
    ControlCaseFile {
        case_file_id: format!("case-{}", current_id_fragment()),
        built_at: current_utc_timestamp().unwrap_or_else(|| "1970-01-01T00:00:00Z".into()),
        workspace: workspace.display().to_string(),
        replay,
        task,
        severity: snapshot.severity,
        event_count: snapshot.event_count,
        intervention_count: snapshot.intervention_count,
        active_agents: snapshot.active_agents.clone(),
        evidence,
        entropy,
        belief_state,
        allowed_actions,
        forbidden_actions,
        latest_verification_status,
        verification,
        requirements,
        completion_certificate,
        memory_candidates,
        durable_memory,
        adapter_capabilities,
        repo_audit,
    }
}

fn case_file_replay(workspace: &Path, snapshot: &DashboardSnapshot) -> CaseFileReplay {
    CaseFileReplay {
        git_head: current_git_head(workspace),
        git_branch: current_git_branch(workspace),
        git_dirty: current_git_dirty(workspace),
        input: CaseFileInputBoundary {
            event_count: snapshot.event_count,
            max_event_seq: snapshot
                .recent_events
                .iter()
                .filter_map(|event| event.seq)
                .max(),
            intervention_count: snapshot.intervention_count,
            verifier_run_count: snapshot.verifier_run_count,
            probe_run_count: snapshot.probe_run_count,
            repo_hunk_history_count: snapshot.repo_hunk_history_count,
            repo_hunk_file_count: snapshot.repo_hunk_file_count,
            requirement_count: snapshot.requirement_count,
            dev_history_count: snapshot.dev_history_count,
            advice_count: snapshot.advice_count,
            packet_count: snapshot.packet_count,
            dispatch_count: snapshot.dispatch_count,
            outcome_count: snapshot.outcome_count,
            lock_event_count: snapshot.lock_event_count,
        },
    }
}

fn case_file_intent_events(workspace: &Path, snapshot: &DashboardSnapshot) -> Vec<Event> {
    let mut events = Vec::new();
    let mut seen_ids = HashSet::new();
    if let Ok(stored_events) =
        read_all_jsonl::<Event>(&workspace.join(".agent-monitor").join("events.jsonl"))
    {
        for event in stored_events {
            if event.kind == EventKind::UserInstruction {
                push_intent_event(&mut events, &mut seen_ids, event);
            }
        }
    }
    for event in &snapshot.recent_events {
        if event.kind == EventKind::UserInstruction {
            push_intent_event(&mut events, &mut seen_ids, event.clone());
        }
    }
    events
}

fn push_intent_event(events: &mut Vec<Event>, seen_ids: &mut HashSet<String>, event: Event) {
    if let Some(event_id) = event.event_id.as_deref()
        && !seen_ids.insert(event_id.to_string())
    {
        return;
    }
    events.push(event);
}

fn task_summary_from_intent_events(
    intent_events: &[Event],
    verification: &VerificationSummary,
) -> TaskSummary {
    let mut summary = TaskSummary::default();

    for (index, event) in intent_events.iter().enumerate() {
        if event.kind != EventKind::UserInstruction {
            continue;
        }
        let Some(content) = event.content.as_deref() else {
            continue;
        };
        if content.trim().is_empty() {
            continue;
        }

        let event_id = event_evidence_id(event, index);
        summary.user_goal = Some(sanitize_task_text(content));
        summary.user_goal_event_id = Some(event_id.clone());

        if looks_like_goal_ambiguity(content) {
            summary.ambiguity_markers.push(TaskAmbiguityMarker {
                text: sanitize_task_text(content),
                source_event_id: Some(event_id),
            });
        }
    }

    let acceptance_sources = acceptance_criteria_source_events(intent_events);
    for criterion in &verification.acceptance_criteria {
        let text = sanitize_task_text(criterion);
        if text.is_empty() {
            continue;
        }
        let source_event_id = acceptance_sources.get(criterion).cloned();
        let id = format!(
            "ac-{}-{}",
            source_event_id
                .as_deref()
                .map(safe_slug)
                .unwrap_or_else(|| safe_slug(&text)),
            summary.acceptance_criteria.len() + 1
        );
        summary.acceptance_criteria.push(TaskAcceptanceCriterion {
            id,
            text,
            source_event_id,
            confidence: 82,
        });
    }

    summary
}

fn event_evidence_id(event: &Event, index: usize) -> String {
    event
        .event_id
        .clone()
        .unwrap_or_else(|| format!("event-{}", index + 1))
}

fn sanitize_task_text(text: &str) -> String {
    let (sanitized, _) = sanitize_evidence_summary(text);
    truncate_evidence(&sanitized)
}

fn looks_like_goal_ambiguity(content: &str) -> bool {
    let text = content.to_lowercase();
    [
        "ambiguous requirement",
        "unclear requirement",
        "not sure",
        "maybe we should",
        "choose between",
        "which provider",
        "which endpoint",
        "which model",
        "which agent",
        "which one",
        "what should be default",
        "should be default",
        "product decision",
        "user preference",
    ]
    .iter()
    .any(|signal| text.contains(signal))
}

fn task_suggests_bug_fix(task: &TaskSummary) -> bool {
    let mut text = String::new();
    if let Some(goal) = &task.user_goal {
        text.push_str(goal);
        text.push(' ');
    }
    for criterion in &task.acceptance_criteria {
        text.push_str(&criterion.text);
        text.push(' ');
    }
    let text = text.to_ascii_lowercase();
    let repair_signal = [
        "fix ",
        "debug ",
        "repair ",
        "resolve ",
        "regression",
        "bug",
        "defect",
    ]
    .iter()
    .any(|signal| text.contains(signal));
    let failure_signal = [
        "failing",
        "failure",
        "broken",
        "error",
        "crash",
        "not working",
        "regression",
        "bug",
        "defect",
        "500",
    ]
    .iter()
    .any(|signal| text.contains(signal));
    repair_signal && failure_signal
}

fn requirement_nodes_from_verification(
    intent_events: &[Event],
    snapshot: &DashboardSnapshot,
    verification: &VerificationSummary,
) -> Vec<RequirementNode> {
    let sources = acceptance_criteria_source_events(intent_events);
    let verification_evidence = latest_verification_evidence_by_key(snapshot);
    verification
        .acceptance_coverage
        .iter()
        .map(|coverage| {
            let source_event_id = sources.get(&coverage.criterion).cloned();
            let latest_verification_evidence_id =
                latest_requirement_verification_evidence(coverage, &verification_evidence);
            let mut evidence_ids = source_event_id.iter().cloned().collect::<Vec<_>>();
            let mut evidence_refs = Vec::new();
            if let Some(evidence_id) = &source_event_id {
                evidence_refs.push(requirement_evidence_ref(
                    evidence_id,
                    RequirementEvidenceRole::RequirementSource,
                ));
            }
            if let Some(evidence_id) = &latest_verification_evidence_id
                && !evidence_ids.contains(evidence_id)
            {
                evidence_ids.push(evidence_id.clone());
                evidence_refs.push(requirement_evidence_ref(
                    evidence_id,
                    RequirementEvidenceRole::VerificationResult,
                ));
            }
            RequirementNode {
                requirement_id: requirement_id_for_text(&coverage.criterion),
                source: RequirementSource::AcceptanceCriterion,
                text: coverage.criterion.clone(),
                evidence_ids,
                evidence_refs,
                source_event_id,
                verifier_ids: coverage.verifier_ids.clone(),
                verifier_commands: coverage.verifier_commands.clone(),
                latest_verification_evidence_id,
                status: coverage.status,
                latest_status: coverage.latest_status,
            }
        })
        .collect()
}

fn requirement_nodes_from_durable_memory(memories: &[MemoryCandidate]) -> Vec<RequirementNode> {
    memories
        .iter()
        .map(|memory| {
            let latest_verification_evidence_id = if memory.source == MemorySource::VerifiedResult {
                memory.evidence_ids.first().cloned()
            } else {
                None
            };
            RequirementNode {
                requirement_id: requirement_id_for_memory(memory),
                source: RequirementSource::DurableMemory,
                text: memory.claim.clone(),
                source_event_id: memory.evidence_ids.first().cloned(),
                evidence_ids: memory.evidence_ids.clone(),
                evidence_refs: requirement_evidence_refs_for_memory(memory),
                verifier_ids: Vec::new(),
                verifier_commands: Vec::new(),
                latest_verification_evidence_id,
                status: AcceptanceCoverageStatus::Covered,
                latest_status: if memory.source == MemorySource::VerifiedResult {
                    Some(VerificationStatus::Passed)
                } else {
                    None
                },
            }
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProjectContractRequirement {
    evidence_id: String,
    text: String,
    source_path: String,
    line: u64,
    source_hash: String,
}

fn project_contract_requirements(workspace: &Path) -> Vec<ProjectContractRequirement> {
    let mut requirements = Vec::new();
    let mut seen = HashSet::new();
    for relative_path in ["AGENTS.md", "CLAUDE.md"] {
        let path = workspace.join(relative_path);
        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };
        let source_hash = fnv1a64_digest(content.as_bytes());
        for (line, text) in extract_project_contract_items(&content) {
            let text = clean_acceptance_criterion(&text);
            if text.is_empty() {
                continue;
            }
            let key = normalize_project_contract_requirement(&text);
            if !seen.insert(key) {
                continue;
            }
            requirements.push(ProjectContractRequirement {
                evidence_id: format!("project-contract-{}-{}", safe_slug(relative_path), line),
                text,
                source_path: relative_path.into(),
                line: line as u64,
                source_hash: source_hash.clone(),
            });
        }
    }
    requirements
}

fn extract_project_contract_items(content: &str) -> Vec<(usize, String)> {
    let mut items = Vec::new();
    let mut in_contract_section = false;
    let mut contract_heading_level = 0usize;

    for (index, line) in content.lines().enumerate() {
        let line_number = index + 1;
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if let Some((level, title)) = markdown_heading(trimmed) {
            if in_contract_section && level <= contract_heading_level {
                in_contract_section = false;
            }
            if is_project_contract_heading(title) {
                in_contract_section = true;
                contract_heading_level = level;
            }
            continue;
        }

        if !in_contract_section {
            continue;
        }

        if let Some(item) = acceptance_block_item(trimmed) {
            items.push((line_number, item.to_string()));
        }
    }

    items
}

fn markdown_heading(line: &str) -> Option<(usize, &str)> {
    let hashes = line.chars().take_while(|ch| *ch == '#').count();
    if hashes == 0 {
        return None;
    }
    let title = line.get(hashes..)?.trim();
    if title.is_empty() {
        None
    } else {
        Some((hashes, title))
    }
}

fn is_project_contract_heading(title: &str) -> bool {
    title
        .trim()
        .eq_ignore_ascii_case("Non-Negotiable Invariants")
}

fn normalize_project_contract_requirement(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

fn requirement_nodes_from_project_contract(
    requirements: &[ProjectContractRequirement],
    snapshot: &DashboardSnapshot,
    verifiers: &[VerifierConfig],
    verification_status: VerificationStatus,
) -> Vec<RequirementNode> {
    let criteria = requirements
        .iter()
        .map(|requirement| requirement.text.clone())
        .collect::<Vec<_>>();
    let coverage_by_criterion =
        acceptance_coverage_for_criteria(&criteria, verifiers, snapshot, verification_status)
            .into_iter()
            .map(|coverage| (coverage.criterion.clone(), coverage))
            .collect::<HashMap<_, _>>();
    let verification_evidence = latest_verification_evidence_by_key(snapshot);
    requirements
        .iter()
        .map(|requirement| {
            let coverage = coverage_by_criterion.get(&requirement.text);
            let latest_verification_evidence_id = coverage.and_then(|coverage| {
                latest_requirement_verification_evidence(coverage, &verification_evidence)
            });
            let mut evidence_ids = vec![requirement.evidence_id.clone()];
            let mut evidence_refs = vec![requirement_evidence_ref(
                &requirement.evidence_id,
                RequirementEvidenceRole::RequirementSource,
            )];
            if let Some(evidence_id) = &latest_verification_evidence_id
                && !evidence_ids.contains(evidence_id)
            {
                evidence_ids.push(evidence_id.clone());
                evidence_refs.push(requirement_evidence_ref(
                    evidence_id,
                    RequirementEvidenceRole::VerificationResult,
                ));
            }
            RequirementNode {
                requirement_id: format!(
                    "req-contract-{}",
                    safe_slug(&clean_acceptance_criterion(&requirement.text).to_ascii_lowercase())
                ),
                source: RequirementSource::ProjectContract,
                text: requirement.text.clone(),
                source_event_id: Some(requirement.evidence_id.clone()),
                evidence_ids,
                evidence_refs,
                verifier_ids: coverage
                    .map(|coverage| coverage.verifier_ids.clone())
                    .unwrap_or_default(),
                verifier_commands: coverage
                    .map(|coverage| coverage.verifier_commands.clone())
                    .unwrap_or_default(),
                latest_verification_evidence_id,
                status: coverage
                    .map(|coverage| coverage.status)
                    .unwrap_or(AcceptanceCoverageStatus::Unmapped),
                latest_status: coverage.and_then(|coverage| coverage.latest_status),
            }
        })
        .collect()
}

fn requirement_evidence_ref(
    evidence_id: &str,
    role: RequirementEvidenceRole,
) -> RequirementEvidenceRef {
    RequirementEvidenceRef {
        evidence_id: evidence_id.to_string(),
        role,
        necessity: RequirementEvidenceNecessity::Necessary,
    }
}

fn requirement_evidence_refs_for_memory(memory: &MemoryCandidate) -> Vec<RequirementEvidenceRef> {
    let mut refs = memory
        .evidence_ids
        .iter()
        .map(|evidence_id| {
            requirement_evidence_ref(evidence_id, RequirementEvidenceRole::DurableMemorySource)
        })
        .collect::<Vec<_>>();
    if memory.source == MemorySource::VerifiedResult
        && let Some(evidence_id) = memory.evidence_ids.first()
    {
        refs.push(requirement_evidence_ref(
            evidence_id,
            RequirementEvidenceRole::VerificationResult,
        ));
    }
    refs
}

fn requirement_id_for_memory(memory: &MemoryCandidate) -> String {
    format!(
        "req-memory-{}",
        safe_slug(&clean_acceptance_criterion(&memory.claim).to_ascii_lowercase())
    )
}

fn requirement_id_for_text(text: &str) -> String {
    format!(
        "req-{}",
        safe_slug(&clean_acceptance_criterion(text).to_ascii_lowercase())
    )
}

fn acceptance_criteria_source_events(intent_events: &[Event]) -> HashMap<String, String> {
    let mut sources = HashMap::new();
    for (index, event) in intent_events.iter().enumerate() {
        if event.kind != EventKind::UserInstruction {
            continue;
        }
        let Some(content) = event.content.as_deref() else {
            continue;
        };
        let evidence_id = event_evidence_id(event, index);
        for criterion in extract_acceptance_criteria(content) {
            sources
                .entry(criterion)
                .or_insert_with(|| evidence_id.clone());
        }
    }
    sources
}

fn latest_requirement_verification_evidence(
    coverage: &AcceptanceCoverage,
    latest: &HashMap<String, ((i64, usize), String)>,
) -> Option<String> {
    coverage
        .verifier_ids
        .iter()
        .chain(coverage.verifier_commands.iter())
        .filter_map(|key| latest.get(&normalize_command_signature(key)))
        .max_by_key(|(order, _)| *order)
        .map(|(_, evidence_id)| evidence_id.clone())
}

fn latest_verification_evidence_by_key(
    snapshot: &DashboardSnapshot,
) -> HashMap<String, ((i64, usize), String)> {
    let mut latest = HashMap::<String, ((i64, usize), String)>::new();
    for (index, event) in snapshot.recent_events.iter().enumerate() {
        if !event_is_verification_result(event) {
            continue;
        }
        let Some(command) = event.command.as_deref() else {
            continue;
        };
        let Some(time) = event.time.as_deref().and_then(parse_utc_seconds) else {
            continue;
        };
        let evidence_id = event
            .event_id
            .clone()
            .unwrap_or_else(|| format!("event-{}", index + 1));
        update_latest_verification_evidence(
            &mut latest,
            normalize_command_signature(command),
            (time, index),
            evidence_id,
        );
    }

    let verifier_sequence_base = snapshot.recent_events.len();
    for (index, run) in snapshot.recent_verifier_runs.iter().enumerate() {
        let Some(time) = verifier_run_time(run) else {
            continue;
        };
        let order = (time, verifier_sequence_base + index);
        update_latest_verification_evidence(
            &mut latest,
            normalize_command_signature(&run.command),
            order,
            run.verifier_run_id.clone(),
        );
        if let Some(verifier_id) = run.verifier_id.as_deref() {
            update_latest_verification_evidence(
                &mut latest,
                normalize_command_signature(verifier_id),
                order,
                run.verifier_run_id.clone(),
            );
        }
    }
    latest
}

fn update_latest_verification_evidence(
    latest: &mut HashMap<String, ((i64, usize), String)>,
    key: String,
    order: (i64, usize),
    evidence_id: String,
) {
    if key.trim().is_empty() {
        return;
    }
    if latest
        .get(&key)
        .is_none_or(|(current_order, _)| order > *current_order)
    {
        latest.insert(key, (order, evidence_id));
    }
}

fn control_action_bounds_for_capabilities(
    adapter_capabilities: &BTreeMap<String, AdapterCapabilities>,
) -> (Vec<ControlActionKind>, Vec<ForbiddenAction>) {
    let mut allowed_actions = vec![
        ControlActionKind::ContinueWorking,
        ControlActionKind::RetryAgent,
        ControlActionKind::ForceVerification,
        ControlActionKind::RunProbe,
        ControlActionKind::SendFollowUp,
        ControlActionKind::SpawnJudgeAgent,
        ControlActionKind::SpawnFreshAgent,
        ControlActionKind::SwitchAgent,
        ControlActionKind::AskUser,
    ];
    let mut forbidden_actions = Vec::new();
    if !adapter_capabilities
        .values()
        .any(adapter_capability_allows_readonly_judge)
    {
        allowed_actions.retain(|action| *action != ControlActionKind::SpawnJudgeAgent);
        forbidden_actions.push(ForbiddenAction {
            action: ControlActionKind::SpawnJudgeAgent,
            reason: "no adapter capability allows a read-only judge packet target".into(),
        });
    }
    if !adapter_capabilities
        .values()
        .any(adapter_capability_allows_writable_handoff)
    {
        allowed_actions.retain(|action| {
            !matches!(
                action,
                ControlActionKind::SpawnFreshAgent | ControlActionKind::SwitchAgent
            )
        });
        let reason = "no adapter capability allows a writable handoff target".to_string();
        forbidden_actions.push(ForbiddenAction {
            action: ControlActionKind::SpawnFreshAgent,
            reason: reason.clone(),
        });
        forbidden_actions.push(ForbiddenAction {
            action: ControlActionKind::SwitchAgent,
            reason,
        });
    }
    (allowed_actions, forbidden_actions)
}

pub fn validate_control_action(
    proposed: ControlAction,
    case_file: &ControlCaseFile,
) -> ControlAction {
    action_from_validation_outcome(&validate_control_action_detailed(proposed, case_file))
}

pub fn validate_control_action_detailed(
    proposed: ControlAction,
    case_file: &ControlCaseFile,
) -> ValidationOutcome {
    if let ControlAction::RetryAgent {
        target_agent,
        max_attempts,
    } = &proposed
    {
        if !retry_agent_entropy_allowed(case_file) {
            return ValidationOutcome::Modified {
                original: proposed.clone(),
                replacement: deterministic_control_action(case_file),
                reason: "retry_agent denied because agent-health entropy is not high enough".into(),
            };
        }
        let replacement_target =
            agent_for_entropy(case_file, EntropyKind::AgentHealth).or_else(|| target_agent.clone());
        let replacement_attempts = (*max_attempts).clamp(1, 3);
        if replacement_target != *target_agent || replacement_attempts != *max_attempts {
            return ValidationOutcome::Modified {
                original: proposed.clone(),
                replacement: ControlAction::RetryAgent {
                    target_agent: replacement_target,
                    max_attempts: replacement_attempts,
                },
                reason: "retry_agent normalized to safe target and attempt range".into(),
            };
        }
    }

    if let ControlAction::SwitchAgent { target_agent } = &proposed
        && let Some(unhealthy_agent) = agent_for_entropy(case_file, EntropyKind::AgentHealth)
        && target_agent == &unhealthy_agent
    {
        let replacement = fallback_agent_for_case_file(case_file, Some(&unhealthy_agent))
            .map(|target_agent| ControlAction::SwitchAgent { target_agent })
            .unwrap_or_else(|| ControlAction::Pause {
                reason: "no adapter capability allows a writable handoff target".into(),
            });
        let reason = "switch_agent target replaced because it matched the unhealthy agent";
        if let Some(entropy_reason) = high_cost_handoff_entropy_reason(&replacement, case_file) {
            return ValidationOutcome::Modified {
                original: proposed.clone(),
                replacement: deterministic_control_action(case_file),
                reason: format!("{reason}; {entropy_reason}"),
            };
        }
        return ValidationOutcome::Modified {
            original: proposed.clone(),
            replacement,
            reason: reason.into(),
        };
    }

    if let Some(outcome) = validate_writable_handoff_target(&proposed, case_file) {
        if let ValidationOutcome::Modified {
            replacement,
            reason: target_reason,
            ..
        } = &outcome
            && let Some(entropy_reason) = high_cost_handoff_entropy_reason(replacement, case_file)
        {
            return ValidationOutcome::Modified {
                original: proposed,
                replacement: deterministic_control_action(case_file),
                reason: format!("{target_reason}; {entropy_reason}"),
            };
        }
        return outcome;
    }

    if let Some(outcome) = validate_active_writer_handoff(&proposed, case_file) {
        return outcome;
    }

    if verification_first_action_is_required(&proposed, case_file) {
        return ValidationOutcome::Modified {
            original: proposed,
            replacement: force_verification_control_action(case_file),
            reason: "action denied because verification entropy is high; force verification before switching, handoff, or progress".into(),
        };
    }

    if let Some(outcome) = validate_high_cost_handoff_entropy(proposed.clone(), case_file) {
        return outcome;
    }

    if matches!(
        proposed,
        ControlAction::ContinueWorking
            | ControlAction::SendFollowUp { .. }
            | ControlAction::RunProbe { .. }
    ) && trace_and_verification_block_required(case_file)
    {
        return ValidationOutcome::Modified {
            original: proposed,
            replacement: trace_and_verification_block_action(case_file),
            reason: "progress action denied because trace/repo-blame entropy requires a blocking repair packet".into(),
        };
    }

    if matches!(proposed, ControlAction::RunProbe { .. }) && !run_probe_entropy_allowed(case_file) {
        return ValidationOutcome::Modified {
            original: proposed,
            replacement: deterministic_control_action(case_file),
            reason: "run_probe denied because probe-worthy entropy is not high enough".into(),
        };
    }

    if matches!(proposed, ControlAction::SendFollowUp { .. })
        && !send_follow_up_entropy_allowed(case_file)
    {
        return ValidationOutcome::Modified {
            original: proposed,
            replacement: deterministic_control_action(case_file),
            reason: "send_follow_up denied because follow-up entropy is not high enough".into(),
        };
    }

    if matches!(proposed, ControlAction::SpawnJudgeAgent { .. })
        && !spawn_judge_entropy_allowed(case_file)
    {
        return ValidationOutcome::Modified {
            original: proposed,
            replacement: deterministic_control_action(case_file),
            reason: "spawn_judge_agent denied because repo/blame entropy is not high enough".into(),
        };
    }

    if let ControlAction::SpawnJudgeAgent { target_agent: None } = &proposed {
        let replacement = judge_agent_for_case_file(case_file).map_or_else(
            || ControlAction::Pause {
                reason: "no adapter capability allows a read-only judge target".into(),
            },
            |target_agent| ControlAction::SpawnJudgeAgent {
                target_agent: Some(target_agent),
            },
        );
        return ValidationOutcome::Modified {
            original: proposed,
            replacement,
            reason: "spawn_judge_agent target normalized to a read-only capable adapter".into(),
        };
    }

    if let ControlAction::SpawnJudgeAgent {
        target_agent: Some(target_agent),
    } = &proposed
        && !adapter_can_receive_readonly_judge(case_file, target_agent)
    {
        return ValidationOutcome::Modified {
            original: proposed.clone(),
            replacement: ControlAction::SpawnJudgeAgent {
                target_agent: judge_agent_for_case_file(case_file),
            },
            reason: "spawn_judge_agent target normalized to a read-only capable adapter".into(),
        };
    }

    if matches!(proposed, ControlAction::AskUser { .. })
        && case_file
            .entropy
            .score(EntropyKind::UserDecision)
            .is_none_or(|score| score.score < USER_DECISION_ASK_USER_THRESHOLD)
    {
        let replacement = deterministic_control_action(case_file);
        return ValidationOutcome::Modified {
            original: proposed,
            replacement,
            reason: "ask_user denied because user-decision entropy is not high enough".into(),
        };
    }

    if let ControlAction::AskUser { question } = &proposed {
        let bounded_question = ask_user_question_for_case_file(case_file);
        if question != &bounded_question {
            return ValidationOutcome::Modified {
                original: proposed,
                replacement: ControlAction::AskUser {
                    question: bounded_question,
                },
                reason: "ask_user normalized to bounded monitor question".into(),
            };
        }
    }

    ValidationOutcome::Approved(proposed)
}

fn validate_high_cost_handoff_entropy(
    proposed: ControlAction,
    case_file: &ControlCaseFile,
) -> Option<ValidationOutcome> {
    high_cost_handoff_entropy_reason(&proposed, case_file).map(|reason| {
        ValidationOutcome::Modified {
            original: proposed,
            replacement: deterministic_control_action(case_file),
            reason,
        }
    })
}

fn high_cost_handoff_entropy_reason(
    action: &ControlAction,
    case_file: &ControlCaseFile,
) -> Option<String> {
    match action {
        ControlAction::SwitchAgent { .. } if !switch_agent_entropy_allowed(case_file) => {
            Some("switch_agent denied because agent-health entropy is not high enough".into())
        }
        ControlAction::SpawnFreshAgent { .. } if !spawn_fresh_entropy_allowed(case_file) => Some(
            "spawn_fresh_agent denied because context or agent-health entropy is not high enough"
                .into(),
        ),
        _ => None,
    }
}

fn validate_writable_handoff_target(
    proposed: &ControlAction,
    case_file: &ControlCaseFile,
) -> Option<ValidationOutcome> {
    if !requires_writable_handoff_lock(proposed) {
        return None;
    }
    let target_agent = target_agent_for_action(proposed, case_file);
    if adapter_can_receive_writable_handoff(case_file, &target_agent) {
        return None;
    }

    let reason = unsafe_writable_handoff_reason(case_file, &target_agent);
    let replacement = match (
        proposed,
        fallback_agent_for_case_file(case_file, Some(&target_agent)),
    ) {
        (ControlAction::SwitchAgent { .. }, Some(target_agent)) => {
            ControlAction::SwitchAgent { target_agent }
        }
        (ControlAction::SpawnFreshAgent { .. }, Some(target_agent)) => {
            ControlAction::SpawnFreshAgent {
                target_agent: Some(target_agent),
            }
        }
        (ControlAction::SwitchAgent { .. } | ControlAction::SpawnFreshAgent { .. }, None) => {
            ControlAction::Pause {
                reason: "no adapter capability allows a writable handoff target".into(),
            }
        }
        _ => return None,
    };
    Some(ValidationOutcome::Modified {
        original: proposed.clone(),
        replacement,
        reason,
    })
}

fn validate_active_writer_handoff(
    proposed: &ControlAction,
    case_file: &ControlCaseFile,
) -> Option<ValidationOutcome> {
    if !requires_writable_handoff_lock(proposed) {
        return None;
    }
    let target_agent = target_agent_for_action(proposed, case_file);
    let reason = active_writer_handoff_conflict_reason(case_file, &target_agent)?;
    Some(ValidationOutcome::Modified {
        original: proposed.clone(),
        replacement: ControlAction::Pause {
            reason: reason.clone(),
        },
        reason,
    })
}

fn action_from_validation_outcome(outcome: &ValidationOutcome) -> ControlAction {
    match outcome {
        ValidationOutcome::Approved(action) => action.clone(),
        ValidationOutcome::Modified { replacement, .. } => replacement.clone(),
        ValidationOutcome::Denied { reason, .. } => ControlAction::Pause {
            reason: reason.clone(),
        },
    }
}

pub fn advise_workspace(workspace: impl AsRef<Path>) -> Result<AdviceRun, AdviceError> {
    let workspace = workspace.as_ref();
    let mut store = ProjectStore::open(workspace)?;
    let snapshot = DashboardSnapshot::load(store.root(), 500)?;
    let config = ProjectConfig::load(store.root())?;
    let calibration = load_control_calibration(workspace)?;
    release_configured_stale_worktree_locks(&mut store, &config.policy)?;
    let mut case_file = build_control_case_file_with_config(workspace, &snapshot, &config);
    record_timed_out_handoff_outcomes(&mut store, &case_file, &config.policy)?;
    apply_dynamic_policy_bounds(&store, &config.policy, &mut case_file)?;
    let (advisor_used, advisor_error, advisor_decision) = if config.advisor.enabled {
        let advisor_case_file =
            bound_case_file_for_advisor(&case_file, config.advisor.provider.max_input_tokens);
        match request_advisor_decision(&config.advisor.provider, &advisor_case_file, store.root())
            .and_then(|decision| {
                validate_advisor_decision(&decision, &advisor_case_file)?;
                Ok(decision)
            }) {
            Ok(decision) => (true, None, Some(decision)),
            Err(error) => (true, Some(error.to_string()), None),
        }
    } else {
        (false, None, None)
    };

    let proposed = advisor_decision
        .as_ref()
        .map(|decision| decision.proposed_action.clone())
        .unwrap_or_else(|| deterministic_control_action_with_calibration(&case_file, &calibration));
    let deterministic_selection_used = advisor_decision.is_none();
    let mut validation_outcome = validate_control_action_detailed(proposed, &case_file);
    let mut final_action = action_from_validation_outcome(&validation_outcome);
    enforce_user_interrupt_budget(
        &store,
        &config.policy,
        &mut validation_outcome,
        &mut final_action,
        &case_file,
    )?;
    enforce_handoff_cooldowns(
        &store,
        &config.policy,
        &mut validation_outcome,
        &mut final_action,
        &case_file,
    )?;
    let acquired_handoff_lock = enforce_writable_handoff_lock(
        &mut store,
        &config.policy,
        &mut validation_outcome,
        &mut final_action,
        &case_file,
    )?;
    let mut packet = control_packet_for_action(&final_action, &case_file);
    let control_rationale = control_rationale_for_action(
        &final_action,
        &case_file,
        deterministic_selection_used.then_some(&calibration),
    );
    let dispatch_result = match duplicate_urgent_packet_dispatch(&store, &final_action, &packet)? {
        Some(duplicate) => {
            packet = duplicate.packet;
            store.append_dispatch(&duplicate.dispatch)?;
            duplicate.dispatch
        }
        None => match store.dispatch_control_packet(&packet) {
            Ok(dispatch) => dispatch,
            Err(error) => {
                if let Some(lock) = acquired_handoff_lock {
                    let _ = store.release_worktree_lock(&lock.worktree, &lock.lock_id);
                }
                if !failed_dispatch_is_replayable(&error) {
                    return Err(error.into());
                }
                let dispatch = failed_dispatch_result(&packet, &error);
                store.append_dispatch(&dispatch)?;
                dispatch
            }
        },
    };
    store.append_case_file(&case_file)?;
    let advice = AdviceRun {
        advice_id: format!("advice-{}", current_id_fragment()),
        case_file_id: case_file.case_file_id,
        advisor_used,
        advisor_error,
        advisor_decision,
        validation_outcome,
        final_action,
        control_rationale,
        packet,
        packet_path: dispatch_result.path.clone(),
        dispatch_result,
    };
    store.append_advice(&advice)?;
    if matches!(advice.final_action, ControlAction::RunProbe { .. }) {
        run_probe(workspace)?;
    }
    record_immediate_outcome_for_advice(&mut store, &advice)?;
    Ok(advice)
}

fn failed_dispatch_result(packet: &ControlPacket, error: &StoreError) -> DispatchResult {
    DispatchResult {
        dispatch_id: format!("dispatch-{}", current_id_fragment()),
        packet_id: packet.packet_id.clone(),
        target_agent: packet.target_agent.clone(),
        status: DispatchStatus::Failed,
        path: None,
        reason: Some(error.to_string()),
    }
}

fn failed_dispatch_is_replayable(error: &StoreError) -> bool {
    !matches!(
        error,
        StoreError::SecretLikePacket { .. } | StoreError::UnknownPacketEvidenceRef { .. }
    )
}

fn apply_dynamic_policy_bounds(
    store: &ProjectStore,
    policy: &PolicyConfig,
    case_file: &mut ControlCaseFile,
) -> Result<(), StoreError> {
    let now = case_file_policy_time(case_file);
    let max_questions = policy.max_user_questions_per_hour as usize;
    let recent_questions = recent_ask_user_advice_count(store, now)?;
    if recent_questions >= max_questions {
        forbid_case_file_action(
            case_file,
            ControlActionKind::AskUser,
            format!(
                "user interrupt budget exhausted: {recent_questions}/{max_questions} ask_user actions in the last hour"
            ),
        );
    }

    apply_user_decision_ask_user_bound(case_file);
    apply_verification_progress_bound(case_file);
    apply_retry_agent_entropy_bound(case_file);
    apply_spawn_judge_entropy_bound(case_file);
    apply_run_probe_entropy_bound(case_file);
    apply_send_follow_up_entropy_bound(case_file);
    apply_handoff_cooldown_bound(
        store,
        case_file,
        now,
        ControlActionKind::SwitchAgent,
        policy.switch_agent_cooldown_min,
    )?;
    apply_handoff_cooldown_bound(
        store,
        case_file,
        now,
        ControlActionKind::SpawnFreshAgent,
        policy.spawn_fresh_cooldown_min,
    )?;
    apply_active_writer_handoff_bound(case_file);
    apply_writable_handoff_capacity_bound(store, policy, case_file)?;
    apply_high_cost_handoff_entropy_bound(case_file);
    Ok(())
}

fn apply_user_decision_ask_user_bound(case_file: &mut ControlCaseFile) {
    if case_file
        .entropy
        .score(EntropyKind::UserDecision)
        .is_some_and(|score| score.score >= USER_DECISION_ASK_USER_THRESHOLD)
    {
        return;
    }

    forbid_case_file_action(
        case_file,
        ControlActionKind::AskUser,
        "user-decision entropy is not high enough for a user interrupt".into(),
    );
}

fn apply_verification_progress_bound(case_file: &mut ControlCaseFile) {
    if !verification_entropy_is_high(case_file) {
        return;
    }

    let reason =
        "verification entropy is high; force verification before progress or handoff actions"
            .to_string();
    forbid_case_file_action(
        case_file,
        ControlActionKind::ContinueWorking,
        reason.clone(),
    );
    forbid_case_file_action(case_file, ControlActionKind::SendFollowUp, reason.clone());
    forbid_case_file_action(case_file, ControlActionKind::RunProbe, reason.clone());
    forbid_case_file_action(
        case_file,
        ControlActionKind::SpawnJudgeAgent,
        reason.clone(),
    );
    forbid_case_file_action(case_file, ControlActionKind::SwitchAgent, reason.clone());
    forbid_case_file_action(case_file, ControlActionKind::SpawnFreshAgent, reason);
}

fn verification_entropy_is_high(case_file: &ControlCaseFile) -> bool {
    case_file
        .entropy
        .score(EntropyKind::Verification)
        .is_some_and(|score| score.score >= 75)
}

fn verification_first_action_is_required(
    action: &ControlAction,
    case_file: &ControlCaseFile,
) -> bool {
    matches!(
        action,
        ControlAction::ContinueWorking
            | ControlAction::SendFollowUp { .. }
            | ControlAction::RunProbe { .. }
            | ControlAction::SpawnJudgeAgent { .. }
            | ControlAction::SwitchAgent { .. }
            | ControlAction::SpawnFreshAgent { .. }
    ) && verification_entropy_is_high(case_file)
}

fn apply_retry_agent_entropy_bound(case_file: &mut ControlCaseFile) {
    if retry_agent_entropy_allowed(case_file) {
        return;
    }

    forbid_case_file_action(
        case_file,
        ControlActionKind::RetryAgent,
        "agent-health entropy is not high enough for retry_agent".into(),
    );
}

fn apply_send_follow_up_entropy_bound(case_file: &mut ControlCaseFile) {
    if send_follow_up_entropy_allowed(case_file) {
        return;
    }

    forbid_case_file_action(
        case_file,
        ControlActionKind::SendFollowUp,
        "follow-up entropy is not high enough for send_follow_up".into(),
    );
}

fn apply_run_probe_entropy_bound(case_file: &mut ControlCaseFile) {
    if run_probe_entropy_allowed(case_file) {
        return;
    }

    forbid_case_file_action(
        case_file,
        ControlActionKind::RunProbe,
        "probe-worthy entropy is not high enough for run_probe".into(),
    );
}

fn apply_spawn_judge_entropy_bound(case_file: &mut ControlCaseFile) {
    if spawn_judge_entropy_allowed(case_file) {
        return;
    }

    forbid_case_file_action(
        case_file,
        ControlActionKind::SpawnJudgeAgent,
        "repo/blame entropy is not high enough for spawn_judge_agent".into(),
    );
}

fn apply_high_cost_handoff_entropy_bound(case_file: &mut ControlCaseFile) {
    if !switch_agent_entropy_allowed(case_file) {
        forbid_case_file_action(
            case_file,
            ControlActionKind::SwitchAgent,
            "agent-health entropy is not high enough for switch_agent".into(),
        );
    }
    if !spawn_fresh_entropy_allowed(case_file) {
        forbid_case_file_action(
            case_file,
            ControlActionKind::SpawnFreshAgent,
            "context or agent-health entropy is not high enough for spawn_fresh_agent".into(),
        );
    }
}

fn apply_writable_handoff_capacity_bound(
    store: &ProjectStore,
    policy: &PolicyConfig,
    case_file: &mut ControlCaseFile,
) -> Result<(), StoreError> {
    let active = store.active_worktree_lock_count()?;
    if let Some(reason) = writable_handoff_capacity_reason(policy, active) {
        forbid_case_file_action(case_file, ControlActionKind::SwitchAgent, reason.clone());
        forbid_case_file_action(case_file, ControlActionKind::SpawnFreshAgent, reason);
    }
    Ok(())
}

fn apply_active_writer_handoff_bound(case_file: &mut ControlCaseFile) {
    let Some(reason) = active_writer_handoff_bound_reason(case_file) else {
        return;
    };
    forbid_case_file_action(case_file, ControlActionKind::SwitchAgent, reason.clone());
    forbid_case_file_action(case_file, ControlActionKind::SpawnFreshAgent, reason);
}

fn apply_handoff_cooldown_bound(
    store: &ProjectStore,
    case_file: &mut ControlCaseFile,
    now: i64,
    kind: ControlActionKind,
    cooldown_min: u32,
) -> Result<(), StoreError> {
    if cooldown_min == 0 {
        return Ok(());
    }
    let recent_actions =
        recent_advice_action_count(store, now, i64::from(cooldown_min) * 60, kind)?;
    if recent_actions > 0 {
        forbid_case_file_action(
            case_file,
            kind,
            format!(
                "{} cooldown active: {recent_actions} matching action(s) within the last {cooldown_min} minute(s)",
                control_action_kind_label(kind)
            ),
        );
    }
    Ok(())
}

fn case_file_policy_time(case_file: &ControlCaseFile) -> i64 {
    parse_utc_seconds(&case_file.built_at)
        .or_else(current_utc_seconds)
        .unwrap_or(i64::MAX)
}

fn forbid_case_file_action(
    case_file: &mut ControlCaseFile,
    action: ControlActionKind,
    reason: String,
) {
    case_file
        .allowed_actions
        .retain(|allowed| allowed != &action);
    if !case_file
        .forbidden_actions
        .iter()
        .any(|forbidden| forbidden.action == action)
    {
        case_file
            .forbidden_actions
            .push(ForbiddenAction { action, reason });
    }
}

fn enforce_user_interrupt_budget(
    store: &ProjectStore,
    policy: &PolicyConfig,
    validation_outcome: &mut ValidationOutcome,
    final_action: &mut ControlAction,
    case_file: &ControlCaseFile,
) -> Result<(), StoreError> {
    if !matches!(final_action, ControlAction::AskUser { .. }) {
        return Ok(());
    }

    let max_questions = policy.max_user_questions_per_hour as usize;
    let now = case_file_policy_time(case_file);
    let recent_questions = recent_ask_user_advice_count(store, now)?;
    if recent_questions < max_questions {
        return Ok(());
    }

    let original = final_action.clone();
    let reason = format!(
        "user interrupt budget exhausted: {recent_questions}/{max_questions} ask_user actions in the last hour"
    );
    let replacement = ControlAction::Pause {
        reason: reason.clone(),
    };
    *validation_outcome = ValidationOutcome::Modified {
        original,
        replacement: replacement.clone(),
        reason,
    };
    *final_action = replacement;
    Ok(())
}

fn recent_ask_user_advice_count(store: &ProjectStore, now: i64) -> Result<usize, StoreError> {
    let case_files = read_all_jsonl::<ControlCaseFile>(&store.root.join("case-files.jsonl"))?;
    let case_times = case_files
        .into_iter()
        .filter_map(|case_file| {
            parse_utc_seconds(&case_file.built_at).map(|seconds| (case_file.case_file_id, seconds))
        })
        .collect::<HashMap<_, _>>();
    let advice_records = read_all_jsonl::<AdviceRun>(&store.root.join("advice.jsonl"))?;
    Ok(advice_records
        .into_iter()
        .filter(|advice| matches!(advice.final_action, ControlAction::AskUser { .. }))
        .filter(|advice| {
            case_times
                .get(&advice.case_file_id)
                .is_some_and(|asked_at| now - *asked_at < 3_600)
        })
        .count())
}

fn enforce_handoff_cooldowns(
    store: &ProjectStore,
    policy: &PolicyConfig,
    validation_outcome: &mut ValidationOutcome,
    final_action: &mut ControlAction,
    case_file: &ControlCaseFile,
) -> Result<(), StoreError> {
    let (kind, cooldown_min) = match final_action {
        ControlAction::SwitchAgent { .. } => (
            ControlActionKind::SwitchAgent,
            policy.switch_agent_cooldown_min,
        ),
        ControlAction::SpawnFreshAgent { .. } => (
            ControlActionKind::SpawnFreshAgent,
            policy.spawn_fresh_cooldown_min,
        ),
        _ => return Ok(()),
    };
    if cooldown_min == 0 {
        return Ok(());
    }

    let now = case_file_policy_time(case_file);
    let recent_actions =
        recent_advice_action_count(store, now, i64::from(cooldown_min) * 60, kind)?;
    if recent_actions == 0 {
        return Ok(());
    }

    let original = final_action.clone();
    let reason = format!(
        "{} cooldown active: {recent_actions} matching action(s) within the last {cooldown_min} minute(s)",
        control_action_kind_label(kind)
    );
    let replacement = ControlAction::Pause {
        reason: reason.clone(),
    };
    *validation_outcome = ValidationOutcome::Modified {
        original,
        replacement: replacement.clone(),
        reason,
    };
    *final_action = replacement;
    Ok(())
}

fn recent_advice_action_count(
    store: &ProjectStore,
    now: i64,
    window_secs: i64,
    kind: ControlActionKind,
) -> Result<usize, StoreError> {
    if window_secs <= 0 {
        return Ok(0);
    }
    let case_files = read_all_jsonl::<ControlCaseFile>(&store.root.join("case-files.jsonl"))?;
    let case_times = case_files
        .into_iter()
        .filter_map(|case_file| {
            parse_utc_seconds(&case_file.built_at).map(|seconds| (case_file.case_file_id, seconds))
        })
        .collect::<HashMap<_, _>>();
    let advice_records = read_all_jsonl::<AdviceRun>(&store.root.join("advice.jsonl"))?;
    Ok(advice_records
        .into_iter()
        .filter(|advice| advice.final_action.kind() == kind)
        .filter(|advice| {
            case_times
                .get(&advice.case_file_id)
                .is_some_and(|action_at| now - *action_at < window_secs)
        })
        .count())
}

fn control_action_kind_label(kind: ControlActionKind) -> &'static str {
    match kind {
        ControlActionKind::ContinueWorking => "continue_working",
        ControlActionKind::RetryAgent => "retry_agent",
        ControlActionKind::ForceVerification => "force_verification",
        ControlActionKind::RunProbe => "run_probe",
        ControlActionKind::BlockProgressUntilTraceAndVerification => {
            "block_progress_until_trace_and_verification"
        }
        ControlActionKind::SendFollowUp => "send_follow_up",
        ControlActionKind::SpawnJudgeAgent => "spawn_judge_agent",
        ControlActionKind::SpawnFreshAgent => "spawn_fresh_agent",
        ControlActionKind::SwitchAgent => "switch_agent",
        ControlActionKind::AskUser => "ask_user",
        ControlActionKind::Pause => "pause",
    }
}

fn writable_handoff_capacity_reason(policy: &PolicyConfig, active: usize) -> Option<String> {
    let max = policy.max_parallel_writable_agents as usize;
    if active < max {
        return None;
    }
    Some(format!(
        "max_parallel_writable_agents limit reached: {active}/{max} active writable handoff lock(s)"
    ))
}

fn enforce_writable_handoff_lock(
    store: &mut ProjectStore,
    policy: &PolicyConfig,
    validation_outcome: &mut ValidationOutcome,
    final_action: &mut ControlAction,
    case_file: &ControlCaseFile,
) -> Result<Option<WorktreeLock>, StoreError> {
    if !requires_writable_handoff_lock(final_action) {
        return Ok(None);
    }

    let target_agent = target_agent_for_action(final_action, case_file);
    if let Some(existing) = store.active_worktree_lock_for(&case_file.workspace)? {
        store.append_lock_event(&WorktreeLockEvent {
            kind: "conflict".into(),
            lock: existing.clone(),
            requested_owner: Some(target_agent.clone()),
        })?;
        let original = final_action.clone();
        let reason = worktree_lock_conflict_reason(&existing, &target_agent);
        let replacement = ControlAction::Pause {
            reason: reason.clone(),
        };
        *validation_outcome = ValidationOutcome::Modified {
            original,
            replacement: replacement.clone(),
            reason,
        };
        *final_action = replacement;
        return Ok(None);
    }

    let active_locks = store.active_worktree_lock_count()?;
    if let Some(reason) = writable_handoff_capacity_reason(policy, active_locks) {
        let original = final_action.clone();
        let replacement = ControlAction::Pause {
            reason: reason.clone(),
        };
        *validation_outcome = ValidationOutcome::Modified {
            original,
            replacement: replacement.clone(),
            reason,
        };
        *final_action = replacement;
        return Ok(None);
    }

    let request = WorktreeLockRequest {
        worktree: case_file.workspace.clone(),
        owner_agent: target_agent.clone(),
        session: None,
    };

    match store.try_acquire_worktree_lock(&request)? {
        WorktreeLockResult::Acquired(lock) => Ok(Some(lock)),
        WorktreeLockResult::Conflict { existing } => {
            let original = final_action.clone();
            let reason = worktree_lock_conflict_reason(&existing, &target_agent);
            let replacement = ControlAction::Pause {
                reason: reason.clone(),
            };
            *validation_outcome = ValidationOutcome::Modified {
                original,
                replacement: replacement.clone(),
                reason,
            };
            *final_action = replacement;
            Ok(None)
        }
    }
}

fn requires_writable_handoff_lock(action: &ControlAction) -> bool {
    matches!(
        action,
        ControlAction::SpawnFreshAgent { .. } | ControlAction::SwitchAgent { .. }
    )
}

fn active_writer_handoff_bound_reason(case_file: &ControlCaseFile) -> Option<String> {
    let writer = latest_file_change_writer(case_file)?;
    Some(format!(
        "active writer {writer} has recent file_change evidence in this worktree; pause writable handoff instead of assigning a second writer"
    ))
}

fn active_writer_handoff_conflict_reason(
    case_file: &ControlCaseFile,
    target_agent: &str,
) -> Option<String> {
    let writer = latest_file_change_writer(case_file)?;
    if normalize_agent_label(&writer) == normalize_agent_label(target_agent) {
        return None;
    }
    Some(format!(
        "active writer {writer} has recent file_change evidence in this worktree; cannot dispatch writable handoff to {target_agent}"
    ))
}

fn latest_file_change_writer(case_file: &ControlCaseFile) -> Option<String> {
    case_file
        .evidence
        .iter()
        .rev()
        .filter(|evidence| evidence.kind == "FileChange")
        .filter_map(|evidence| evidence.agent.as_deref())
        .find(|agent| {
            let agent = agent.trim();
            !agent.is_empty() && !agent.eq_ignore_ascii_case("user")
        })
        .map(str::to_string)
}

fn acquire_writable_handoff_lock(
    store: &mut ProjectStore,
    policy: &PolicyConfig,
    worktree: &str,
    target_agent: &str,
) -> Result<WorktreeLock, StoreError> {
    if let Some(existing) = store.active_worktree_lock_for(worktree)? {
        store.append_lock_event(&WorktreeLockEvent {
            kind: "conflict".into(),
            lock: existing.clone(),
            requested_owner: Some(target_agent.to_string()),
        })?;
        return Err(StoreError::WorktreeLockConflict {
            worktree: existing.worktree,
            existing_owner: existing.owner_agent,
            requested_owner: target_agent.to_string(),
        });
    }

    let active = store.active_worktree_lock_count()?;
    let max = policy.max_parallel_writable_agents as usize;
    if active >= max {
        return Err(StoreError::WorktreeCapacityExceeded {
            active,
            max,
            requested_owner: target_agent.to_string(),
        });
    }

    match store.try_acquire_worktree_lock(&WorktreeLockRequest {
        worktree: worktree.to_string(),
        owner_agent: target_agent.to_string(),
        session: None,
    })? {
        WorktreeLockResult::Acquired(lock) => Ok(lock),
        WorktreeLockResult::Conflict { existing } => Err(StoreError::WorktreeLockConflict {
            worktree: existing.worktree,
            existing_owner: existing.owner_agent,
            requested_owner: target_agent.to_string(),
        }),
    }
}

fn worktree_lock_conflict_reason(existing: &WorktreeLock, target_agent: &str) -> String {
    format!(
        "worktree {} is already locked by {}; cannot dispatch writable handoff to {}",
        existing.worktree, existing.owner_agent, target_agent
    )
}

pub fn handoff_workspace(
    workspace: impl AsRef<Path>,
    target_agent: AgentKind,
) -> Result<HandoffRun, AdviceError> {
    let workspace = workspace.as_ref();
    let mut store = ProjectStore::open(workspace)?;
    let config = ProjectConfig::load(store.root())?;
    release_configured_stale_worktree_locks(&mut store, &config.policy)?;
    let snapshot = DashboardSnapshot::load(store.root(), 500)?;
    let case_file = build_control_case_file_with_config(workspace, &snapshot, &config);
    let target_agent_label = agent_kind_label(target_agent);
    if !adapter_can_receive_writable_handoff(&case_file, target_agent_label) {
        return Err(AdviceError::UnsafeAdapterTarget {
            agent: target_agent_label.into(),
            reason: unsafe_writable_handoff_reason(&case_file, target_agent_label),
        });
    }
    let packet = handoff_packet_for_agent(target_agent, &case_file);
    let lock = acquire_writable_handoff_lock(
        &mut store,
        &config.policy,
        &case_file.workspace,
        &packet.target_agent,
    )?;
    let dispatch_result = match store.dispatch_control_packet(&packet) {
        Ok(dispatch) => dispatch,
        Err(error) => {
            let _ = store.release_worktree_lock(&lock.worktree, &lock.lock_id);
            return Err(error.into());
        }
    };
    store.append_case_file(&case_file)?;
    let packet_path = dispatch_result.path.clone();
    Ok(HandoffRun {
        case_file,
        packet,
        dispatch_result,
        packet_path,
    })
}

fn release_configured_stale_worktree_locks(
    store: &mut ProjectStore,
    policy: &PolicyConfig,
) -> Result<(), StoreError> {
    if let Some(stale_after_secs) = policy.worktree_lock_stale_after_secs {
        store.release_stale_worktree_locks(stale_after_secs)?;
    }
    Ok(())
}

pub fn load_decision_trails(
    store_root: impl AsRef<Path>,
) -> Result<Vec<DecisionTrail>, StoreError> {
    let store_root = store_root.as_ref();
    let case_files = read_all_jsonl::<ControlCaseFile>(&store_root.join("case-files.jsonl"))?;
    let advice_records = read_all_jsonl::<AdviceRun>(&store_root.join("advice.jsonl"))?;
    let packets = read_all_jsonl::<ControlPacket>(&store_root.join("packets.jsonl"))?;
    let dispatches = read_all_jsonl::<DispatchResult>(&store_root.join("dispatch.jsonl"))?;
    let outcomes = read_all_jsonl::<ActionOutcome>(&store_root.join("outcomes.jsonl"))?;

    let case_by_id = case_files
        .into_iter()
        .map(|case_file| (case_file.case_file_id.clone(), case_file))
        .collect::<HashMap<_, _>>();
    let packet_by_id = packets
        .into_iter()
        .map(|packet| (packet.packet_id.clone(), packet))
        .collect::<HashMap<_, _>>();
    let dispatch_by_packet_id = dispatches
        .into_iter()
        .map(|dispatch| (dispatch.packet_id.clone(), dispatch))
        .collect::<HashMap<_, _>>();
    let mut outcomes_by_advice = HashMap::<String, Vec<ActionOutcome>>::new();
    for outcome in outcomes {
        outcomes_by_advice
            .entry(outcome.advice_id.clone())
            .or_default()
            .push(outcome);
    }

    let mut trails = Vec::new();
    for advice in advice_records {
        let Some(case_file) = case_by_id.get(&advice.case_file_id).cloned() else {
            continue;
        };
        let packet = packet_by_id
            .get(&advice.packet.packet_id)
            .cloned()
            .unwrap_or_else(|| advice.packet.clone());
        let dispatch = dispatch_by_packet_id
            .get(&packet.packet_id)
            .cloned()
            .unwrap_or_else(|| inline_dispatch_for_advice(&advice, &packet));
        let outcomes = outcomes_by_advice
            .remove(&advice.advice_id)
            .unwrap_or_default();
        trails.push(DecisionTrail {
            case_file,
            advice,
            packet,
            dispatch,
            outcomes,
        });
    }
    Ok(trails)
}

fn inline_dispatch_for_advice(advice: &AdviceRun, packet: &ControlPacket) -> DispatchResult {
    let mut dispatch = advice.dispatch_result.clone();
    if dispatch.packet_id != packet.packet_id {
        dispatch.packet_id = packet.packet_id.clone();
    }
    if dispatch.target_agent.is_empty() {
        dispatch.target_agent = packet.target_agent.clone();
    }
    dispatch
}

#[cfg(test)]
#[path = "acceptance_extraction_tests.rs"]
mod acceptance_extraction_tests;

fn latest_verification_status(
    snapshot: &DashboardSnapshot,
    verifiers: &[VerifierConfig],
    policy: &PolicyConfig,
    repo_audit: Option<&RepoAuditReport>,
) -> VerificationStatus {
    let mut latest: Option<((i64, usize), VerificationStatus)> = None;
    for (index, event) in snapshot.recent_events.iter().enumerate() {
        if !event_is_verification_result(event) {
            continue;
        }
        let Some(time) = event.time.as_deref().and_then(parse_utc_seconds) else {
            continue;
        };
        let order = (time, index);
        let status = match event.exit_code {
            Some(0) => VerificationStatus::Passed,
            Some(_) => VerificationStatus::Failed,
            None => VerificationStatus::NotRun,
        };
        if latest.is_none_or(|(current, _)| order > current) {
            latest = Some((order, status));
        }
    }
    let verifier_sequence_base = snapshot.recent_events.len();
    for (index, run) in snapshot.recent_verifier_runs.iter().enumerate() {
        let Some(time) = verifier_run_time(run) else {
            continue;
        };
        let order = (time, verifier_sequence_base + index);
        let status = verifier_run_verification_status(run);
        if latest.is_none_or(|(current, _)| order > current) {
            latest = Some((order, status));
        }
    }

    let latest_status = latest
        .map(|(_, status)| status)
        .unwrap_or(VerificationStatus::NotRun);
    if latest_status == VerificationStatus::Passed
        && policy.require_verification_after_source_change
    {
        for (file, write_order) in changed_verification_file_orders(snapshot, policy, repo_audit) {
            let latest_covering_pass =
                latest_covering_pass_order(snapshot, verifiers, &file, verifier_sequence_base);
            if latest_covering_pass.is_none_or(|pass_order| pass_order < write_order) {
                return VerificationStatus::Stale;
            }
        }
    }
    latest_status
}

fn changed_verification_file_orders(
    snapshot: &DashboardSnapshot,
    policy: &PolicyConfig,
    repo_audit: Option<&RepoAuditReport>,
) -> BTreeMap<String, (i64, usize)> {
    let mut changed = BTreeMap::new();
    for (index, event) in snapshot.recent_events.iter().enumerate() {
        let Some(file) = event.file.as_deref() else {
            continue;
        };
        if !event_is_change_like(event) || !is_verification_relevant_file(file, policy) {
            continue;
        }
        record_changed_verification_file(
            &mut changed,
            file,
            (
                event
                    .time
                    .as_deref()
                    .and_then(parse_utc_seconds)
                    .unwrap_or(i64::MAX),
                index,
            ),
        );
    }

    let repo_sequence_base = snapshot.recent_events.len() + snapshot.recent_verifier_runs.len();
    if let Some(repo_audit) = repo_audit {
        for (index, change) in repo_audit.changes.iter().enumerate() {
            if !is_verification_relevant_file(&change.path, policy) {
                continue;
            }
            record_changed_verification_file(
                &mut changed,
                &change.path,
                (
                    change.modified_at.unwrap_or(i64::MAX),
                    repo_sequence_base + index,
                ),
            );
        }
    }
    changed
}

fn record_changed_verification_file(
    changed: &mut BTreeMap<String, (i64, usize)>,
    file: &str,
    order: (i64, usize),
) {
    let file = normalize_path_for_match(file);
    if changed
        .get(&file)
        .is_none_or(|current_order| order > *current_order)
    {
        changed.insert(file, order);
    }
}

fn latest_covering_pass_order(
    snapshot: &DashboardSnapshot,
    verifiers: &[VerifierConfig],
    file: &str,
    verifier_sequence_base: usize,
) -> Option<(i64, usize)> {
    let latest_event_pass = snapshot
        .recent_events
        .iter()
        .enumerate()
        .filter(|(_, event)| event_is_verification_result(event) && event.exit_code == Some(0))
        .filter(|(_, event)| {
            verification_result_covers_path(verifiers, None, event.command.as_deref(), file)
        })
        .filter_map(|(index, event)| {
            event
                .time
                .as_deref()
                .and_then(parse_utc_seconds)
                .map(|time| (time, index))
        })
        .max();
    let latest_verifier_pass = snapshot
        .recent_verifier_runs
        .iter()
        .enumerate()
        .filter(|(_, run)| run.status == VerificationRunStatus::Passed)
        .filter(|(_, run)| {
            verification_result_covers_path(
                verifiers,
                run.verifier_id.as_deref(),
                Some(run.command.as_str()),
                file,
            )
        })
        .filter_map(|(index, run)| {
            verifier_run_time(run).map(|time| (time, verifier_sequence_base + index))
        })
        .max();
    latest_event_pass.max(latest_verifier_pass)
}

fn verification_result_covers_path(
    verifiers: &[VerifierConfig],
    verifier_id: Option<&str>,
    command: Option<&str>,
    file: &str,
) -> bool {
    let command = command.map(normalize_command_signature);
    let mut matched_registered_verifier = false;
    for verifier in verifiers {
        let id_matches = verifier_id.is_some_and(|id| id == verifier.id);
        let command_matches = command
            .as_deref()
            .is_some_and(|command| normalize_command_signature(&verifier.command) == command);
        if !id_matches && !command_matches {
            continue;
        }
        matched_registered_verifier = true;
        if verifier_covers_path(verifier, file) {
            return true;
        }
    }

    if matched_registered_verifier {
        return false;
    }

    if !verifiers
        .iter()
        .any(|verifier| verifier_covers_path(verifier, file))
    {
        return true;
    }

    command
        .as_deref()
        .is_some_and(is_broad_verification_command)
}

fn verifier_covers_path(verifier: &VerifierConfig, file: &str) -> bool {
    verifier.scope == VerificationScope::Full
        || verifier.paths.is_empty()
        || verifier_matches_path(verifier, file)
}

fn is_broad_verification_command(command: &str) -> bool {
    let command = normalize_command_signature(command).to_lowercase();
    matches!(
        command.as_str(),
        "cargo test"
            | "cargo check"
            | "cargo build"
            | "npm test"
            | "npm run test"
            | "npm run build"
            | "pnpm test"
            | "pnpm build"
            | "yarn test"
            | "yarn build"
            | "pytest"
            | "python -m pytest"
            | "vitest"
            | "npx vitest"
            | "jest"
            | "npx jest"
            | "tsc"
            | "npx tsc"
    ) || command.starts_with("cargo test --workspace")
        || command.starts_with("cargo test --all")
        || command.starts_with("cargo test --all-targets")
        || command.starts_with("cargo test --all-features")
        || command.starts_with("pytest ")
        || command.starts_with("python -m pytest ")
}

fn verifier_run_time(run: &VerifierRun) -> Option<i64> {
    run.completed_at
        .as_deref()
        .or(Some(run.started_at.as_str()))
        .and_then(parse_utc_seconds)
}

#[cfg(test)]
#[path = "console_decode_tests.rs"]
mod console_decode_tests;
