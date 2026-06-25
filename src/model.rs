//! Domain model: data types shared across the monitor control plane.
//!
//! These are the typed events, entropy vectors, case-file, control-action,
//! packet, dashboard, and memory records. They carry no I/O; behavior lives in
//! `store`, the control-engine free functions, and the calibration/monitor
//! layers in `lib.rs`.
//!
//! This file is large by design for now: keeping serde-facing wire types in one
//! place makes the JSONL/case-file contract easier to audit while the schema is
//! still changing.

use crate::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub open_work: bool,
    pub retry_limit: usize,
    pub fallback_agents: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            open_work: true,
            retry_limit: 2,
            fallback_agents: vec!["claude-code".into(), "opencode".into(), "pi".into()],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdapterIngestOptions {
    pub adapter: AgentKind,
    pub session: Option<String>,
    pub config: Config,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Intervention {
    pub kind: InterventionKind,
    pub action: Action,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InterventionKind {
    PrematureStop,
    ServiceFailure,
    AgentDegraded,
    SuspiciousChange,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    ContinueWorking,
    RetrySameAgent,
    SwitchAgent,
    SpawnFreshAgent,
    SpawnJudgeAgent,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DesignEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time: Option<String>,
    pub agent: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    pub content: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TraceEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_id: Option<String>,
    pub agent: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    pub file: String,
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
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BlameQuery {
    pub file: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    pub limit: usize,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BlameStatus {
    Traced,
    Untraced,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BlameMatchKind {
    ExactLine,
    File,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BlameMatch {
    pub match_kind: BlameMatchKind,
    pub trace: TraceEntry,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BlameReport {
    pub workspace: String,
    pub file: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    pub status: BlameStatus,
    pub trace_count: usize,
    pub matches: Vec<BlameMatch>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RepoAuditStatus {
    Clean,
    Warning,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RepoChangeKind {
    Modified,
    Added,
    Deleted,
    Untracked,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RepoTraceStatus {
    Traced,
    MissingRationale,
    #[default]
    Untraced,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepoDiffHunk {
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    #[serde(default)]
    pub trace_status: RepoTraceStatus,
    #[serde(default)]
    pub matching_trace_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepoChangeAudit {
    pub path: String,
    pub kind: RepoChangeKind,
    pub trace_status: RepoTraceStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modified_at: Option<i64>,
    pub hunks: Vec<RepoDiffHunk>,
    pub matching_traces: Vec<TraceEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepoHunkHistoryEntry {
    pub history_id: String,
    pub observed_at: String,
    pub workspace: String,
    pub path: String,
    pub kind: RepoChangeKind,
    pub hunk_index: usize,
    pub old_start: u32,
    pub old_lines: u32,
    pub new_start: u32,
    pub new_lines: u32,
    pub trace_status: RepoTraceStatus,
    pub matching_trace_count: usize,
    pub change_trace_status: RepoTraceStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modified_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub matching_trace_refs: Vec<RepoHunkTraceRef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepoHunkTraceRef {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_id: Option<String>,
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
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepoAuditReport {
    pub workspace: String,
    pub status: RepoAuditStatus,
    pub changes: Vec<RepoChangeAudit>,
    pub untraced_count: usize,
    pub unexplained_count: usize,
}

pub(crate) const REPO_AUDIT_MAX_MATCHING_TRACES_PER_CHANGE: usize = 5;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentHealth {
    pub agent: String,
    pub score: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AgentEventHealth {
    Healthy,
    ServiceFailure,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentActivityStatus {
    Active,
    Stale,
    Degraded,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentSession {
    pub agent: String,
    pub status: AgentActivityStatus,
    pub score: i32,
    pub events: usize,
    pub interventions: usize,
    pub last_seen: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DashboardSeverity {
    Healthy,
    Warning,
    Critical,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DashboardAdvisorCredentialKind {
    None,
    Env,
    ApiKey,
    JwtBearer,
    MissingProfile,
    InvalidProfile,
    UnsupportedSource,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DashboardAdvisorStatus {
    pub enabled: bool,
    pub credential_source: AdvisorCredentialSource,
    pub credential_kind: DashboardAdvisorCredentialKind,
    pub uses_dedicated_profile: bool,
    pub endpoint: String,
    pub endpoint_host: Option<String>,
    pub model: String,
    pub credential_file: Option<String>,
    pub severity: DashboardSeverity,
    pub message: String,
}

impl Default for DashboardAdvisorStatus {
    fn default() -> Self {
        Self {
            enabled: false,
            credential_source: AdvisorCredentialSource::Env,
            credential_kind: DashboardAdvisorCredentialKind::None,
            uses_dedicated_profile: false,
            endpoint: String::new(),
            endpoint_host: None,
            model: String::new(),
            credential_file: None,
            severity: DashboardSeverity::Healthy,
            message: "advisor disabled".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DashboardSnapshot {
    pub severity: DashboardSeverity,
    #[serde(default)]
    pub advisor_status: DashboardAdvisorStatus,
    pub event_count: usize,
    pub intervention_count: usize,
    #[serde(default)]
    pub verifier_run_count: usize,
    #[serde(default)]
    pub probe_run_count: usize,
    #[serde(default)]
    pub repo_hunk_history_count: usize,
    #[serde(default)]
    pub repo_hunk_file_count: usize,
    #[serde(default)]
    pub requirement_count: usize,
    #[serde(default)]
    pub dev_history_count: usize,
    #[serde(default)]
    pub decision_trail_count: usize,
    #[serde(default)]
    pub advice_count: usize,
    #[serde(default)]
    pub packet_count: usize,
    #[serde(default)]
    pub dispatch_count: usize,
    #[serde(default)]
    pub outcome_count: usize,
    #[serde(default)]
    pub lock_event_count: usize,
    pub design_count: usize,
    pub trace_count: usize,
    pub active_agents: Vec<String>,
    pub agent_health: Vec<AgentHealth>,
    pub agent_sessions: Vec<AgentSession>,
    pub rows: Vec<DashboardRow>,
    pub recent_events: Vec<Event>,
    pub recent_interventions: Vec<Intervention>,
    #[serde(default)]
    pub recent_verifier_runs: Vec<VerifierRun>,
    #[serde(default)]
    pub recent_probe_runs: Vec<ProbeRun>,
    #[serde(default)]
    pub recent_repo_hunks: Vec<RepoHunkHistoryEntry>,
    #[serde(default)]
    pub recent_repo_hunk_files: Vec<RepoHunkFileSummary>,
    #[serde(default)]
    pub recent_requirements: Vec<RequirementNode>,
    #[serde(default)]
    pub recent_requirement_proofs: Vec<RequirementProofStep>,
    #[serde(default)]
    pub recent_dev_history: Vec<DevHistoryReport>,
    #[serde(default)]
    pub recent_decision_trails: Vec<DecisionTrail>,
    #[serde(default)]
    pub recent_worktree_lock_events: Vec<WorktreeLockEvent>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdapterCapabilities {
    #[serde(default = "default_adapter_enabled")]
    pub enabled: bool,
    pub ingest_transcript: bool,
    pub ingest_jsonl: bool,
    pub hook_pre_tool: bool,
    pub hook_post_tool: bool,
    pub hook_stop: bool,
    pub can_block_tool: bool,
    pub can_rewrite_tool_input: bool,
    pub can_inject_context: bool,
    pub can_run_headless: bool,
    pub can_resume_session: bool,
    pub can_export_session: bool,
    pub can_start_subagent: bool,
    pub can_switch_mode: bool,
    pub supports_readonly_mode: bool,
    pub supports_workspace_write_mode: bool,
    pub requires_external_sandbox: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_auth: Option<RuntimeAuthConfig>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum EntropyKind {
    Goal,
    Context,
    RepoBlame,
    Verification,
    Plan,
    AgentHealth,
    UserDecision,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EntropyTrend {
    Rising,
    Falling,
    Stable,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EntropyScore {
    pub kind: EntropyKind,
    pub score: u8,
    pub confidence: u8,
    pub trend: EntropyTrend,
    pub top_causes: Vec<String>,
    pub evidence_ids: Vec<String>,
    pub missing_evidence: Vec<String>,
    pub recommended_observations: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EntropyVector {
    pub scores: Vec<EntropyScore>,
}

impl EntropyVector {
    pub fn baseline() -> Self {
        Self {
            scores: [
                EntropyKind::Goal,
                EntropyKind::Context,
                EntropyKind::RepoBlame,
                EntropyKind::Verification,
                EntropyKind::Plan,
                EntropyKind::AgentHealth,
                EntropyKind::UserDecision,
            ]
            .into_iter()
            .map(|kind| EntropyScore {
                kind,
                score: 0,
                confidence: 70,
                trend: EntropyTrend::Stable,
                top_causes: Vec::new(),
                evidence_ids: Vec::new(),
                missing_evidence: Vec::new(),
                recommended_observations: Vec::new(),
            })
            .collect(),
        }
    }

    pub fn score(&self, kind: EntropyKind) -> Option<&EntropyScore> {
        self.scores.iter().find(|score| score.kind == kind)
    }

    pub(crate) fn score_mut(&mut self, kind: EntropyKind) -> &mut EntropyScore {
        self.scores
            .iter_mut()
            .find(|score| score.kind == kind)
            .expect("baseline entropy vector should contain every kind")
    }

    pub(crate) fn raise(
        &mut self,
        kind: EntropyKind,
        score: u8,
        confidence: u8,
        cause: impl Into<String>,
        evidence_id: Option<String>,
        missing_evidence: Option<String>,
    ) {
        let entry = self.score_mut(kind);
        if score >= entry.score {
            entry.score = score;
            entry.confidence = confidence;
            entry.trend = EntropyTrend::Rising;
        }
        let cause = cause.into();
        if !entry.top_causes.contains(&cause) {
            entry.top_causes.push(cause);
        }
        if let Some(evidence_id) = evidence_id
            && !entry.evidence_ids.contains(&evidence_id)
        {
            entry.evidence_ids.push(evidence_id);
        }
        if let Some(missing) = missing_evidence
            && !entry.missing_evidence.contains(&missing)
        {
            entry.missing_evidence.push(missing);
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EvidenceItem {
    pub id: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_session_id: Option<String>,
    pub summary: String,
    #[serde(default)]
    pub redaction_status: RedactionStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_offset: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_hash: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub redaction_rules: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ForbiddenAction {
    pub action: ControlActionKind,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ControlCaseFile {
    pub case_file_id: String,
    pub built_at: String,
    pub workspace: String,
    #[serde(default)]
    pub replay: CaseFileReplay,
    #[serde(default)]
    pub task: TaskSummary,
    pub severity: DashboardSeverity,
    pub event_count: usize,
    pub intervention_count: usize,
    pub active_agents: Vec<String>,
    pub evidence: Vec<EvidenceItem>,
    pub entropy: EntropyVector,
    #[serde(default)]
    pub belief_state: BeliefState,
    pub allowed_actions: Vec<ControlActionKind>,
    pub forbidden_actions: Vec<ForbiddenAction>,
    #[serde(default)]
    pub latest_verification_status: VerificationStatus,
    #[serde(default)]
    pub verification: VerificationSummary,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requirements: Vec<RequirementNode>,
    #[serde(default)]
    pub completion_certificate: CompletionCertificate,
    #[serde(default)]
    pub memory_candidates: Vec<MemoryCandidate>,
    #[serde(default)]
    pub durable_memory: Vec<MemoryCandidate>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub adapter_capabilities: BTreeMap<String, AdapterCapabilities>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo_audit: Option<RepoAuditReport>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskSummary {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_goal: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_goal_event_id: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub acceptance_criteria: Vec<TaskAcceptanceCriterion>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ambiguity_markers: Vec<TaskAmbiguityMarker>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskAcceptanceCriterion {
    pub id: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_event_id: Option<String>,
    pub confidence: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskAmbiguityMarker {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_event_id: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct CaseFileReplay {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_head: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_dirty: Option<bool>,
    #[serde(default)]
    pub input: CaseFileInputBoundary,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct CaseFileInputBoundary {
    pub event_count: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_event_seq: Option<u64>,
    pub intervention_count: usize,
    pub verifier_run_count: usize,
    #[serde(default)]
    pub probe_run_count: usize,
    pub repo_hunk_history_count: usize,
    #[serde(default)]
    pub repo_hunk_file_count: usize,
    pub requirement_count: usize,
    #[serde(default)]
    pub dev_history_count: usize,
    #[serde(default)]
    pub advice_count: usize,
    #[serde(default)]
    pub packet_count: usize,
    #[serde(default)]
    pub dispatch_count: usize,
    #[serde(default)]
    pub outcome_count: usize,
    #[serde(default)]
    pub lock_event_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemoryCandidate {
    pub memory_id: String,
    pub scope: MemoryScope,
    pub claim: String,
    pub status: MemoryStatus,
    pub source: MemorySource,
    pub evidence_ids: Vec<String>,
    pub confidence: u8,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MemoryScope {
    Project,
    Module,
    File,
    Task,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MemoryStatus {
    Active,
    Deprecated,
    Conflicted,
    Unverified,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MemorySource {
    User,
    VerifiedResult,
    AgentClaim,
    ManualReview,
}

impl MemorySource {
    pub(crate) fn is_trusted_promotion_source(self) -> bool {
        matches!(
            self,
            MemorySource::User | MemorySource::VerifiedResult | MemorySource::ManualReview
        )
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VerificationStatus {
    Passed,
    Failed,
    Stale,
    #[default]
    NotRun,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VerificationSummary {
    pub status: VerificationStatus,
    pub recommended_commands: Vec<String>,
    pub changed_source_files: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub acceptance_criteria: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub uncovered_acceptance_criteria: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub acceptance_coverage: Vec<AcceptanceCoverage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_passing_command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_failing_command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_failure_class: Option<VerificationFailureClass>,
}

impl Default for VerificationSummary {
    fn default() -> Self {
        Self {
            status: VerificationStatus::NotRun,
            recommended_commands: Vec::new(),
            changed_source_files: Vec::new(),
            acceptance_criteria: Vec::new(),
            uncovered_acceptance_criteria: Vec::new(),
            acceptance_coverage: Vec::new(),
            latest_passing_command: None,
            latest_failing_command: None,
            latest_failure_class: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AcceptanceCoverageStatus {
    Covered,
    Stale,
    Failed,
    Unverified,
    Unmapped,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AcceptanceCoverage {
    pub criterion: String,
    pub status: AcceptanceCoverageStatus,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub verifier_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub verifier_commands: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_status: Option<VerificationStatus>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RequirementNode {
    pub requirement_id: String,
    #[serde(default)]
    pub source: RequirementSource,
    pub text: String,
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
    pub status: AcceptanceCoverageStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_status: Option<VerificationStatus>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RequirementSource {
    #[default]
    AcceptanceCriterion,
    DurableMemory,
    ProjectContract,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RequirementEvidenceRef {
    pub evidence_id: String,
    #[serde(default)]
    pub role: RequirementEvidenceRole,
    #[serde(default)]
    pub necessity: RequirementEvidenceNecessity,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RequirementEvidenceRole {
    #[default]
    SupportingEvidence,
    RequirementSource,
    VerificationResult,
    DurableMemorySource,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RequirementEvidenceNecessity {
    #[default]
    Necessary,
    Correlated,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VerificationRunStatus {
    Passed,
    Failed,
    TimedOut,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VerificationFailureClass {
    Deterministic,
    Flaky,
    Environment,
    Compile,
    Assertion,
    CoverageGap,
    Timeout,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VerifierRun {
    pub verifier_run_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verifier_id: Option<String>,
    pub command: String,
    pub status: VerificationRunStatus,
    pub started_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    pub output_digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_class: Option<VerificationFailureClass>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum ControlActionKind {
    ContinueWorking,
    RetryAgent,
    ForceVerification,
    RunProbe,
    BlockProgressUntilTraceAndVerification,
    SendFollowUp,
    SpawnJudgeAgent,
    SpawnFreshAgent,
    SwitchAgent,
    AskUser,
    Pause,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VerificationSuite {
    Full,
    Targeted,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeValidationSurface {
    WebUi,
    MobileApp,
    NativeGui,
    SystemComponent,
    MlSystem,
}

impl RuntimeValidationSurface {
    pub fn label(self) -> &'static str {
        match self {
            RuntimeValidationSurface::WebUi => "web UI",
            RuntimeValidationSurface::MobileApp => "mobile app",
            RuntimeValidationSurface::NativeGui => "native GUI",
            RuntimeValidationSurface::SystemComponent => "system component",
            RuntimeValidationSurface::MlSystem => "ML system",
        }
    }

    pub fn kind_label(self) -> &'static str {
        match self {
            RuntimeValidationSurface::WebUi => "web_ui",
            RuntimeValidationSurface::MobileApp => "mobile_app",
            RuntimeValidationSurface::NativeGui => "native_gui",
            RuntimeValidationSurface::SystemComponent => "system_component",
            RuntimeValidationSurface::MlSystem => "ml_system",
        }
    }

    pub fn evidence_phrase(self) -> &'static str {
        match self {
            RuntimeValidationSurface::WebUi => {
                "browser, route, console, Playwright, Cypress, or equivalent web evidence"
            }
            RuntimeValidationSurface::MobileApp => {
                "simulator, emulator, device, Appium, Detox, Maestro, or platform test evidence"
            }
            RuntimeValidationSurface::NativeGui => {
                "desktop GUI smoke, e2e, screenshot, or rendered-state evidence"
            }
            RuntimeValidationSurface::SystemComponent => {
                "service, daemon, container, healthcheck, or integration evidence"
            }
            RuntimeValidationSurface::MlSystem => {
                "model eval, benchmark, golden dataset, inference smoke, or dataset check evidence"
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProbeSpec {
    LocalEvidence {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<String>,
    },
    RuntimeValidation {
        surface: RuntimeValidationSurface,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<String>,
    },
    #[doc(hidden)]
    BrowserValidation {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<String>,
    },
    RepoInspection {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        target: Option<String>,
    },
    TargetedTest {
        command: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ControlAction {
    ContinueWorking,
    RetryAgent {
        target_agent: Option<String>,
        max_attempts: u8,
    },
    ForceVerification {
        suite: VerificationSuite,
        blocking: bool,
    },
    RunProbe {
        probe: ProbeSpec,
    },
    BlockProgressUntilTraceAndVerification {
        reason: String,
    },
    SendFollowUp {
        target_agent: Option<String>,
    },
    SpawnJudgeAgent {
        target_agent: Option<String>,
    },
    SpawnFreshAgent {
        target_agent: Option<String>,
    },
    SwitchAgent {
        target_agent: String,
    },
    AskUser {
        question: String,
    },
    Pause {
        reason: String,
    },
}

impl ControlAction {
    pub fn kind(&self) -> ControlActionKind {
        match self {
            Self::ContinueWorking => ControlActionKind::ContinueWorking,
            Self::RetryAgent { .. } => ControlActionKind::RetryAgent,
            Self::ForceVerification { .. } => ControlActionKind::ForceVerification,
            Self::RunProbe { .. } => ControlActionKind::RunProbe,
            Self::BlockProgressUntilTraceAndVerification { .. } => {
                ControlActionKind::BlockProgressUntilTraceAndVerification
            }
            Self::SendFollowUp { .. } => ControlActionKind::SendFollowUp,
            Self::SpawnJudgeAgent { .. } => ControlActionKind::SpawnJudgeAgent,
            Self::SpawnFreshAgent { .. } => ControlActionKind::SpawnFreshAgent,
            Self::SwitchAgent { .. } => ControlActionKind::SwitchAgent,
            Self::AskUser { .. } => ControlActionKind::AskUser,
            Self::Pause { .. } => ControlActionKind::Pause,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AdvisorDecision {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub diagnosis_id: Option<String>,
    pub dominant_entropy: EntropyKind,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub entropy_scores: BTreeMap<EntropyKind, AdvisorEntropyEstimate>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub top_evidence: Vec<AdvisorEvidenceRef>,
    #[serde(default)]
    pub cited_evidence_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub missing_evidence: Vec<String>,
    pub proposed_action: ControlAction,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub expected_entropy_delta: Vec<EntropyDelta>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub packet_intent: Option<String>,
    pub packet_draft: AdvisorPacketDraft,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ask_user: Option<serde_json::Value>,
    pub confidence: f32,
    #[serde(default)]
    pub raw: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdvisorEntropyEstimate {
    pub score: u8,
    pub confidence: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdvisorEvidenceRef {
    pub event_id: String,
    pub why_it_matters: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdvisorPacketDraft {
    pub urgency: PacketUrgency,
    pub summary: String,
    pub instructions: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence_refs: Vec<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PacketUrgency {
    Urgent,
    FollowUp,
    Context,
    Verification,
}

impl PacketUrgency {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Urgent => "urgent",
            Self::FollowUp => "follow-up",
            Self::Context => "context",
            Self::Verification => "verification",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PacketInstructionPriority {
    Must,
    Should,
    May,
}

impl PacketInstructionPriority {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Must => "MUST",
            Self::Should => "SHOULD",
            Self::May => "MAY",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PacketInstruction {
    pub priority: PacketInstructionPriority,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ControlPacket {
    pub packet_id: String,
    pub target_agent: String,
    pub urgency: PacketUrgency,
    pub title: String,
    pub summary: String,
    pub instructions: Vec<PacketInstruction>,
    pub evidence_refs: Vec<String>,
    pub forbidden: Vec<String>,
    pub success_criteria: Vec<String>,
    #[serde(default)]
    pub preconditions: PacketPreconditions,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PacketPreconditions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_head: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adapter: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_session_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AdviceRun {
    pub advice_id: String,
    pub case_file_id: String,
    pub advisor_used: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub advisor_error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub advisor_decision: Option<AdvisorDecision>,
    #[serde(default)]
    pub validation_outcome: ValidationOutcome,
    pub final_action: ControlAction,
    #[serde(default)]
    pub control_rationale: ControlRationale,
    pub packet: ControlPacket,
    #[serde(default)]
    pub dispatch_result: DispatchResult,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub packet_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ControlRationale {
    pub selected_action: ControlActionKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dominant_entropy: Option<EntropyKind>,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub expected_entropy_delta: Vec<EntropyDelta>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requirement_ids: Vec<String>,
}

impl Default for ControlRationale {
    fn default() -> Self {
        Self {
            selected_action: ControlActionKind::ContinueWorking,
            dominant_entropy: None,
            reason: "legacy advice missing monitor control rationale".into(),
            expected_entropy_delta: Vec::new(),
            evidence_ids: Vec::new(),
            requirement_ids: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HandoffRun {
    pub case_file: ControlCaseFile,
    pub packet: ControlPacket,
    pub dispatch_result: DispatchResult,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub packet_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DispatchResult {
    pub dispatch_id: String,
    pub packet_id: String,
    pub target_agent: String,
    pub status: DispatchStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl Default for DispatchResult {
    fn default() -> Self {
        Self {
            dispatch_id: "legacy-missing-dispatch".into(),
            packet_id: String::new(),
            target_agent: String::new(),
            status: DispatchStatus::Failed,
            path: None,
            reason: Some("dispatch result missing from legacy advice record".into()),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DispatchStatus {
    OutboxWritten,
    SuppressedDuplicate,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorktreeLockRequest {
    pub worktree: String,
    pub owner_agent: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorktreeLock {
    pub lock_id: String,
    pub worktree: String,
    pub owner_agent: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    pub acquired_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum WorktreeLockResult {
    Acquired(WorktreeLock),
    Conflict { existing: WorktreeLock },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorktreeLockEvent {
    pub kind: String,
    pub lock: WorktreeLock,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_owner: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ValidationOutcome {
    Approved(ControlAction),
    Modified {
        original: ControlAction,
        replacement: ControlAction,
        reason: String,
    },
    Denied {
        original: ControlAction,
        reason: String,
    },
}

impl Default for ValidationOutcome {
    fn default() -> Self {
        Self::Approved(ControlAction::ContinueWorking)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AdvisorValidationError {
    #[error("advisor cited evidence id not present in case file: {evidence_id}")]
    UnknownEvidence { evidence_id: String },
    #[error("advisor dominant entropy {kind:?} is missing from entropy_scores")]
    MissingDominantEntropyScore { kind: EntropyKind },
    #[error("advisor entropy score for {kind:?} is outside 0..100")]
    InvalidEntropyScore { kind: EntropyKind },
    #[error("advisor expected entropy delta for {kind:?} is outside -100..100: {delta}")]
    InvalidExpectedEntropyDelta { kind: EntropyKind, delta: i16 },
    #[error("advisor returned duplicate entropy delta for {kind:?}")]
    DuplicateExpectedEntropyDelta { kind: EntropyKind },
    #[error("advisor confidence is outside 0.0..1.0")]
    InvalidConfidence,
    #[error("advisor proposed forbidden action: {action:?}")]
    ForbiddenAction { action: ControlActionKind },
    #[error("advisor targeted unsupported agent {agent}: {reason}")]
    UnsupportedTargetAgent { agent: String, reason: String },
    #[error("advisor proposed unsupported probe spec: {kind}")]
    UnsupportedProbeSpec { kind: &'static str },
    #[error("advisor packet contains tainted content")]
    TaintedPacket,
}

#[derive(Debug, thiserror::Error)]
pub enum AdviceError {
    #[error("project store: {0}")]
    Store(#[from] StoreError),
    #[error("project config: {0}")]
    Config(#[from] ProjectConfigError),
    #[error("advisor request: {0}")]
    Advisor(#[from] AdvisorClientError),
    #[error("probe execution: {0}")]
    Probe(#[from] ProbeError),
    #[error("adapter capabilities do not allow writable handoff to {agent}: {reason}")]
    UnsafeAdapterTarget { agent: String, reason: String },
}

#[derive(Debug, thiserror::Error)]
pub enum MemoryPromotionError {
    #[error("project store: {0}")]
    Store(#[from] StoreError),
    #[error("project config: {0}")]
    Config(#[from] ProjectConfigError),
    #[error("memory candidate not found: {memory_id}")]
    CandidateNotFound { memory_id: String },
    #[error("memory candidate already governed: {memory_id} has latest status {status:?}")]
    AlreadyGoverned {
        memory_id: String,
        status: MemoryStatus,
    },
    #[error("memory promotion requires a trusted source, got {memory_source:?}")]
    UntrustedSource { memory_source: MemorySource },
    #[error("memory candidate {memory_id} is tainted and cannot be promoted")]
    TaintedClaim { memory_id: String },
    #[error(
        "memory candidate conflict: {memory_id} conflicts with active durable memory {existing_memory_id}"
    )]
    ConflictingClaim {
        memory_id: String,
        existing_memory_id: String,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum VerifyError {
    #[error("project store: {0}")]
    Store(#[from] StoreError),
    #[error("project config: {0}")]
    Config(#[from] ProjectConfigError),
    #[error("unknown verifier id: {0}")]
    UnknownVerifier(String),
    #[error("spawn verifier command: {0}")]
    Spawn(#[source] std::io::Error),
    #[error("wait for verifier command: {0}")]
    Wait(#[source] std::io::Error),
    #[error("kill timed-out verifier command: {0}")]
    Kill(#[source] std::io::Error),
    #[error("prepare verifier process group: {0}")]
    ProcessGroup(#[source] std::io::Error),
    #[error("read verifier output: {0}")]
    ReadOutput(#[source] std::io::Error),
    #[error("verifier output reader thread panicked")]
    OutputReaderPanicked,
}

#[derive(Debug, thiserror::Error)]
pub enum RepoAuditError {
    #[error("project store: {0}")]
    Store(#[from] StoreError),
    #[error("run git {args}: {source}")]
    GitSpawn {
        args: String,
        #[source]
        source: std::io::Error,
    },
    #[error("git {args} failed: {stderr}")]
    GitFailed { args: String, stderr: String },
    #[error("git output is not utf-8 for {args}: {source}")]
    GitUtf8 {
        args: String,
        #[source]
        source: std::string::FromUtf8Error,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum AdvisorClientError {
    #[error("advisor endpoint is empty")]
    EmptyEndpoint,
    #[error("advisor model is empty")]
    EmptyModel,
    #[error("advisor api key env var is not set: {0}")]
    MissingApiKey(String),
    #[error("advisor credential file is not configured for {kind:?}")]
    MissingCredentialFile { kind: AdvisorCredentialSource },
    #[error("read advisor credential {path}: {source}")]
    CredentialRead {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("decode advisor credential {path}: {source}")]
    CredentialJson {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("advisor credential token not found for {kind:?} in {path}")]
    MissingCredentialToken {
        kind: AdvisorCredentialSource,
        path: PathBuf,
    },
    #[error(
        "advisor credential file {path} points at local CLI auth directory {cli_dir}; configure a dedicated coding-plan credential profile instead"
    )]
    LocalCliAuthCredentialProfile {
        path: PathBuf,
        cli_dir: &'static str,
    },
    #[error(
        "advisor credential source {kind:?} is unsupported; use CodingPlan for dedicated provider credential profiles"
    )]
    UnsupportedCredentialSource { kind: AdvisorCredentialSource },
    #[error(
        "JWT/OAuth-style coding-plan credential is not compatible with advisor endpoint {endpoint}; configure a provider endpoint that accepts coding-plan bearer tokens or import an API-key-style dedicated advisor credential"
    )]
    IncompatibleCodingPlanCredentialEndpoint { endpoint: String },
    #[error("invalid advisor endpoint: {0}")]
    InvalidEndpoint(String),
    #[error("advisor transport: {0}")]
    Transport(String),
    #[error("connect advisor endpoint: {0}")]
    Connect(#[source] std::io::Error),
    #[error("write advisor request: {0}")]
    Write(#[source] std::io::Error),
    #[error("read advisor response: {0}")]
    Read(#[source] std::io::Error),
    #[error("advisor returned non-success status: {0}")]
    HttpStatus(String),
    #[error("decode advisor response json: {0}")]
    ResponseJson(#[source] serde_json::Error),
    #[error("advisor response missing message content")]
    MissingContent,
    #[error("decode advisor decision json: {0}")]
    DecisionJson(#[source] serde_json::Error),
    #[error("advisor validation: {0}")]
    Validation(#[from] AdvisorValidationError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DashboardOptions {
    pub recent_limit: usize,
    pub now: Option<String>,
    pub stale_after_secs: Option<i64>,
}

impl DashboardSnapshot {
    pub fn load(store_root: impl AsRef<Path>, recent_limit: usize) -> Result<Self, StoreError> {
        Self::load_with_options(
            store_root,
            DashboardOptions {
                recent_limit,
                now: None,
                stale_after_secs: None,
            },
        )
    }

    pub fn load_with_options(
        store_root: impl AsRef<Path>,
        options: DashboardOptions,
    ) -> Result<Self, StoreError> {
        let store_root = store_root.as_ref();
        let mut agent_scores = HashMap::<String, i32>::new();
        let mut agent_events = HashMap::<String, usize>::new();
        let mut agent_interventions = HashMap::<String, usize>::new();
        let mut agent_last_seen = HashMap::<String, String>::new();
        let mut agent_latest_event_health = HashMap::<String, AgentEventHealth>::new();
        let events = read_jsonl_summary::<Event, _>(
            &store_root.join("events.jsonl"),
            options.recent_limit,
            |event| {
                agent_scores.entry(event.agent.clone()).or_default();
                agent_events
                    .entry(event.agent.clone())
                    .and_modify(|count| *count += 1)
                    .or_insert(1);
                if let Some(time) = &event.time {
                    agent_last_seen
                        .entry(event.agent.clone())
                        .and_modify(|last_seen| {
                            if time > last_seen {
                                *last_seen = time.clone();
                            }
                        })
                        .or_insert_with(|| time.clone());
                }
                if event.kind == EventKind::CommandResult && event.exit_code == Some(0) {
                    agent_latest_event_health
                        .insert(event.agent.clone(), AgentEventHealth::Healthy);
                } else if matches!(
                    event.kind,
                    EventKind::ModelMessage | EventKind::CommandOutput
                ) && let Some(content) = event.content.as_deref()
                    && !content.trim().is_empty()
                {
                    agent_latest_event_health.insert(
                        event.agent.clone(),
                        if looks_like_service_failure(content) {
                            AgentEventHealth::ServiceFailure
                        } else {
                            AgentEventHealth::Healthy
                        },
                    );
                }
            },
        )?;
        let interventions = read_jsonl_summary::<Intervention, _>(
            &store_root.join("interventions.jsonl"),
            options.recent_limit,
            |intervention| {
                if let Some(agent) = &intervention.agent {
                    if intervention.kind == InterventionKind::ServiceFailure
                        && intervention.action == Action::SwitchAgent
                    {
                        return;
                    }
                    let recovered_service_failure = intervention.kind
                        == InterventionKind::ServiceFailure
                        && agent_latest_event_health
                            .get(agent)
                            .is_some_and(|health| *health == AgentEventHealth::Healthy);
                    if recovered_service_failure {
                        return;
                    }
                    let score = agent_scores.entry(agent.clone()).or_default();
                    agent_interventions
                        .entry(agent.clone())
                        .and_modify(|count| *count += 1)
                        .or_insert(1);
                    match intervention.kind {
                        InterventionKind::PrematureStop => *score -= 2,
                        InterventionKind::ServiceFailure => *score -= 1,
                        InterventionKind::AgentDegraded => *score -= 3,
                        InterventionKind::SuspiciousChange => *score -= 2,
                    }
                }
            },
        )?;
        let verifier_runs = read_jsonl_summary::<VerifierRun, _>(
            &store_root.join("verifier-runs.jsonl"),
            options.recent_limit,
            |_| {},
        )?;
        let probe_runs = read_jsonl_summary::<ProbeRun, _>(
            &store_root.join("probe-runs.jsonl"),
            options.recent_limit,
            |_| {},
        )?;
        let mut repo_hunk_file_accumulator =
            repo_history::RepoHunkFileSummaryAccumulator::default();
        let repo_hunks = read_jsonl_summary::<RepoHunkHistoryEntry, _>(
            &store_root.join("repo-hunks.jsonl"),
            options.recent_limit,
            |hunk| {
                repo_hunk_file_accumulator.observe(hunk);
            },
        )?;
        let mut repo_hunk_files = repo_hunk_file_accumulator.finish();
        let repo_hunk_file_count = repo_hunk_files.len();
        repo_hunk_files.truncate(options.recent_limit);
        let requirements = requirements::load_requirement_graph_from_store_root(
            store_root,
            store_root.display().to_string(),
            RequirementGraphQuery {
                limit: options.recent_limit,
                ..RequirementGraphQuery::default()
            },
        )?;
        let dev_history = read_jsonl_summary::<DevHistoryReport, _>(
            &store_root.join("dev-history.jsonl"),
            options.recent_limit,
            |_| {},
        )?;
        let mut decision_trails = load_decision_trails(store_root)?;
        let decision_trail_count = decision_trails.len();
        if decision_trails.len() > options.recent_limit {
            decision_trails =
                decision_trails.split_off(decision_trails.len() - options.recent_limit);
        }
        let worktree_lock_events = read_jsonl_summary::<WorktreeLockEvent, _>(
            &store_root.join("locks.jsonl"),
            options.recent_limit,
            |_| {},
        )?;
        let advice_count = count_jsonl_lines(&store_root.join("advice.jsonl"))?;
        let packet_count = count_jsonl_lines(&store_root.join("packets.jsonl"))?;
        let dispatch_count = count_jsonl_lines(&store_root.join("dispatch.jsonl"))?;
        let outcome_count = count_jsonl_lines(&store_root.join("outcomes.jsonl"))?;
        let design_count = count_jsonl_lines(&store_root.join("design.jsonl"))?;
        let trace_count = count_jsonl_lines(&store_root.join("trace.jsonl"))?;

        let mut active_agents = agent_scores.keys().cloned().collect::<Vec<_>>();
        active_agents.sort();

        let mut agent_health = agent_scores
            .iter()
            .map(|(agent, score)| AgentHealth {
                agent: agent.clone(),
                score: *score,
            })
            .collect::<Vec<_>>();
        agent_health.sort_by(|left, right| {
            left.score
                .cmp(&right.score)
                .then_with(|| left.agent.cmp(&right.agent))
        });

        let rows = dashboard_rows(DashboardRowsInput {
            events: &events.recent,
            interventions: &interventions.recent,
            verifier_runs: &verifier_runs.recent,
            probe_runs: &probe_runs.recent,
            repo_hunks: &repo_hunks.recent,
            repo_hunk_files: &repo_hunk_files,
            requirements: &requirements.requirements,
            requirement_proofs: &requirements.proofs,
            dev_history: &dev_history.recent,
            decision_trails: &decision_trails,
            worktree_lock_events: &worktree_lock_events.recent,
        });
        let advisor_status = dashboard_advisor_status(store_root);
        let severity = max_dashboard_severity(
            dashboard_severity(&agent_health, interventions.count, &rows),
            advisor_status.severity,
        );
        let agent_sessions = agent_sessions(
            &agent_scores,
            &agent_events,
            &agent_interventions,
            &agent_last_seen,
            options.now.as_deref(),
            options.stale_after_secs,
        );

        Ok(Self {
            severity,
            advisor_status,
            event_count: events.count,
            intervention_count: interventions.count,
            verifier_run_count: verifier_runs.count,
            probe_run_count: probe_runs.count,
            repo_hunk_history_count: repo_hunks.count,
            repo_hunk_file_count,
            requirement_count: requirements.requirement_count,
            dev_history_count: dev_history.count,
            decision_trail_count,
            advice_count,
            packet_count,
            dispatch_count,
            outcome_count,
            lock_event_count: worktree_lock_events.count,
            design_count,
            trace_count,
            active_agents,
            agent_health,
            agent_sessions,
            rows,
            recent_events: events.recent,
            recent_interventions: interventions.recent,
            recent_verifier_runs: verifier_runs.recent,
            recent_probe_runs: probe_runs.recent,
            recent_repo_hunks: repo_hunks.recent,
            recent_repo_hunk_files: repo_hunk_files,
            recent_requirements: requirements.requirements,
            recent_requirement_proofs: requirements.proofs,
            recent_dev_history: dev_history.recent,
            recent_decision_trails: decision_trails,
            recent_worktree_lock_events: worktree_lock_events.recent,
        })
    }

    pub fn filtered_rows(&self, filter: &DashboardFilter) -> Vec<&DashboardRow> {
        self.rows.iter().filter(|row| filter.matches(row)).collect()
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DashboardRowKind {
    Event,
    Intervention,
    VerifierRun,
    ProbeRun,
    RepoHunkFile,
    RepoHunk,
    Requirement,
    DevHistory,
    DecisionTrail,
    WorktreeLock,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DashboardRow {
    pub number: usize,
    pub kind: DashboardRowKind,
    pub severity: DashboardSeverity,
    pub agent: Option<String>,
    pub protocol: String,
    pub summary: String,
    pub detail: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DashboardFilter {
    kind: Option<DashboardRowKind>,
    agent: Option<String>,
    severity: Option<DashboardSeverity>,
    text_terms: Vec<String>,
}

impl DashboardFilter {
    pub fn parse(input: &str) -> Result<Self, DashboardFilterError> {
        let mut filter = Self::default();
        for token in input.split_whitespace() {
            if let Some((field, value)) = token.split_once(':') {
                match field {
                    "kind" => filter.kind = Some(parse_row_kind(value)?),
                    "agent" => filter.agent = Some(value.to_lowercase()),
                    "severity" => filter.severity = Some(parse_dashboard_severity(value)?),
                    "text" => filter.text_terms.push(value.to_lowercase()),
                    _ => {
                        return Err(DashboardFilterError::UnknownField {
                            field: field.into(),
                        });
                    }
                }
            } else {
                filter.text_terms.push(token.to_lowercase());
            }
        }
        Ok(filter)
    }

    pub fn matches(&self, row: &DashboardRow) -> bool {
        if self.kind.is_some_and(|kind| kind != row.kind) {
            return false;
        }
        if self
            .severity
            .is_some_and(|severity| severity != row.severity)
        {
            return false;
        }
        if let Some(agent) = &self.agent
            && row.agent.as_deref().map(str::to_lowercase).as_ref() != Some(agent)
        {
            return false;
        }
        let haystack = format!(
            "{} {} {} {}",
            row.protocol,
            row.agent.as_deref().unwrap_or_default(),
            row.summary,
            row.detail
        )
        .to_lowercase();
        self.text_terms.iter().all(|term| haystack.contains(term))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DashboardFilterError {
    #[error("unknown dashboard filter field: {field}")]
    UnknownField { field: String },
    #[error("invalid dashboard filter value for {field}: {value}")]
    InvalidValue { field: String, value: String },
}
