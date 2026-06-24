use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
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
mod dev_history;
mod events;
mod injection;
mod outcome_recording;
mod outcomes;
mod packet_dispatch;
mod probe;
mod redaction;
mod repo_history;
mod requirements;
mod validation_surface;
mod verifier;

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
    ProjectConfig, ProjectConfigError, ProjectConfigWriteError, SecurityConfig, VerificationScope,
    VerifierConfig, import_coding_plan_advisor_credentials, import_local_agent_configs,
    write_advisor_endpoint_config, write_verifier_config,
};
pub use dev_history::{
    DevHistoryAnalysisOptions, DevHistoryCount, DevHistoryError, DevHistoryFinding,
    DevHistoryRawCopyError, DevHistoryRawExportFile, DevHistoryRawExportIncluded,
    DevHistoryRawExportOptions, DevHistoryRawExportReport, DevHistoryRawMatchingRules,
    DevHistoryReport, DevHistorySourceReport, analyze_local_dev_history, export_raw_dev_history,
};
pub use events::{CapturedStream, Event, EventKind, command_output_event, command_result_event};
pub use injection::{
    InjectionFile, InjectionInstallError, InjectionPlan, InstallMode, injection_plan_for,
    injection_plan_for_workspace, install_agent_injection, install_injection_plan,
};
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
use validation_surface::{
    ValidationSurface, is_ui_validation_relevant_file, ordered_validation_surfaces,
    push_validation_surface, validation_surface_for_path, validation_surfaces_for_command,
    validation_surfaces_for_event,
};
pub use verifier::run_verifier;

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

const REPO_AUDIT_MAX_MATCHING_TRACES_PER_CHANGE: usize = 5;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentHealth {
    pub agent: String,
    pub score: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AgentEventHealth {
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

    fn score_mut(&mut self, kind: EntropyKind) -> &mut EntropyScore {
        self.scores
            .iter_mut()
            .find(|score| score.kind == kind)
            .expect("baseline entropy vector should contain every kind")
    }

    fn raise(
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
    fn is_trusted_promotion_source(self) -> bool {
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
    fn label(self) -> &'static str {
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

#[derive(Debug)]
pub struct ProjectStore {
    workspace_root: PathBuf,
    root: PathBuf,
    temp_dir: PathBuf,
}

const JSONL_APPEND_LOCK_ATTEMPTS: usize = 2_000;
const JSONL_APPEND_LOCK_SLEEP: Duration = Duration::from_millis(5);

struct JsonlAppendLock {
    path: PathBuf,
    file: Option<fs::File>,
}

impl Drop for JsonlAppendLock {
    fn drop(&mut self) {
        drop(self.file.take());
        let _ = fs::remove_file(&self.path);
    }
}

impl ProjectStore {
    pub fn open(workspace_root: impl AsRef<Path>) -> Result<Self, StoreError> {
        let workspace_root = workspace_root.as_ref().to_path_buf();
        let root = workspace_root.join(".agent-monitor");
        let temp_dir = root.join("tmp");
        fs::create_dir_all(&temp_dir).map_err(|source| StoreError::CreateDir {
            path: temp_dir.clone(),
            source,
        })?;
        Ok(Self {
            workspace_root,
            root,
            temp_dir,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn temp_dir(&self) -> PathBuf {
        self.temp_dir.clone()
    }

    pub fn append_event(&mut self, event: &Event) -> Result<(), StoreError> {
        self.append_event_and_return(event).map(|_| ())
    }

    fn append_event_and_return(&mut self, event: &Event) -> Result<Event, StoreError> {
        let _lock = self.acquire_jsonl_append_lock("events.jsonl")?;
        let event = self.prepare_event_for_persistence(event)?;
        self.append_prepared_event(&event)?;
        Ok(event)
    }

    fn prepare_event_for_persistence(&self, event: &Event) -> Result<Event, StoreError> {
        let mut event = event.clone();
        if event.event_id.as_deref().is_none_or(str::is_empty) {
            event.event_id = Some(format!("event-{}", current_id_fragment()));
        }
        if event.seq.is_none() {
            event.seq = Some(self.next_event_seq()?);
        }
        stamp_store_event_provenance(&mut event, &self.workspace_root);
        Ok(event)
    }

    fn next_event_seq(&self) -> Result<u64, StoreError> {
        let max_seq = read_all_jsonl::<Event>(&self.root.join("events.jsonl"))?
            .into_iter()
            .filter_map(|event| event.seq)
            .max()
            .unwrap_or(0);
        Ok(max_seq.saturating_add(1))
    }

    fn acquire_jsonl_append_lock(&self, name: &str) -> Result<JsonlAppendLock, StoreError> {
        let path = self.jsonl_append_lock_path(name);
        for _ in 0..JSONL_APPEND_LOCK_ATTEMPTS {
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(file) => {
                    return Ok(JsonlAppendLock {
                        path,
                        file: Some(file),
                    });
                }
                Err(source)
                    if matches!(
                        source.kind(),
                        std::io::ErrorKind::AlreadyExists | std::io::ErrorKind::PermissionDenied
                    ) =>
                {
                    thread::sleep(JSONL_APPEND_LOCK_SLEEP);
                }
                Err(source) => {
                    return Err(StoreError::Append {
                        path: path.clone(),
                        source,
                    });
                }
            }
        }
        Err(StoreError::JsonlAppendLockTimeout { path })
    }

    fn jsonl_append_lock_path(&self, name: &str) -> PathBuf {
        let stem = name.strip_suffix(".jsonl").unwrap_or(name);
        self.root.join(format!("{}.lock", safe_slug(stem)))
    }

    fn append_prepared_event(&mut self, event: &Event) -> Result<(), StoreError> {
        let event = storage_redacted_event(event);
        self.append_jsonl_unlocked("events.jsonl", &event)
    }

    pub fn append_intervention(&mut self, intervention: &Intervention) -> Result<(), StoreError> {
        self.append_jsonl("interventions.jsonl", intervention)
    }

    pub fn append_design(&mut self, entry: &DesignEntry) -> Result<(), StoreError> {
        self.append_jsonl("design.jsonl", entry)
    }

    pub fn append_memory(&mut self, memory: &MemoryCandidate) -> Result<(), StoreError> {
        self.append_jsonl("memories.jsonl", memory)
    }

    pub fn append_trace(&mut self, entry: &TraceEntry) -> Result<(), StoreError> {
        let entry = storage_redacted_trace(entry);
        self.append_jsonl("trace.jsonl", &entry)
    }

    pub fn append_case_file(&mut self, case_file: &ControlCaseFile) -> Result<(), StoreError> {
        self.append_jsonl("case-files.jsonl", case_file)
    }

    pub fn append_advice(&mut self, advice: &AdviceRun) -> Result<(), StoreError> {
        self.validate_advice_for_persistence(advice)?;
        self.append_jsonl("advice.jsonl", advice)
    }

    pub fn append_packet(&mut self, packet: &ControlPacket) -> Result<(), StoreError> {
        self.validate_control_packet_for_persistence(packet)?;
        self.append_packet_unchecked(packet)
    }

    fn append_packet_unchecked(&mut self, packet: &ControlPacket) -> Result<(), StoreError> {
        self.append_jsonl("packets.jsonl", packet)
    }

    pub fn append_dispatch(&mut self, dispatch: &DispatchResult) -> Result<(), StoreError> {
        self.append_jsonl("dispatch.jsonl", dispatch)
    }

    pub fn append_action_outcome(&mut self, outcome: &ActionOutcome) -> Result<(), StoreError> {
        self.append_jsonl("outcomes.jsonl", outcome)
    }

    pub fn append_verifier_run(&mut self, run: &VerifierRun) -> Result<(), StoreError> {
        self.append_jsonl("verifier-runs.jsonl", run)
    }

    pub fn append_probe_run(&mut self, run: &ProbeRun) -> Result<(), StoreError> {
        self.append_jsonl("probe-runs.jsonl", run)
    }

    pub fn append_repo_hunk_history(
        &mut self,
        entry: &RepoHunkHistoryEntry,
    ) -> Result<(), StoreError> {
        self.append_jsonl("repo-hunks.jsonl", entry)
    }

    pub fn append_dev_history_report(
        &mut self,
        report: &DevHistoryReport,
    ) -> Result<(), StoreError> {
        self.append_jsonl("dev-history.jsonl", report)
    }

    pub fn dispatch_control_packet(
        &mut self,
        packet: &ControlPacket,
    ) -> Result<DispatchResult, StoreError> {
        self.validate_control_packet_for_persistence(packet)?;
        let agent_dir = self.control_packet_agent_dir(packet);
        fs::create_dir_all(&agent_dir).map_err(|source| StoreError::CreateDir {
            path: agent_dir.clone(),
            source,
        })?;
        let path = immutable_control_packet_path(&agent_dir, packet);
        let rendered = render_control_packet(packet);
        write_new_control_packet_file(&path, &rendered)?;
        self.append_packet_unchecked(packet)?;
        let dispatch = DispatchResult {
            dispatch_id: format!("dispatch-{}", current_id_fragment()),
            packet_id: packet.packet_id.clone(),
            target_agent: packet.target_agent.clone(),
            status: DispatchStatus::OutboxWritten,
            path: Some(path.display().to_string()),
            reason: None,
        };
        publish_latest_control_packet(&agent_dir, packet, &rendered)?;
        self.append_dispatch(&dispatch)?;
        Ok(dispatch)
    }

    pub fn try_acquire_worktree_lock(
        &mut self,
        request: &WorktreeLockRequest,
    ) -> Result<WorktreeLockResult, StoreError> {
        let lock_dir = self.root.join("locks").join("worktrees");
        fs::create_dir_all(&lock_dir).map_err(|source| StoreError::CreateDir {
            path: lock_dir.clone(),
            source,
        })?;
        let path = worktree_lock_path(&self.root, &request.worktree);

        let lock = WorktreeLock {
            lock_id: format!("lock-{}", current_id_fragment()),
            worktree: request.worktree.clone(),
            owner_agent: request.owner_agent.clone(),
            session: request.session.clone(),
            acquired_at: current_utc_timestamp().unwrap_or_else(|| "1970-01-01T00:00:00Z".into()),
        };
        let mut file = match OpenOptions::new().create_new(true).write(true).open(&path) {
            Ok(file) => file,
            Err(source) if source.kind() == std::io::ErrorKind::AlreadyExists => {
                let existing = read_worktree_lock(&path)?;
                self.append_lock_event(&WorktreeLockEvent {
                    kind: "conflict".into(),
                    lock: existing.clone(),
                    requested_owner: Some(request.owner_agent.clone()),
                })?;
                return Ok(WorktreeLockResult::Conflict { existing });
            }
            Err(source) => {
                return Err(StoreError::Append {
                    path: path.clone(),
                    source,
                });
            }
        };
        serde_json::to_writer(&mut file, &lock).map_err(|source| StoreError::Encode {
            path: path.clone(),
            source,
        })?;
        file.write_all(b"\n").map_err(|source| StoreError::Append {
            path: path.clone(),
            source,
        })?;
        self.append_lock_event(&WorktreeLockEvent {
            kind: "acquired".into(),
            lock: lock.clone(),
            requested_owner: None,
        })?;
        Ok(WorktreeLockResult::Acquired(lock))
    }

    pub fn release_worktree_lock(
        &mut self,
        worktree: &str,
        lock_id: &str,
    ) -> Result<bool, StoreError> {
        let path = worktree_lock_path(&self.root, worktree);
        if !path.exists() {
            return Ok(false);
        }
        let lock = read_worktree_lock(&path)?;
        if lock.lock_id != lock_id {
            return Ok(false);
        }
        fs::remove_file(&path).map_err(|source| StoreError::Remove {
            path: path.clone(),
            source,
        })?;
        self.append_lock_event(&WorktreeLockEvent {
            kind: "released".into(),
            lock,
            requested_owner: None,
        })?;
        Ok(true)
    }

    pub fn release_stale_worktree_locks(
        &mut self,
        stale_after_secs: i64,
    ) -> Result<Vec<WorktreeLock>, StoreError> {
        if stale_after_secs <= 0 {
            return Ok(Vec::new());
        }
        let Some(now) = current_utc_seconds() else {
            return Ok(Vec::new());
        };
        let lock_dir = self.root.join("locks").join("worktrees");
        if !lock_dir.exists() {
            return Ok(Vec::new());
        }

        let mut released = Vec::new();
        for entry in fs::read_dir(&lock_dir).map_err(|source| StoreError::Read {
            path: lock_dir.clone(),
            source,
        })? {
            let path = entry
                .map_err(|source| StoreError::Read {
                    path: lock_dir.clone(),
                    source,
                })?
                .path();
            if path.extension().is_none_or(|extension| extension != "json") {
                continue;
            }
            let lock = read_worktree_lock(&path)?;
            let Some(acquired_at) = parse_utc_seconds(&lock.acquired_at) else {
                continue;
            };
            if now - acquired_at < stale_after_secs {
                continue;
            }
            fs::remove_file(&path).map_err(|source| StoreError::Remove {
                path: path.clone(),
                source,
            })?;
            self.append_lock_event(&WorktreeLockEvent {
                kind: "expired".into(),
                lock: lock.clone(),
                requested_owner: None,
            })?;
            released.push(lock);
        }
        Ok(released)
    }

    pub(crate) fn active_worktree_lock_for(
        &self,
        worktree: &str,
    ) -> Result<Option<WorktreeLock>, StoreError> {
        let path = worktree_lock_path(&self.root, worktree);
        if !path.exists() {
            return Ok(None);
        }
        read_worktree_lock(&path).map(Some)
    }

    fn active_worktree_lock_count(&self) -> Result<usize, StoreError> {
        let lock_dir = self.root.join("locks").join("worktrees");
        if !lock_dir.exists() {
            return Ok(0);
        }

        let mut count = 0;
        for entry in fs::read_dir(&lock_dir).map_err(|source| StoreError::Read {
            path: lock_dir.clone(),
            source,
        })? {
            let path = entry
                .map_err(|source| StoreError::Read {
                    path: lock_dir.clone(),
                    source,
                })?
                .path();
            if path
                .extension()
                .is_some_and(|extension| extension == "json")
            {
                read_worktree_lock(&path)?;
                count += 1;
            }
        }
        Ok(count)
    }

    pub fn write_control_packet(&mut self, packet: &ControlPacket) -> Result<PathBuf, StoreError> {
        self.validate_control_packet_for_persistence(packet)?;
        let agent_dir = self.control_packet_agent_dir(packet);
        fs::create_dir_all(&agent_dir).map_err(|source| StoreError::CreateDir {
            path: agent_dir.clone(),
            source,
        })?;
        let path = immutable_control_packet_path(&agent_dir, packet);
        let rendered = render_control_packet(packet);
        write_new_control_packet_file(&path, &rendered)?;
        self.append_packet_unchecked(packet)?;
        publish_latest_control_packet(&agent_dir, packet, &rendered)?;
        Ok(path)
    }

    fn control_packet_agent_dir(&self, packet: &ControlPacket) -> PathBuf {
        self.root
            .join("outbox")
            .join(safe_slug(&packet.target_agent))
    }

    fn validate_control_packet_for_persistence(
        &self,
        packet: &ControlPacket,
    ) -> Result<(), StoreError> {
        self.validate_packet_preconditions(packet)?;
        validate_control_packet_is_clean(packet)?;
        self.validate_packet_evidence_refs(packet)
    }

    fn validate_advice_for_persistence(&self, advice: &AdviceRun) -> Result<(), StoreError> {
        validate_control_packet_is_clean(&advice.packet)?;
        let case_file = self.case_file_by_id(&advice.case_file_id)?;
        let refs = packet_evidence_refs(&advice.packet);
        if refs.is_empty() {
            return Ok(());
        }

        let known_ids = case_file_known_evidence_ids(&case_file);

        for evidence_ref in refs {
            if !known_ids.contains(evidence_ref) {
                return Err(StoreError::UnknownPacketEvidenceRef {
                    evidence_ref: evidence_ref.into(),
                });
            }
        }
        Ok(())
    }

    fn case_file_by_id(&self, case_file_id: &str) -> Result<ControlCaseFile, StoreError> {
        read_all_jsonl::<ControlCaseFile>(&self.root.join("case-files.jsonl"))?
            .into_iter()
            .rev()
            .find(|case_file| case_file.case_file_id == case_file_id)
            .ok_or_else(|| StoreError::AdviceCaseFileMissing {
                case_file_id: case_file_id.into(),
            })
    }

    fn validate_packet_evidence_refs(&self, packet: &ControlPacket) -> Result<(), StoreError> {
        let refs = packet_evidence_refs(packet);
        if refs.is_empty() {
            return Ok(());
        }

        let snapshot = DashboardSnapshot::load(&self.root, 500)?;
        let case_file = build_control_case_file(&self.workspace_root, &snapshot);
        let known_ids = case_file_known_evidence_ids(&case_file);

        for evidence_ref in refs {
            if !known_ids.contains(evidence_ref) {
                return Err(StoreError::UnknownPacketEvidenceRef {
                    evidence_ref: evidence_ref.into(),
                });
            }
        }
        Ok(())
    }

    fn validate_packet_preconditions(&self, packet: &ControlPacket) -> Result<(), StoreError> {
        if let Some(expected_adapter) = &packet.preconditions.adapter
            && normalize_agent_label(expected_adapter)
                != normalize_agent_label(&packet.target_agent)
        {
            return Err(StoreError::PacketPrecondition {
                field: "adapter".into(),
                expected: expected_adapter.clone(),
                actual: packet.target_agent.clone(),
            });
        }

        if let Some(expected_worktree) = &packet.preconditions.worktree {
            let actual_worktree = self.workspace_root.display().to_string();
            if normalize_path_for_match(expected_worktree)
                != normalize_path_for_match(&actual_worktree)
            {
                return Err(StoreError::PacketPrecondition {
                    field: "worktree".into(),
                    expected: expected_worktree.clone(),
                    actual: actual_worktree,
                });
            }
        }

        if let Some(expected_head) = &packet.preconditions.git_head {
            let actual_head =
                current_git_head(&self.workspace_root).unwrap_or_else(|| "<unavailable>".into());
            if expected_head != &actual_head {
                return Err(StoreError::PacketPrecondition {
                    field: "git_head".into(),
                    expected: expected_head.clone(),
                    actual: actual_head,
                });
            }
        }

        if let Some(expected_run_id) = &packet.preconditions.run_id {
            let target_agent = packet.target_agent.as_str();
            let actual_run_id = self
                .latest_event_precondition_value(target_agent, |event| event.run_id.as_deref())?
                .unwrap_or_else(|| "<unavailable>".into());
            if expected_run_id != &actual_run_id {
                return Err(StoreError::PacketPrecondition {
                    field: "run_id".into(),
                    expected: expected_run_id.clone(),
                    actual: actual_run_id,
                });
            }
        }

        if let Some(expected_session_id) = &packet.preconditions.agent_session_id {
            let target_agent = packet.target_agent.as_str();
            let actual_session_id = self
                .latest_event_precondition_value(target_agent, |event| {
                    event.agent_session_id.as_deref()
                })?
                .or(self.latest_event_precondition_value(target_agent, |event| {
                    event.session.as_deref()
                })?)
                .unwrap_or_else(|| "<unavailable>".into());
            if expected_session_id != &actual_session_id {
                return Err(StoreError::PacketPrecondition {
                    field: "agent_session_id".into(),
                    expected: expected_session_id.clone(),
                    actual: actual_session_id,
                });
            }
        }

        Ok(())
    }

    fn latest_event_precondition_value<F>(
        &self,
        target_agent: &str,
        mut extract: F,
    ) -> Result<Option<String>, StoreError>
    where
        F: FnMut(&Event) -> Option<&str>,
    {
        let target_agent = normalize_agent_label(target_agent);
        Ok(read_all_jsonl::<Event>(&self.root.join("events.jsonl"))?
            .into_iter()
            .rev()
            .filter(|event| normalize_agent_label(&event.agent) == target_agent)
            .find_map(|event| {
                extract(&event)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
            }))
    }

    fn append_jsonl<T: Serialize>(&mut self, name: &str, value: &T) -> Result<(), StoreError> {
        let _lock = self.acquire_jsonl_append_lock(name)?;
        self.append_jsonl_unlocked(name, value)
    }

    fn append_jsonl_unlocked<T: Serialize>(
        &mut self,
        name: &str,
        value: &T,
    ) -> Result<(), StoreError> {
        let path = self.root.join(name);
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|source| StoreError::Append {
                path: path.clone(),
                source,
            })?;

        serde_json::to_writer(&mut file, value).map_err(|source| StoreError::Encode {
            path: path.clone(),
            source,
        })?;
        writeln!(file).map_err(|source| StoreError::Append { path, source })?;
        Ok(())
    }

    fn append_lock_event(&mut self, event: &WorktreeLockEvent) -> Result<(), StoreError> {
        self.append_jsonl("locks.jsonl", event)
    }
}

fn stamp_store_event_provenance(event: &mut Event, workspace_root: &Path) {
    let observed_at = current_utc_timestamp().unwrap_or_else(|| "1970-01-01T00:00:00Z".into());
    fill_empty_string(&mut event.observed_at, observed_at.clone());
    let occurred_at = event.time.clone().unwrap_or_else(|| observed_at.clone());
    fill_empty_string(&mut event.occurred_at, occurred_at);

    let workspace = workspace_root.display().to_string();
    fill_empty_string(&mut event.workspace, workspace.clone());
    fill_empty_string(&mut event.cwd, workspace.clone());
    fill_empty_string(&mut event.worktree, workspace);

    if event.git_head.as_deref().is_none_or(str::is_empty) {
        event.git_head = current_git_head(workspace_root);
    }
    if event.git_branch.as_deref().is_none_or(str::is_empty) {
        event.git_branch = current_git_branch(workspace_root);
    }
    if event.git_dirty.is_none() {
        event.git_dirty = current_git_dirty(workspace_root);
    }

    fill_empty_string(&mut event.source_type, "monitor".into());
    fill_empty_string(&mut event.source_path, "ProjectStore::append_event".into());
    if event.source_hash.as_deref().is_none_or(str::is_empty) {
        let bytes = serde_json::to_vec(event).unwrap_or_default();
        event.source_hash = Some(fnv1a64_digest(&bytes));
    }
    fill_empty_string(&mut event.redaction_status, "clean".into());
}

fn fill_empty_string(target: &mut Option<String>, value: String) {
    if target.as_deref().is_none_or(str::is_empty) {
        *target = Some(value);
    }
}

fn immutable_control_packet_path(agent_dir: &Path, packet: &ControlPacket) -> PathBuf {
    agent_dir.join(format!("{}.md", safe_slug(&packet.packet_id)))
}

fn write_new_control_packet_file(path: &Path, rendered: &str) -> Result<(), StoreError> {
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(|source| {
            if source.kind() == std::io::ErrorKind::AlreadyExists {
                StoreError::PacketExists {
                    path: path.to_path_buf(),
                }
            } else {
                StoreError::Append {
                    path: path.to_path_buf(),
                    source,
                }
            }
        })?;
    file.write_all(rendered.as_bytes())
        .map_err(|source| StoreError::Append {
            path: path.to_path_buf(),
            source,
        })?;
    Ok(())
}

fn publish_latest_control_packet(
    agent_dir: &Path,
    packet: &ControlPacket,
    rendered: &str,
) -> Result<(), StoreError> {
    let latest_path = agent_dir.join("latest.md");
    let temp_path = agent_dir.join(format!(
        ".latest-{}-{}.tmp",
        safe_slug(&packet.packet_id),
        current_id_fragment()
    ));
    {
        let mut temp = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&temp_path)
            .map_err(|source| StoreError::Append {
                path: temp_path.clone(),
                source,
            })?;
        temp.write_all(rendered.as_bytes())
            .map_err(|source| StoreError::Append {
                path: temp_path.clone(),
                source,
            })?;
        temp.sync_all().map_err(|source| StoreError::Append {
            path: temp_path.clone(),
            source,
        })?;
    }
    replace_file(&temp_path, &latest_path).map_err(|source| StoreError::Append {
        path: latest_path,
        source,
    })?;
    Ok(())
}

#[cfg(windows)]
fn replace_file(from: &Path, to: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };

    let from = from
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let to = to
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let ok = unsafe {
        MoveFileExW(
            from.as_ptr(),
            to.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if ok == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn replace_file(from: &Path, to: &Path) -> std::io::Result<()> {
    fs::rename(from, to)
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("create directory {path}: {source}")]
    CreateDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("append {path}: {source}")]
    Append {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("timed out waiting for JSONL append lock: {path}")]
    JsonlAppendLockTimeout { path: PathBuf },
    #[error("encode {path}: {source}")]
    Encode {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("read {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("decode {path} line {line}: {source}")]
    Decode {
        path: PathBuf,
        line: usize,
        #[source]
        source: serde_json::Error,
    },
    #[error("remove {path}: {source}")]
    Remove {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("packet already exists: {path}")]
    PacketExists { path: PathBuf },
    #[error("secret-like packet content in {field}")]
    SecretLikePacket { field: String },
    #[error("advice references missing case file: {case_file_id}")]
    AdviceCaseFileMissing { case_file_id: String },
    #[error("unknown packet evidence ref: {evidence_ref}")]
    UnknownPacketEvidenceRef { evidence_ref: String },
    #[error("packet precondition failed for {field}: expected {expected}, actual {actual}")]
    PacketPrecondition {
        field: String,
        expected: String,
        actual: String,
    },
    #[error(
        "worktree {worktree} is already locked by {existing_owner}; cannot assign writable handoff to {requested_owner}"
    )]
    WorktreeLockConflict {
        worktree: String,
        existing_owner: String,
        requested_owner: String,
    },
    #[error(
        "max_parallel_writable_agents limit reached: {active}/{max}; cannot assign writable handoff to {requested_owner}"
    )]
    WorktreeCapacityExceeded {
        active: usize,
        max: usize,
        requested_owner: String,
    },
}

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
    let entropy = score_entropy(EntropyScoringInput {
        snapshot,
        intent_events: &intent_events,
        task: &task,
        verification: &verification,
        durable_memory: &durable_memory_load.memories,
        repo_audit: repo_audit.as_ref(),
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

pub fn load_blame_report(
    workspace: impl AsRef<Path>,
    query: BlameQuery,
) -> Result<BlameReport, StoreError> {
    let workspace = workspace.as_ref();
    let store_root = workspace.join(".agent-monitor");
    let traces = read_all_jsonl::<TraceEntry>(&store_root.join("trace.jsonl"))?;
    let match_workspace = blame_match_workspace(workspace);
    let target_file = normalize_blame_path(&match_workspace, &query.file);
    let mut matches = traces
        .iter()
        .cloned()
        .enumerate()
        .filter_map(|(sequence, trace)| {
            if normalize_blame_path(&match_workspace, &trace.file) != target_file {
                return None;
            }
            let match_kind = match (query.line, trace.line) {
                (Some(line), Some(_)) if trace_line_range_contains(&trace, line) => {
                    BlameMatchKind::ExactLine
                }
                (Some(_), Some(_)) => return None,
                (Some(_), None) | (None, _) => BlameMatchKind::File,
            };
            Some((sequence, BlameMatch { match_kind, trace }))
        })
        .collect::<Vec<_>>();

    matches.sort_by(|(left_sequence, left), (right_sequence, right)| {
        blame_match_rank(left.match_kind)
            .cmp(&blame_match_rank(right.match_kind))
            .then_with(|| right_sequence.cmp(left_sequence))
    });
    let has_matches = !matches.is_empty();
    let matches = matches
        .into_iter()
        .take(query.limit)
        .map(|(_, entry)| entry)
        .collect::<Vec<_>>();
    let status = if has_matches {
        BlameStatus::Traced
    } else {
        BlameStatus::Untraced
    };

    Ok(BlameReport {
        workspace: workspace.display().to_string(),
        file: query.file,
        line: query.line,
        status,
        trace_count: traces.len(),
        matches,
    })
}

pub fn load_repo_audit(workspace: impl AsRef<Path>) -> Result<RepoAuditReport, RepoAuditError> {
    let workspace = workspace.as_ref();
    let traces = read_all_jsonl::<TraceEntry>(&workspace.join(".agent-monitor/trace.jsonl"))?;
    let changes = git_changed_files(workspace)?;
    let match_workspace = blame_match_workspace(workspace);
    let mut audits = Vec::new();

    for (path, kind) in changes {
        if repo_audit_path_is_ignored(&path) {
            continue;
        }
        let mut hunks = match kind {
            RepoChangeKind::Untracked => Vec::new(),
            RepoChangeKind::Modified | RepoChangeKind::Added | RepoChangeKind::Deleted => {
                git_diff_hunks(workspace, &path)?
            }
        };
        let modified_at = repo_change_fresh_after_seconds(workspace, &path, kind);
        let all_matching_traces =
            traces_for_repo_change(&traces, &match_workspace, &path, &hunks, modified_at);
        annotate_repo_diff_hunks(&all_matching_traces, &mut hunks);
        let trace_status = trace_status_for_hunks(&all_matching_traces, &hunks);
        let matching_traces = bound_repo_audit_traces(all_matching_traces);
        audits.push(RepoChangeAudit {
            path,
            kind,
            trace_status,
            modified_at,
            hunks,
            matching_traces,
        });
    }

    audits.sort_by(|left, right| left.path.cmp(&right.path));
    let untraced_count = audits
        .iter()
        .filter(|change| change.trace_status == RepoTraceStatus::Untraced)
        .count();
    let unexplained_count = audits
        .iter()
        .filter(|change| change.trace_status == RepoTraceStatus::MissingRationale)
        .count();
    let status = if untraced_count == 0 && unexplained_count == 0 {
        RepoAuditStatus::Clean
    } else {
        RepoAuditStatus::Warning
    };

    Ok(RepoAuditReport {
        workspace: workspace.display().to_string(),
        status,
        changes: audits,
        untraced_count,
        unexplained_count,
    })
}

pub fn record_repo_audit_history(
    workspace: impl AsRef<Path>,
) -> Result<RepoAuditReport, RepoAuditError> {
    let workspace = workspace.as_ref();
    let report = load_repo_audit(workspace)?;
    let mut store = ProjectStore::open(workspace)?;
    append_repo_hunk_history_entries(&mut store, &report)?;
    Ok(report)
}

fn append_repo_hunk_history_entries(
    store: &mut ProjectStore,
    report: &RepoAuditReport,
) -> Result<(), StoreError> {
    let observed_at = current_utc_timestamp().unwrap_or_else(|| "1970-01-01T00:00:00Z".into());
    for change in &report.changes {
        for (hunk_index, hunk) in change.hunks.iter().enumerate() {
            store.append_repo_hunk_history(&RepoHunkHistoryEntry {
                history_id: format!("repo-hunk-{}", current_id_fragment()),
                observed_at: observed_at.clone(),
                workspace: report.workspace.clone(),
                path: change.path.clone(),
                kind: change.kind,
                hunk_index,
                old_start: hunk.old_start,
                old_lines: hunk.old_lines,
                new_start: hunk.new_start,
                new_lines: hunk.new_lines,
                trace_status: hunk.trace_status,
                matching_trace_count: hunk.matching_trace_count,
                change_trace_status: change.trace_status,
                modified_at: change.modified_at,
                matching_trace_refs: repo_hunk_trace_refs(change, hunk),
            })?;
        }
    }
    Ok(())
}

fn repo_hunk_trace_refs(change: &RepoChangeAudit, hunk: &RepoDiffHunk) -> Vec<RepoHunkTraceRef> {
    matching_traces_for_hunk(&change.matching_traces, hunk, change.hunks.len())
        .into_iter()
        .take(REPO_AUDIT_MAX_MATCHING_TRACES_PER_CHANGE)
        .map(repo_hunk_trace_ref)
        .collect()
}

fn matching_traces_for_hunk<'a>(
    matches: &'a [TraceEntry],
    hunk: &RepoDiffHunk,
    hunk_count: usize,
) -> Vec<&'a TraceEntry> {
    let hunk_matches = matches
        .iter()
        .filter(|trace| {
            trace.line.is_some_and(|_| trace_matches_hunk(trace, hunk))
                || (hunk_count == 1 && trace.line.is_none())
        })
        .collect::<Vec<_>>();
    if !hunk_matches.is_empty() {
        return hunk_matches;
    }
    matches
        .iter()
        .filter(|trace| trace.line.is_none())
        .collect::<Vec<_>>()
}

fn repo_hunk_trace_ref(trace: &TraceEntry) -> RepoHunkTraceRef {
    RepoHunkTraceRef {
        event_id: trace.event_id.clone(),
        agent: Some(trace.agent.clone()).filter(|agent| !agent.trim().is_empty()),
        session: trace.session.clone(),
        line: trace.line,
        line_end: trace.line_end,
        rationale: trace.rationale.clone(),
        related_event_ids: trace.related_event_ids.clone(),
    }
}

fn git_changed_files(workspace: &Path) -> Result<Vec<(String, RepoChangeKind)>, RepoAuditError> {
    let output = git_output(
        workspace,
        ["status", "--porcelain=v1", "--untracked-files=all"],
    )?;
    let mut changes = Vec::new();
    for line in output.lines() {
        if line.len() < 4 {
            continue;
        }
        let status = &line[..2];
        let path = parse_git_status_path(&line[3..]);
        if path.is_empty() {
            continue;
        }
        let kind = if status == "??" {
            RepoChangeKind::Untracked
        } else if status.contains('D') {
            RepoChangeKind::Deleted
        } else if status.contains('A') {
            RepoChangeKind::Added
        } else {
            RepoChangeKind::Modified
        };
        changes.push((path, kind));
    }
    Ok(changes)
}

fn parse_git_status_path(path: &str) -> String {
    path.rsplit_once(" -> ")
        .map(|(_, renamed_to)| renamed_to)
        .unwrap_or(path)
        .trim_matches('"')
        .replace('\\', "/")
}

fn git_diff_hunks(workspace: &Path, path: &str) -> Result<Vec<RepoDiffHunk>, RepoAuditError> {
    let output = git_output(
        workspace,
        ["diff", "--unified=0", "--no-ext-diff", "HEAD", "--", path],
    )?;
    Ok(output
        .lines()
        .filter_map(parse_git_diff_hunk)
        .collect::<Vec<_>>())
}

fn parse_git_diff_hunk(line: &str) -> Option<RepoDiffHunk> {
    let rest = line.strip_prefix("@@ -")?;
    let (old_range, rest) = rest.split_once(" +")?;
    let (new_range, _) = rest.split_once(" @@")?;
    let (old_start, old_lines) = parse_diff_range(old_range)?;
    let (new_start, new_lines) = parse_diff_range(new_range)?;
    Some(RepoDiffHunk {
        old_start,
        old_lines,
        new_start,
        new_lines,
        trace_status: RepoTraceStatus::Untraced,
        matching_trace_count: 0,
    })
}

fn parse_diff_range(range: &str) -> Option<(u32, u32)> {
    if let Some((start, count)) = range.split_once(',') {
        Some((start.parse().ok()?, count.parse().ok()?))
    } else {
        Some((range.parse().ok()?, 1))
    }
}

fn traces_for_repo_change(
    traces: &[TraceEntry],
    workspace: &Path,
    path: &str,
    hunks: &[RepoDiffHunk],
    fresh_after: Option<i64>,
) -> Vec<TraceEntry> {
    let target = normalize_blame_path(workspace, path);
    traces
        .iter()
        .filter(|trace| normalize_blame_path(workspace, &trace.file) == target)
        .filter(|trace| trace_is_fresh_for_repo_audit(trace, fresh_after))
        .filter(|trace| trace_matches_hunks(trace, hunks))
        .cloned()
        .collect()
}

fn repo_change_fresh_after_seconds(
    workspace: &Path,
    path: &str,
    kind: RepoChangeKind,
) -> Option<i64> {
    if kind == RepoChangeKind::Deleted {
        return deleted_path_parent_modified_seconds(workspace, path)
            .or_else(|| git_head_commit_seconds(workspace));
    }
    fs::metadata(workspace.join(path))
        .ok()
        .and_then(|metadata| metadata.modified().ok())
        .and_then(system_time_seconds)
        .or_else(|| git_head_commit_seconds(workspace))
}

fn deleted_path_parent_modified_seconds(workspace: &Path, path: &str) -> Option<i64> {
    let mut current = workspace.join(path);
    while let Some(parent) = current.parent() {
        if let Ok(metadata) = fs::metadata(parent) {
            return metadata.modified().ok().and_then(system_time_seconds);
        }
        current = parent.to_path_buf();
    }
    None
}

fn git_head_commit_seconds(workspace: &Path) -> Option<i64> {
    git_output(workspace, ["log", "-1", "--format=%ct", "HEAD"])
        .ok()?
        .trim()
        .parse()
        .ok()
}

fn system_time_seconds(time: SystemTime) -> Option<i64> {
    let duration = time.duration_since(UNIX_EPOCH).ok()?;
    i64::try_from(duration.as_secs()).ok()
}

fn trace_is_fresh_for_repo_audit(trace: &TraceEntry, fresh_after: Option<i64>) -> bool {
    let Some(fresh_after) = fresh_after else {
        return true;
    };
    trace
        .time
        .as_deref()
        .and_then(parse_utc_seconds)
        .is_some_and(|trace_time| trace_time >= fresh_after)
}

fn bound_repo_audit_traces(traces: Vec<TraceEntry>) -> Vec<TraceEntry> {
    traces
        .into_iter()
        .rev()
        .take(REPO_AUDIT_MAX_MATCHING_TRACES_PER_CHANGE)
        .collect()
}

fn trace_matches_hunks(trace: &TraceEntry, hunks: &[RepoDiffHunk]) -> bool {
    if hunks.is_empty() || trace.line.is_none() {
        return true;
    }
    hunks.iter().any(|hunk| trace_matches_hunk(trace, hunk))
}

fn trace_matches_hunk(trace: &TraceEntry, hunk: &RepoDiffHunk) -> bool {
    trace_overlaps_diff_range(trace, hunk.new_start, hunk.new_lines)
        || trace_overlaps_diff_range(trace, hunk.old_start, hunk.old_lines)
}

fn trace_line_range_contains(trace: &TraceEntry, line: u32) -> bool {
    let Some(start) = trace.line else {
        return false;
    };
    let end = trace.line_end.unwrap_or(start).max(start);
    line >= start && line <= end
}

fn trace_overlaps_diff_range(trace: &TraceEntry, start: u32, lines: u32) -> bool {
    let Some(trace_start) = trace.line else {
        return true;
    };
    if lines == 0 {
        return false;
    }
    let trace_end = trace.line_end.unwrap_or(trace_start).max(trace_start);
    let range_end = start.saturating_add(lines - 1);
    trace_start <= range_end && trace_end >= start
}

fn trace_status_for_hunks(matches: &[TraceEntry], hunks: &[RepoDiffHunk]) -> RepoTraceStatus {
    if hunks.is_empty() {
        return trace_status_for_matches(matches);
    }

    let mut has_missing_rationale = false;
    for hunk in hunks {
        match trace_status_for_hunk(matches, hunk, hunks.len()).0 {
            RepoTraceStatus::Traced => {}
            RepoTraceStatus::MissingRationale => has_missing_rationale = true,
            RepoTraceStatus::Untraced => return RepoTraceStatus::Untraced,
        }
    }

    if has_missing_rationale {
        RepoTraceStatus::MissingRationale
    } else {
        RepoTraceStatus::Traced
    }
}

fn annotate_repo_diff_hunks(matches: &[TraceEntry], hunks: &mut [RepoDiffHunk]) {
    let hunk_count = hunks.len();
    for hunk in hunks {
        let (trace_status, matching_trace_count) = trace_status_for_hunk(matches, hunk, hunk_count);
        hunk.trace_status = trace_status;
        hunk.matching_trace_count = matching_trace_count;
    }
}

fn trace_status_for_hunk(
    matches: &[TraceEntry],
    hunk: &RepoDiffHunk,
    hunk_count: usize,
) -> (RepoTraceStatus, usize) {
    let hunk_matches = matches
        .iter()
        .filter(|trace| {
            trace.line.is_some_and(|_| trace_matches_hunk(trace, hunk))
                || (hunk_count == 1 && trace.line.is_none())
        })
        .collect::<Vec<_>>();
    if !hunk_matches.is_empty() {
        let status = if hunk_matches.iter().any(|trace| trace_has_rationale(trace)) {
            RepoTraceStatus::Traced
        } else {
            RepoTraceStatus::MissingRationale
        };
        return (status, hunk_matches.len());
    }

    let file_level_trace_count = matches.iter().filter(|trace| trace.line.is_none()).count();
    if file_level_trace_count > 0 {
        (RepoTraceStatus::MissingRationale, file_level_trace_count)
    } else {
        (RepoTraceStatus::Untraced, 0)
    }
}

fn trace_status_for_matches(matches: &[TraceEntry]) -> RepoTraceStatus {
    if matches.is_empty() {
        RepoTraceStatus::Untraced
    } else if matches.iter().any(trace_has_rationale) {
        RepoTraceStatus::Traced
    } else {
        RepoTraceStatus::MissingRationale
    }
}

fn trace_has_rationale(trace: &TraceEntry) -> bool {
    trace
        .rationale
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
}

fn repo_audit_path_is_ignored(path: &str) -> bool {
    let normalized = normalize_path_text(path);
    normalized.starts_with(".agent-monitor/")
        || normalized == ".agent-monitor"
        || normalized.starts_with("target/")
}

fn git_output<const N: usize>(workspace: &Path, args: [&str; N]) -> Result<String, RepoAuditError> {
    let args_display = args.join(" ");
    let output = Command::new("git")
        .current_dir(workspace)
        .args(args)
        .output()
        .map_err(|source| RepoAuditError::GitSpawn {
            args: args_display.clone(),
            source,
        })?;
    if !output.status.success() {
        return Err(RepoAuditError::GitFailed {
            args: args_display,
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }
    String::from_utf8(output.stdout).map_err(|source| RepoAuditError::GitUtf8 {
        args: args_display,
        source,
    })
}

fn blame_match_rank(kind: BlameMatchKind) -> u8 {
    match kind {
        BlameMatchKind::ExactLine => 0,
        BlameMatchKind::File => 1,
    }
}

fn evidence_from_snapshot(snapshot: &DashboardSnapshot) -> Vec<EvidenceItem> {
    let mut evidence = Vec::new();
    for (index, event) in snapshot.recent_events.iter().enumerate() {
        let (summary, redaction_status) =
            sanitize_evidence_summary(&truncate_evidence(&event_summary(event)));
        let redaction_status =
            strongest_redaction_status(redaction_status, event_redaction_status(event));
        evidence.push(EvidenceItem {
            id: event
                .event_id
                .clone()
                .unwrap_or_else(|| format!("event-{}", index + 1)),
            kind: format!("{:?}", event.kind),
            agent: Some(event.agent.clone()),
            session: event.session.clone(),
            run_id: event.run_id.clone(),
            agent_session_id: event.agent_session_id.clone(),
            summary,
            redaction_status,
            source: event.file.clone().or_else(|| event.command.clone()),
            source_type: event.source_type.clone(),
            source_path: event.source_path.clone(),
            source_offset: event.source_offset,
            source_hash: event.source_hash.clone(),
            redaction_rules: event.redaction_rules.clone(),
        });
    }
    for (index, intervention) in snapshot.recent_interventions.iter().enumerate() {
        let (summary, redaction_status) = sanitize_evidence_summary(&intervention.reason);
        evidence.push(EvidenceItem {
            id: format!("intervention-{}", index + 1),
            kind: format!("{:?}", intervention.kind),
            agent: intervention.agent.clone(),
            session: None,
            run_id: None,
            agent_session_id: None,
            summary: truncate_evidence(&summary),
            redaction_status,
            source: None,
            source_type: None,
            source_path: None,
            source_offset: None,
            source_hash: None,
            redaction_rules: Vec::new(),
        });
    }
    for run in &snapshot.recent_verifier_runs {
        let (summary, redaction_status) =
            sanitize_evidence_summary(&truncate_evidence(&verifier_run_summary(run)));
        evidence.push(EvidenceItem {
            id: run.verifier_run_id.clone(),
            kind: "VerifierRun".into(),
            agent: None,
            session: None,
            run_id: None,
            agent_session_id: None,
            summary,
            redaction_status,
            source: Some(run.command.clone()),
            source_type: Some("verifier".into()),
            source_path: None,
            source_offset: None,
            source_hash: None,
            redaction_rules: Vec::new(),
        });
    }
    for run in &snapshot.recent_probe_runs {
        let (summary, redaction_status) =
            sanitize_evidence_summary(&truncate_evidence(&probe_run_summary(run)));
        evidence.push(EvidenceItem {
            id: run.probe_run_id.clone(),
            kind: "ProbeRun".into(),
            agent: None,
            session: None,
            run_id: None,
            agent_session_id: None,
            summary,
            redaction_status,
            source: Some(run.advice_id.clone()),
            source_type: Some("probe".into()),
            source_path: Some("probe-runs.jsonl".into()),
            source_offset: None,
            source_hash: None,
            redaction_rules: Vec::new(),
        });
    }
    for report in &snapshot.recent_dev_history {
        for (finding_index, finding) in report.findings.iter().enumerate() {
            let (summary, redaction_status) = sanitize_evidence_summary(&truncate_evidence(
                &dev_history_finding_evidence_summary(report, finding),
            ));
            evidence.push(EvidenceItem {
                id: dev_history_finding_evidence_id(report, finding_index, finding),
                kind: "DevHistoryFinding".into(),
                agent: None,
                session: None,
                run_id: None,
                agent_session_id: None,
                summary,
                redaction_status,
                source: Some(report.workspace.clone()),
                source_type: Some("dev_history".into()),
                source_path: Some("dev-history.jsonl".into()),
                source_offset: None,
                source_hash: None,
                redaction_rules: Vec::new(),
            });
        }
    }
    evidence
}

fn evidence_from_project_contract_requirements(
    requirements: &[ProjectContractRequirement],
) -> Vec<EvidenceItem> {
    requirements
        .iter()
        .map(|requirement| {
            let raw_summary = format!(
                "Project contract requirement from {}:{}: {}",
                requirement.source_path, requirement.line, requirement.text
            );
            let (summary, redaction_status) =
                sanitize_evidence_summary(&truncate_evidence(&raw_summary));
            EvidenceItem {
                id: requirement.evidence_id.clone(),
                kind: "ProjectContract".into(),
                agent: None,
                session: None,
                run_id: None,
                agent_session_id: None,
                summary,
                redaction_status,
                source: Some(format!("{}:{}", requirement.source_path, requirement.line)),
                source_type: Some("project_contract".into()),
                source_path: Some(requirement.source_path.clone()),
                source_offset: Some(requirement.line),
                source_hash: Some(requirement.source_hash.clone()),
                redaction_rules: Vec::new(),
            }
        })
        .collect()
}

fn dev_history_finding_evidence_id(
    report: &DevHistoryReport,
    finding_index: usize,
    finding: &DevHistoryFinding,
) -> String {
    let mut seed = String::new();
    push_dev_history_id_field(&mut seed, &report.workspace);
    push_dev_history_id_field(&mut seed, &report.generated_at);
    push_dev_history_id_field(&mut seed, &finding_index.to_string());
    for source in &report.sources {
        push_dev_history_id_field(&mut seed, &source.source);
        push_dev_history_id_field(&mut seed, &source.history_root);
        push_dev_history_id_field(&mut seed, &source.files.to_string());
        push_dev_history_id_field(&mut seed, &source.bytes.to_string());
        push_dev_history_id_field(&mut seed, &source.lines.to_string());
        push_dev_history_id_field(&mut seed, &source.parsed.to_string());
        push_dev_history_id_field(&mut seed, &source.sessions.to_string());
    }
    push_dev_history_id_field(&mut seed, &finding.kind);
    push_dev_history_id_field(&mut seed, &finding.severity);
    push_dev_history_id_field(&mut seed, &finding.summary);
    for evidence in &finding.evidence {
        push_dev_history_id_field(&mut seed, evidence);
    }
    for response in &finding.monitor_response {
        push_dev_history_id_field(&mut seed, response);
    }
    let digest = fnv1a64_digest(seed.as_bytes())
        .strip_prefix("fnv1a64:")
        .unwrap_or("unknown")
        .to_string();
    format!("dev-history-{}-{digest}", safe_slug(&finding.kind))
}

fn push_dev_history_id_field(seed: &mut String, value: &str) {
    seed.push_str(&value.len().to_string());
    seed.push(':');
    seed.push_str(value);
    seed.push('\n');
}

fn dev_history_finding_evidence_summary(
    report: &DevHistoryReport,
    finding: &DevHistoryFinding,
) -> String {
    let evidence = if finding.evidence.is_empty() {
        "no aggregate evidence details".into()
    } else {
        finding.evidence.join("; ")
    };
    let sources = report
        .sources
        .iter()
        .map(|source| format!("{}:{} file(s)", source.source, source.files))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "{} {} finding: {} Evidence: {}. Sources: {}.",
        finding.severity, finding.kind, finding.summary, evidence, sources
    )
}

fn evidence_from_repo_audit(report: &RepoAuditReport) -> Vec<EvidenceItem> {
    report
        .changes
        .iter()
        .map(|change| {
            let status = match change.trace_status {
                RepoTraceStatus::Traced => "traced",
                RepoTraceStatus::MissingRationale => "missing rationale",
                RepoTraceStatus::Untraced => "untraced",
            };
            let summary = format!("{} has {status} dirty git hunks", change.path);
            let (summary, redaction_status) = sanitize_evidence_summary(&summary);
            EvidenceItem {
                id: format!("repo-audit-{}", safe_slug(&change.path)),
                kind: "repo_audit".into(),
                agent: None,
                session: None,
                run_id: None,
                agent_session_id: None,
                summary: truncate_evidence(&summary),
                redaction_status,
                source: Some(change.path.clone()),
                source_type: Some("git".into()),
                source_path: Some(change.path.clone()),
                source_offset: None,
                source_hash: None,
                redaction_rules: Vec::new(),
            }
        })
        .collect()
}

fn memory_candidates_from_snapshot(snapshot: &DashboardSnapshot) -> Vec<MemoryCandidate> {
    let mut candidates = Vec::new();
    for (index, event) in snapshot.recent_events.iter().enumerate() {
        if event.redaction_status.as_deref() == Some("tainted") {
            continue;
        }
        let Some((claim, source, confidence)) = memory_candidate_claim_from_event(event) else {
            continue;
        };
        let evidence_id = event
            .event_id
            .clone()
            .unwrap_or_else(|| format!("event-{}", index + 1));
        candidates.push(MemoryCandidate {
            memory_id: format!("mem-{}", safe_slug(&evidence_id)),
            scope: MemoryScope::Project,
            claim,
            status: MemoryStatus::Unverified,
            source,
            evidence_ids: vec![evidence_id],
            confidence,
        });
    }
    candidates
}

fn memory_candidate_claim_from_event(event: &Event) -> Option<(String, MemorySource, u8)> {
    let content = event.content.as_ref()?.trim();
    if content.is_empty() {
        return None;
    }
    match event.kind {
        EventKind::DesignThought => Some((content.to_string(), MemorySource::AgentClaim, 50)),
        EventKind::UserInstruction => {
            durable_user_instruction_claim(content).map(|claim| (claim, MemorySource::User, 80))
        }
        _ => None,
    }
}

fn durable_user_instruction_claim(content: &str) -> Option<String> {
    content
        .lines()
        .map(strip_user_memory_line_prefix)
        .find(|line| durable_user_memory_line(line))
        .map(clean_user_memory_claim)
        .filter(|claim| !claim.is_empty())
}

fn strip_user_memory_line_prefix(line: &str) -> &str {
    line.trim_start_matches([' ', '\t', '-', '*', '#'])
        .trim_start()
}

fn durable_user_memory_line(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    [
        "remember:",
        "remember this:",
        "keep in mind:",
        "project constraint:",
        "constraint:",
        "preference:",
        "prefer ",
        "do not ",
        "never ",
    ]
    .iter()
    .any(|marker| lower.starts_with(marker))
}

fn clean_user_memory_claim(line: &str) -> String {
    for prefix in [
        "remember this:",
        "remember:",
        "keep in mind:",
        "project constraint:",
        "constraint:",
        "preference:",
    ] {
        if line
            .get(..prefix.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
        {
            return line[prefix.len()..].trim().to_string();
        }
    }
    line.trim().to_string()
}

#[derive(Debug, Default)]
struct DurableMemoryLoad {
    memories: Vec<MemoryCandidate>,
    warnings: Vec<EvidenceItem>,
}

fn load_durable_memory(workspace: &Path) -> DurableMemoryLoad {
    let path = workspace.join(".agent-monitor/memories.jsonl");
    let (records, warnings) = read_durable_memory_records(&path);
    let mut warnings = warnings;
    let mut latest_by_id = HashMap::<String, (usize, MemoryCandidate)>::new();
    for (sequence, memory) in records {
        latest_by_id.insert(memory.memory_id.clone(), (sequence, memory));
    }
    let mut latest = latest_by_id.into_values().collect::<Vec<_>>();
    latest.sort_by(|(left, _), (right, _)| right.cmp(left));
    let conflicts = durable_memory_conflicts(&latest);
    let conflicted_ids = conflicts
        .iter()
        .flat_map(|conflict| {
            [
                conflict.left_memory_id.clone(),
                conflict.right_memory_id.clone(),
            ]
        })
        .collect::<HashSet<_>>();
    warnings.extend(
        conflicts
            .iter()
            .map(|conflict| durable_memory_conflict_evidence(&path, conflict)),
    );
    let memories = latest
        .into_iter()
        .map(|(_, memory)| memory)
        .filter(|memory| !conflicted_ids.contains(&memory.memory_id))
        .filter(memory_is_durable_active)
        .take(20)
        .collect();
    DurableMemoryLoad { memories, warnings }
}

fn latest_active_durable_memory_records(path: &Path) -> Vec<(usize, MemoryCandidate)> {
    let (records, _) = read_durable_memory_records(path);
    let mut latest_by_id = HashMap::<String, (usize, MemoryCandidate)>::new();
    for (sequence, memory) in records {
        latest_by_id.insert(memory.memory_id.clone(), (sequence, memory));
    }
    let mut latest = latest_by_id.into_values().collect::<Vec<_>>();
    latest.sort_by(|(left, _), (right, _)| right.cmp(left));
    latest
        .into_iter()
        .filter(|(_, memory)| memory_is_durable_active(memory))
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DurableMemoryConflict {
    left_memory_id: String,
    right_memory_id: String,
    subject: String,
}

fn durable_memory_conflicts(records: &[(usize, MemoryCandidate)]) -> Vec<DurableMemoryConflict> {
    let active = records
        .iter()
        .filter(|(_, memory)| memory_is_durable_active(memory))
        .map(|(_, memory)| memory)
        .collect::<Vec<_>>();
    let mut conflicts = Vec::new();
    for (left_index, left) in active.iter().enumerate() {
        for right in active.iter().skip(left_index + 1) {
            if let Some(subject) = memory_conflict_subject(left, right) {
                conflicts.push(DurableMemoryConflict {
                    left_memory_id: left.memory_id.clone(),
                    right_memory_id: right.memory_id.clone(),
                    subject,
                });
            }
        }
    }
    conflicts
}

fn memory_claims_conflict(left: &MemoryCandidate, right: &MemoryCandidate) -> bool {
    memory_conflict_subject(left, right).is_some()
}

fn memory_conflict_subject(left: &MemoryCandidate, right: &MemoryCandidate) -> Option<String> {
    if left.memory_id == right.memory_id {
        return None;
    }
    let left = memory_claim_polarity_subject(&left.claim)?;
    let right = memory_claim_polarity_subject(&right.claim)?;
    if left.subject == right.subject && left.deny != right.deny {
        Some(left.subject)
    } else {
        None
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MemoryClaimPolarity {
    deny: bool,
    subject: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RejectedAlternative {
    subject: String,
    evidence_id: String,
}

fn rejected_alternatives_from_intent_and_memory(
    intent_events: &[Event],
    durable_memory: &[MemoryCandidate],
) -> Vec<RejectedAlternative> {
    let mut rejected = Vec::new();
    for (index, event) in intent_events.iter().enumerate() {
        let Some(content) = event.content.as_deref() else {
            continue;
        };
        let evidence_id = event_evidence_id(event, index);
        for subject in rejected_alternative_subjects_from_text(content) {
            push_rejected_alternative(&mut rejected, subject, evidence_id.clone());
        }
    }
    for memory in durable_memory {
        let evidence_id = memory
            .evidence_ids
            .first()
            .cloned()
            .unwrap_or_else(|| memory.memory_id.clone());
        for subject in rejected_alternative_subjects_from_text(&memory.claim) {
            push_rejected_alternative(&mut rejected, subject, evidence_id.clone());
        }
    }
    rejected
}

fn push_rejected_alternative(
    rejected: &mut Vec<RejectedAlternative>,
    subject: String,
    evidence_id: String,
) {
    if subject.is_empty() || rejected.iter().any(|existing| existing.subject == subject) {
        return;
    }
    rejected.push(RejectedAlternative {
        subject,
        evidence_id,
    });
}

fn rejected_alternative_subjects_from_text(text: &str) -> Vec<String> {
    let mut subjects = Vec::new();
    for line in text.lines().map(strip_user_memory_line_prefix) {
        let normalized = normalize_memory_claim_for_conflict(line);
        if let Some(subject) = normalized
            .strip_prefix("rejected alternative ")
            .or_else(|| normalized.strip_prefix("rejected approach "))
            .or_else(|| normalized.strip_prefix("rejected design "))
        {
            push_unique_string(&mut subjects, &rejected_alternative_subject_key(subject));
            continue;
        }
        if let Some(polarity) = memory_claim_polarity_subject(line)
            && polarity.deny
        {
            push_unique_string(
                &mut subjects,
                &rejected_alternative_subject_key(&polarity.subject),
            );
        }
    }
    subjects
}

fn rejected_alternative_subject_key(subject: &str) -> String {
    let mut tokens = normalize_memory_claim_for_conflict(subject)
        .split_whitespace()
        .map(str::to_string)
        .collect::<Vec<_>>();
    while tokens
        .first()
        .is_some_and(|token| rejected_alternative_leading_token(token))
    {
        tokens.remove(0);
    }
    tokens.join(" ")
}

fn rejected_alternative_leading_token(token: &str) -> bool {
    matches!(
        token,
        "create"
            | "add"
            | "introduce"
            | "use"
            | "preserve"
            | "keep"
            | "build"
            | "implement"
            | "make"
            | "a"
            | "an"
            | "the"
    )
}

fn event_reintroduces_rejected_alternative<'a>(
    event: &Event,
    evidence_id: &str,
    content: &str,
    rejected_alternatives: &'a [RejectedAlternative],
) -> Option<&'a RejectedAlternative> {
    if rejected_alternatives.is_empty() || event.kind == EventKind::UserInstruction {
        return None;
    }
    let text = normalized_event_text(event, content);
    if text.is_empty() {
        return None;
    }
    rejected_alternatives
        .iter()
        .filter(|rejected| rejected.evidence_id != evidence_id)
        .find(|rejected| {
            event_text_reintroduces_rejected_subject(&text, &rejected.subject)
                && !event_text_reaffirms_rejected_subject(&text, &rejected.subject)
        })
}

fn event_text_reintroduces_rejected_subject(text: &str, subject: &str) -> bool {
    !subject.is_empty()
        && (text.contains(subject)
            || subject
                .split_whitespace()
                .filter(|token| token.len() > 2)
                .all(|token| text.contains(token)))
}

fn event_text_reaffirms_rejected_subject(text: &str, subject: &str) -> bool {
    [
        "do not",
        "never",
        "must not",
        "should not",
        "avoid",
        "reject",
        "rejected",
    ]
    .iter()
    .any(|prefix| text.contains(&format!("{prefix} {subject}")))
}

fn normalized_event_text(event: &Event, content: &str) -> String {
    let mut text = String::new();
    if let Some(command) = event.command.as_deref() {
        text.push_str(command);
        text.push(' ');
    }
    text.push_str(content);
    text.push(' ');
    if let Some(rationale) = event.rationale.as_deref() {
        text.push_str(rationale);
    }
    normalize_memory_claim_for_conflict(&text)
}

fn push_unique_string(values: &mut Vec<String>, value: &str) {
    if !value.is_empty() && !values.iter().any(|existing| existing == value) {
        values.push(value.to_string());
    }
}

fn memory_claim_polarity_subject(claim: &str) -> Option<MemoryClaimPolarity> {
    let normalized = normalize_memory_claim_for_conflict(claim);
    if normalized.is_empty() {
        return None;
    }
    for prefix in [
        "do not ",
        "never ",
        "must not ",
        "should not ",
        "dont ",
        "don t ",
    ] {
        if let Some(subject) = normalized.strip_prefix(prefix) {
            let subject = subject.trim().to_string();
            if !subject.is_empty() {
                return Some(MemoryClaimPolarity {
                    deny: true,
                    subject,
                });
            }
        }
    }
    Some(MemoryClaimPolarity {
        deny: false,
        subject: normalized,
    })
}

fn normalize_memory_claim_for_conflict(claim: &str) -> String {
    claim
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn durable_memory_conflict_evidence(path: &Path, conflict: &DurableMemoryConflict) -> EvidenceItem {
    let issue = format!(
        "Durable memory conflict between {} and {} on subject '{}'; both memories quarantined",
        conflict.left_memory_id, conflict.right_memory_id, conflict.subject
    );
    let (summary, redaction_status) = sanitize_evidence_summary(&issue);
    EvidenceItem {
        id: format!(
            "memory-conflict-{}-{}",
            safe_slug(&conflict.left_memory_id),
            safe_slug(&conflict.right_memory_id)
        ),
        kind: "memory_conflict".into(),
        agent: None,
        session: None,
        run_id: None,
        agent_session_id: None,
        summary: truncate_evidence(&summary),
        redaction_status,
        source: Some(path.display().to_string()),
        source_type: Some("memory".into()),
        source_path: Some(path.display().to_string()),
        source_offset: None,
        source_hash: None,
        redaction_rules: Vec::new(),
    }
}

fn read_durable_memory_records(path: &Path) -> (Vec<(usize, MemoryCandidate)>, Vec<EvidenceItem>) {
    if !path.exists() {
        return (Vec::new(), Vec::new());
    }

    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(source) => {
            return (
                Vec::new(),
                vec![durable_memory_load_warning(
                    path,
                    None,
                    &format!("could not read durable memory log: {source}"),
                )],
            );
        }
    };

    let reader = BufReader::new(file);
    let mut lines = Vec::new();
    let mut warnings = Vec::new();
    for (index, line) in reader.lines().enumerate() {
        match line {
            Ok(line) if !line.trim().is_empty() => lines.push((index + 1, line)),
            Ok(_) => {}
            Err(source) => warnings.push(durable_memory_load_warning(
                path,
                Some(index + 1),
                &format!("could not read durable memory log line: {source}"),
            )),
        }
    }

    let last_line_number = lines.last().map(|(line_number, _)| *line_number);
    let mut records = Vec::new();
    for (line_number, line) in lines {
        match serde_json::from_str::<MemoryCandidate>(&line) {
            Ok(memory) => records.push((line_number, memory)),
            Err(source) if Some(line_number) == last_line_number && source.is_eof() => {
                warnings.push(durable_memory_load_warning(
                    path,
                    Some(line_number),
                    "trailing partial durable memory record ignored",
                ));
                break;
            }
            Err(source) => warnings.push(durable_memory_load_warning(
                path,
                Some(line_number),
                &format!("malformed durable memory record skipped: {source}"),
            )),
        }
    }

    (records, warnings)
}

fn durable_memory_load_warning(
    path: &Path,
    line_number: Option<usize>,
    issue: &str,
) -> EvidenceItem {
    let id = line_number
        .map(|line| format!("memory-load-warning-line-{line}"))
        .unwrap_or_else(|| "memory-load-warning-read".into());
    let location = line_number
        .map(|line| format!("line {line}"))
        .unwrap_or_else(|| "file".into());
    let (summary, redaction_status) =
        sanitize_evidence_summary(&format!("Durable memory log {location}: {issue}"));
    EvidenceItem {
        id,
        kind: "memory_load_warning".into(),
        agent: None,
        session: None,
        run_id: None,
        agent_session_id: None,
        summary: truncate_evidence(&summary),
        redaction_status,
        source: Some(path.display().to_string()),
        source_type: Some("memory".into()),
        source_path: Some(path.display().to_string()),
        source_offset: line_number.map(|line| line as u64),
        source_hash: None,
        redaction_rules: Vec::new(),
    }
}

fn memory_is_durable_active(memory: &MemoryCandidate) -> bool {
    memory.status == MemoryStatus::Active
        && matches!(
            memory.source,
            MemorySource::User | MemorySource::VerifiedResult | MemorySource::ManualReview
        )
        && !packet_text_is_tainted(&memory.claim)
}

struct EntropyScoringInput<'a> {
    snapshot: &'a DashboardSnapshot,
    intent_events: &'a [Event],
    task: &'a TaskSummary,
    verification: &'a VerificationSummary,
    durable_memory: &'a [MemoryCandidate],
    repo_audit: Option<&'a RepoAuditReport>,
    policy: &'a PolicyConfig,
    security: &'a SecurityConfig,
}

fn score_entropy(input: EntropyScoringInput<'_>) -> EntropyVector {
    let EntropyScoringInput {
        snapshot,
        intent_events,
        task,
        verification,
        durable_memory,
        repo_audit,
        policy,
        security,
    } = input;
    let mut vector = EntropyVector::baseline();

    if task.user_goal.is_none() && task.acceptance_criteria.is_empty() {
        vector.raise(
            EntropyKind::Goal,
            65,
            75,
            "no current user goal captured",
            None,
            Some("current user goal or acceptance criteria".into()),
        );
    }

    for marker in &task.ambiguity_markers {
        vector.raise(
            EntropyKind::Goal,
            82,
            80,
            "user goal contains unresolved ambiguity",
            marker.source_event_id.clone(),
            Some("clarified acceptance criteria or bounded user decision".into()),
        );
    }

    let mut latest_source_write: Option<((i64, usize), String)> = None;
    let mut latest_passing_verifier: Option<(i64, usize)> = None;
    let mut latest_failed_verifier: Option<((i64, usize), String, String, String)> = None;
    let mut failing_commands = HashMap::<(String, String, FailureLayer), (usize, String)>::new();
    let mut service_failures = HashMap::<(String, FailureLayer), (usize, String)>::new();
    let mut permission_denials = HashMap::<String, (usize, String)>::new();
    let mut permission_requests = HashMap::<String, (usize, String)>::new();
    let mut inspection_loops = HashMap::<(String, String), (usize, String)>::new();
    let mut unresolved_subagents = HashMap::<String, (usize, String)>::new();
    let mut unresolved_subagent_paths = HashMap::<(String, String), (usize, String)>::new();
    let mut verifier_failure_loops = HashMap::<(String, String), VerifierFailureLoop>::new();
    let mut latest_domain_validation_write: Option<(
        (i64, usize),
        String,
        String,
        ValidationSurface,
    )> = None;
    let mut latest_domain_validation_pass = HashMap::<ValidationSurface, (i64, usize)>::new();
    let mut latest_completion_claim: Option<((i64, usize), String, String)> = None;
    let bug_fix_goal = task_suggests_bug_fix(task);
    let mut bug_fix_probe_seen = false;
    let mut bug_fix_pre_edit_gap_recorded = false;
    let rejected_alternatives =
        rejected_alternatives_from_intent_and_memory(intent_events, durable_memory);

    for (index, event) in snapshot.recent_events.iter().enumerate() {
        let evidence_id = event
            .event_id
            .clone()
            .unwrap_or_else(|| format!("event-{}", index + 1));
        let time = event.time.as_deref().and_then(parse_utc_seconds);
        let content = event.content.as_deref().unwrap_or_default();

        if event_is_change_like(event) {
            if bug_fix_goal
                && !bug_fix_probe_seen
                && !bug_fix_pre_edit_gap_recorded
                && event
                    .file
                    .as_deref()
                    .is_some_and(|file| is_verification_relevant_file(file, policy))
            {
                vector.raise(
                    EntropyKind::Plan,
                    72,
                    82,
                    "bug-fix edit happened before reproduction or localization evidence",
                    Some(evidence_id.clone()),
                    Some(
                        "reproduction, failing verifier, or localization probe before more edits"
                            .into(),
                    ),
                );
                bug_fix_pre_edit_gap_recorded = true;
            }
            for ((agent, _), loop_state) in verifier_failure_loops.iter_mut() {
                if agent == &event.agent {
                    loop_state.edits_since_last_failure += 1;
                }
            }
            if let Some(file) = event.file.as_deref()
                && test_oracle_change_lacks_authority(event, file)
            {
                vector.raise(
                    EntropyKind::Verification,
                    83,
                    85,
                    format!(
                        "test oracle change `{file}` lacks authority and independent behavior evidence"
                    ),
                    Some(evidence_id.clone()),
                    Some(
                        "spec authority plus independent behavior evidence for the test oracle change"
                            .into(),
                    ),
                );
            }
            if let Some(cause) = event
                .file
                .as_deref()
                .and_then(|file| security_path_user_decision_cause(file, security))
            {
                vector.raise(
                    EntropyKind::UserDecision,
                    88,
                    90,
                    cause,
                    Some(evidence_id.clone()),
                    Some("user authorization or required external input".into()),
                );
            }
            if event
                .file
                .as_deref()
                .is_some_and(|file| is_verification_relevant_file(file, policy))
            {
                let order = (time.unwrap_or(i64::MAX), index);
                if latest_source_write
                    .as_ref()
                    .is_none_or(|(current, _)| order > *current)
                {
                    latest_source_write = Some((order, evidence_id.clone()));
                }
                if policy.require_verification_after_source_change
                    && let Some(file) = event.file.as_deref()
                    && let Some(surface) = validation_surface_for_path(file)
                    && latest_domain_validation_write
                        .as_ref()
                        .is_none_or(|(current, _, _, _)| order > *current)
                {
                    latest_domain_validation_write =
                        Some((order, evidence_id.clone(), file.to_string(), surface));
                }
            }
            if event
                .rationale
                .as_deref()
                .is_none_or(|rationale| rationale.trim().is_empty())
            {
                vector.raise(
                    EntropyKind::RepoBlame,
                    75,
                    85,
                    "file change lacks rationale",
                    Some(evidence_id.clone()),
                    Some("rationale linked to file change".into()),
                );
            }
        }

        if event_records_failure_hypothesis(event, content) {
            for ((agent, _), loop_state) in verifier_failure_loops.iter_mut() {
                if agent == &event.agent {
                    loop_state.hypothesis_since_last_failure = true;
                }
            }
        }

        if matches!(event.kind, EventKind::CommandResult | EventKind::TestResult)
            && event.exit_code == Some(0)
        {
            let order = (time.unwrap_or(i64::MAX), index);
            for surface in validation_surfaces_for_event(event) {
                let entry = latest_domain_validation_pass
                    .entry(surface)
                    .or_insert(order);
                if order > *entry {
                    *entry = order;
                }
            }
        }

        if event_is_verification_result(event) {
            match event.exit_code {
                Some(0) => {
                    verifier_failure_loops.retain(|(agent, _), _| agent != &event.agent);
                    if let Some(time) = time {
                        let order = (time, index);
                        latest_passing_verifier = Some(
                            latest_passing_verifier
                                .map(|current| current.max(order))
                                .unwrap_or(order),
                        );
                    }
                }
                Some(_) => {
                    if let Some(signature) = verifier_failure_signature(event) {
                        let entry = verifier_failure_loops
                            .entry((event.agent.clone(), signature))
                            .or_default();
                        if !entry.evidence_id.is_empty()
                            && entry.edits_since_last_failure > 0
                            && !entry.hypothesis_since_last_failure
                        {
                            entry.repeated_after_edits += 1;
                        }
                        entry.command = event.command.clone().unwrap_or_default();
                        entry.evidence_id = evidence_id.clone();
                        entry.edits_since_last_failure = 0;
                        entry.hypothesis_since_last_failure = false;
                    }
                    let failure_order = (time.unwrap_or(i64::MAX), index);
                    if latest_failed_verifier
                        .as_ref()
                        .is_none_or(|(current, _, _, _)| failure_order > *current)
                    {
                        latest_failed_verifier = Some((
                            failure_order,
                            evidence_id.clone(),
                            "verification command failed".into(),
                            "passing verification result".into(),
                        ));
                    }
                }
                None => {}
            }
        }

        if event.kind == EventKind::CommandResult && event.exit_code == Some(0) {
            failing_commands.retain(|(agent, _, _), _| agent != &event.agent);
        }

        if event.kind == EventKind::CommandResult
            && let Some(command) = event.command.as_deref().map(normalize_command_signature)
            && !is_verification_command(&command)
        {
            let key = (event.agent.clone(), command);
            match event.exit_code {
                Some(0) => {}
                Some(_) => {
                    let layer = classify_command_failure_layer(event, content);
                    let entry = failing_commands
                        .entry((key.0, key.1, layer))
                        .or_insert((0, evidence_id.clone()));
                    entry.0 += 1;
                    entry.1 = evidence_id.clone();
                }
                None => {}
            }
        }

        if let Some(layer) = classify_service_failure_layer(content) {
            let entry = service_failures
                .entry((event.agent.clone(), layer))
                .or_insert((0, evidence_id.clone()));
            entry.0 += 1;
            entry.1 = evidence_id.clone();
        } else if event_can_clear_service_failure(event, content) {
            service_failures.retain(|(agent, _), _| agent != &event.agent);
        }
        if looks_like_permission_denial(content) {
            let entry = permission_denials
                .entry(event.agent.clone())
                .or_insert((0, evidence_id.clone()));
            entry.0 += 1;
            entry.1 = evidence_id.clone();
        } else if looks_like_permission_request(content) {
            let entry = permission_requests
                .entry(event.agent.clone())
                .or_insert((0, evidence_id.clone()));
            entry.0 += 1;
            entry.1 = evidence_id.clone();
        } else if event_can_clear_service_failure(event, content) {
            permission_denials.remove(&event.agent);
            permission_requests.remove(&event.agent);
        }
        if event_breaks_rediscovery_loop(event) {
            inspection_loops.retain(|(agent, _), _| agent != &event.agent);
        } else if let Some(target) = inspection_loop_target(event) {
            let entry = inspection_loops
                .entry((event.agent.clone(), target))
                .or_insert((0, evidence_id.clone()));
            entry.0 += 1;
            entry.1 = evidence_id.clone();
        }
        if let Some(action) = subagent_lifecycle_action(event) {
            let entry = unresolved_subagents
                .entry(event.agent.clone())
                .or_insert((0, evidence_id.clone()));
            match action {
                SubagentLifecycleAction::Spawned => {
                    entry.0 += 1;
                    entry.1 = evidence_id.clone();
                    for path in subagent_ownership_paths(event) {
                        let path_entry = unresolved_subagent_paths
                            .entry((event.agent.clone(), path))
                            .or_insert((0, evidence_id.clone()));
                        path_entry.0 += 1;
                        path_entry.1 = evidence_id.clone();
                    }
                }
                SubagentLifecycleAction::Terminal => {
                    entry.0 = entry.0.saturating_sub(1);
                    if entry.0 == 0 {
                        entry.1 = evidence_id.clone();
                    }
                    unresolved_subagent_paths.retain(|(agent, _), _| agent != &event.agent);
                }
            }
        }
        if looks_like_unverified_completion(content) {
            vector.raise(
                EntropyKind::Verification,
                90,
                90,
                "agent claimed completion without verification",
                Some(evidence_id.clone()),
                Some("passing verification result after completion claim".into()),
            );
        }
        if looks_like_completion_claim(content) {
            let order = (time.unwrap_or(i64::MAX), index);
            if latest_completion_claim
                .as_ref()
                .is_none_or(|(current, _, _)| order > *current)
            {
                latest_completion_claim = Some((order, evidence_id.clone(), event.agent.clone()));
            }
        }
        if looks_like_premature_stop(content) {
            vector.raise(
                EntropyKind::Plan,
                70,
                80,
                "agent attempted to stop while obvious work remained",
                Some(evidence_id.clone()),
                Some("next concrete action".into()),
            );
        }
        if event_asks_routine_user_question(event, content) {
            vector.raise(
                EntropyKind::Plan,
                74,
                84,
                "agent asked a routine user question before exhausting local probes",
                Some(evidence_id.clone()),
                Some("local probe or obvious next step before user interruption".into()),
            );
        }
        if looks_like_forgetting_design_memory(content) {
            vector.raise(
                EntropyKind::Context,
                85,
                90,
                "agent appears to have lost durable design memory",
                Some(evidence_id.clone()),
                Some("fresh handoff case file".into()),
            );
        }
        if looks_like_context_compaction(content) {
            vector.raise(
                EntropyKind::Context,
                85,
                90,
                "agent context was compacted or summarized",
                Some(evidence_id.clone()),
                Some("fresh handoff case file".into()),
            );
        }
        if looks_like_session_error(content) {
            vector.raise(
                EntropyKind::AgentHealth,
                85,
                85,
                "agent session reported a lifecycle error",
                Some(evidence_id.clone()),
                Some("recovered agent session or fallback agent".into()),
            );
        }
        if let Some(cause) = user_decision_cause_for_event(event, content) {
            vector.raise(
                EntropyKind::UserDecision,
                85,
                90,
                cause,
                Some(evidence_id.clone()),
                Some("user authorization or required external input".into()),
            );
        }
        if let Some(rejected) = event_reintroduces_rejected_alternative(
            event,
            &evidence_id,
            content,
            &rejected_alternatives,
        ) {
            let cause = format!(
                "{} reintroduced rejected alternative `{}`",
                event.agent, rejected.subject
            );
            vector.raise(
                EntropyKind::Context,
                82,
                86,
                cause.clone(),
                Some(evidence_id.clone()),
                Some("user authorization to revisit the rejected alternative".into()),
            );
            vector.raise(
                EntropyKind::Plan,
                72,
                82,
                cause,
                Some(evidence_id.clone()),
                Some("revise plan to honor rejected-alternative memory".into()),
            );
        }
        if bug_fix_goal && event_establishes_bug_reproduction_or_localization(event, content) {
            bug_fix_probe_seen = true;
        }
    }

    if policy.require_verification_after_source_change
        && let Some((write_order, evidence_id, path, surface)) =
            latest_domain_validation_write.as_ref()
        && latest_domain_validation_pass
            .get(surface)
            .is_none_or(|pass_order| pass_order < write_order)
    {
        vector.raise(
            EntropyKind::Verification,
            82,
            84,
            format!(
                "{} `{path}` lacks intended-environment validation",
                surface.change_label()
            ),
            Some(evidence_id.clone()),
            Some(surface.missing_evidence().into()),
        );
    }

    for ((agent, command, layer), (count, evidence_id)) in failing_commands {
        if count >= 3 {
            vector.raise(
                EntropyKind::AgentHealth,
                82,
                88,
                format!(
                    "{agent} repeated failing command `{command}` {count} times at {} layer",
                    layer.as_str()
                ),
                Some(evidence_id),
                Some(format!(
                    "loop-breaking retry packet after {}-layer diagnosis",
                    layer.as_str()
                )),
            );
        }
    }

    for ((agent, layer), (count, evidence_id)) in service_failures {
        if count >= 3 {
            vector.raise(
                EntropyKind::AgentHealth,
                90,
                90,
                format!(
                    "{agent} hit repeated service failures at {} layer",
                    layer.as_str()
                ),
                Some(evidence_id),
                Some(format!(
                    "{}-layer recovery evidence before retry or fallback",
                    layer.as_str()
                )),
            );
        }
    }

    for (agent, (count, evidence_id)) in permission_denials {
        if count >= 2 {
            vector.raise(
                EntropyKind::AgentHealth,
                80,
                85,
                format!("{agent} hit repeated permission denials"),
                Some(evidence_id),
                Some("permission-aware retry packet".into()),
            );
        }
    }

    for (agent, (count, evidence_id)) in permission_requests {
        if count >= 2 {
            vector.raise(
                EntropyKind::AgentHealth,
                78,
                85,
                format!("{agent} hit repeated permission requests"),
                Some(evidence_id),
                Some("permission-aware retry packet".into()),
            );
        }
    }

    for ((agent, target), (count, evidence_id)) in inspection_loops {
        if count >= 4 {
            let cause =
                format!("{agent} repeatedly inspected `{target}` {count} times without progress");
            vector.raise(
                EntropyKind::Context,
                78,
                80,
                cause.clone(),
                Some(evidence_id.clone()),
                Some("new hypothesis, edit, or verification for the inspected target".into()),
            );
            vector.raise(
                EntropyKind::Plan,
                65,
                75,
                cause,
                Some(evidence_id),
                Some("new hypothesis, edit, or verification before more broad search".into()),
            );
        }
    }

    for ((agent, _signature), loop_state) in verifier_failure_loops {
        if loop_state.repeated_after_edits > 0 {
            let command = if loop_state.command.trim().is_empty() {
                "verifier".into()
            } else {
                format!("`{}`", loop_state.command)
            };
            let cause = format!(
                "{agent} saw the same verifier failure signature recur in {command} after edits without a new hypothesis"
            );
            vector.raise(
                EntropyKind::Verification,
                86,
                88,
                cause.clone(),
                Some(loop_state.evidence_id.clone()),
                Some("failure hypothesis or isolation probe before more edits".into()),
            );
            vector.raise(
                EntropyKind::Plan,
                72,
                82,
                cause,
                Some(loop_state.evidence_id),
                Some("failure hypothesis before another edit or verifier retry".into()),
            );
        }
    }

    for (agent, (count, evidence_id)) in &unresolved_subagents {
        if *count >= SUBAGENT_WIP_CAP {
            let cause = format!(
                "{agent} has {count} unresolved spawned worker(s), reaching the subagent WIP cap"
            );
            vector.raise(
                EntropyKind::Plan,
                72,
                84,
                cause.clone(),
                Some(evidence_id.clone()),
                Some("join or cancel spawned workers before starting more subagents".into()),
            );
            vector.raise(
                EntropyKind::AgentHealth,
                72,
                80,
                cause,
                Some(evidence_id.clone()),
                Some("subagent terminal outcomes before more fan-out".into()),
            );
        }
    }

    for ((agent, path), (count, evidence_id)) in &unresolved_subagent_paths {
        if *count >= 2 {
            let cause = format!(
                "{agent} has overlapping subagent path ownership for `{path}` across {count} unresolved worker(s)"
            );
            vector.raise(
                EntropyKind::Plan,
                74,
                84,
                cause.clone(),
                Some(evidence_id.clone()),
                Some("disjoint worker path ownership or terminal worker outcomes".into()),
            );
            vector.raise(
                EntropyKind::AgentHealth,
                70,
                78,
                cause,
                Some(evidence_id.clone()),
                Some("join, cancel, or reassign overlapping subagents before more fan-out".into()),
            );
        }
    }

    let verifier_sequence_base = snapshot.recent_events.len();
    for (index, run) in snapshot.recent_verifier_runs.iter().enumerate() {
        let sequence = verifier_sequence_base + index;
        let time = verifier_run_time(run);
        match run.status {
            VerificationRunStatus::Passed => {
                if let Some(time) = time {
                    let order = (time, sequence);
                    latest_passing_verifier = Some(
                        latest_passing_verifier
                            .map(|current| current.max(order))
                            .unwrap_or(order),
                    );
                }
            }
            VerificationRunStatus::Failed | VerificationRunStatus::TimedOut => {
                let failure_order = (time.unwrap_or(i64::MAX), sequence);
                let cause = match run.status {
                    VerificationRunStatus::Failed => "verifier run failed",
                    VerificationRunStatus::TimedOut => "verifier run timed out",
                    VerificationRunStatus::Passed => unreachable!(),
                };
                if latest_failed_verifier
                    .as_ref()
                    .is_none_or(|(current, _, _, _)| failure_order > *current)
                {
                    latest_failed_verifier = Some((
                        failure_order,
                        run.verifier_run_id.clone(),
                        cause.into(),
                        "passing verifier run".into(),
                    ));
                }
            }
        }
    }

    if let Some(repo_audit) = repo_audit {
        let repo_sequence_base = snapshot.recent_events.len() + snapshot.recent_verifier_runs.len();
        for (index, change) in repo_audit.changes.iter().enumerate() {
            if !is_verification_relevant_file(&change.path, policy) {
                continue;
            }
            let order = (
                change.modified_at.unwrap_or(i64::MAX),
                repo_sequence_base + index,
            );
            if latest_source_write
                .as_ref()
                .is_none_or(|(current, _)| order > *current)
            {
                latest_source_write =
                    Some((order, format!("repo-audit-{}", safe_slug(&change.path))));
            }
        }
    }

    if policy.require_verification_after_source_change
        && let Some((write_order, evidence_id)) = latest_source_write
    {
        let stale = verification.status == VerificationStatus::Stale
            || latest_passing_verifier.is_none_or(|pass_order| pass_order < write_order);
        if stale {
            let cause = if evidence_id.starts_with("repo-audit-") {
                "dirty source/test git hunks after last passing verification"
            } else {
                "source changes after last passing verification"
            };
            vector.raise(
                EntropyKind::Verification,
                85,
                90,
                cause,
                Some(evidence_id),
                Some("passing verification after latest source change".into()),
            );
        }
    }

    let unresolved_failed_verifier =
        latest_failed_verifier
            .as_ref()
            .is_some_and(|(failure_order, _, _, _)| {
                latest_passing_verifier.is_none_or(|pass_order| pass_order <= *failure_order)
            });

    if unresolved_failed_verifier
        && let Some((_, evidence_id, cause, missing)) = latest_failed_verifier.as_ref()
    {
        vector.raise(
            EntropyKind::Verification,
            80,
            90,
            cause.clone(),
            Some(evidence_id.clone()),
            Some(missing.clone()),
        );
    }

    if unresolved_failed_verifier {
        vector.raise(
            EntropyKind::Verification,
            80,
            85,
            "failing verification has not been cleared",
            None,
            Some("later passing verification result".into()),
        );
    }

    if let Some((claim_order, evidence_id, _agent)) = latest_completion_claim.as_ref()
        && latest_passing_verifier.is_none_or(|pass_order| pass_order < *claim_order)
    {
        vector.raise(
            EntropyKind::Verification,
            84,
            86,
            "agent completion claim lacks objective verification evidence",
            Some(evidence_id.clone()),
            Some("passing verifier result after completion claim".into()),
        );
    }

    if let Some((_, completion_evidence_id, agent)) = latest_completion_claim.as_ref()
        && let Some((count, lifecycle_evidence_id)) = unresolved_subagents.get(agent)
        && *count > 0
    {
        vector.raise(
            EntropyKind::Verification,
            86,
            86,
            format!(
                "{agent} completion claim has {count} spawned worker(s) without terminal outcomes"
            ),
            Some(lifecycle_evidence_id.clone()),
            Some(
                "joined_with_summary, cancelled_with_reason, timed_out, superseded, or failed worker outcome"
                    .into(),
            ),
        );
        vector.raise(
            EntropyKind::Plan,
            76,
            82,
            "spawned worker lifecycle is unresolved at completion",
            Some(completion_evidence_id.clone()),
            Some("terminal worker outcome before completion".into()),
        );
    }

    for session in &snapshot.agent_sessions {
        match session.status {
            AgentActivityStatus::Degraded => vector.raise(
                EntropyKind::AgentHealth,
                85,
                85,
                format!("{} is degraded", session.agent),
                None,
                Some("fresh or recovered agent session".into()),
            ),
            AgentActivityStatus::Stale => vector.raise(
                EntropyKind::AgentHealth,
                60,
                75,
                format!("{} is stale", session.agent),
                None,
                Some("recent agent event".into()),
            ),
            AgentActivityStatus::Active => {}
        }
    }

    if !verification.recommended_commands.is_empty() {
        let verification_score = vector.score_mut(EntropyKind::Verification);
        for command in &verification.recommended_commands {
            if !verification_score
                .recommended_observations
                .contains(command)
            {
                verification_score
                    .recommended_observations
                    .push(command.clone());
            }
        }
    }

    if !verification.uncovered_acceptance_criteria.is_empty() {
        let acceptance_evidence_id =
            acceptance_criteria_evidence_id(intent_events, &verification.acceptance_criteria);
        vector.raise(
            EntropyKind::Verification,
            82,
            80,
            "acceptance criteria have no mapped verifier",
            acceptance_evidence_id,
            Some("mapped verifier for acceptance criterion".into()),
        );
    }

    let covered_acceptance_exists = !verification.acceptance_criteria.is_empty()
        && verification.uncovered_acceptance_criteria.len()
            < verification.acceptance_criteria.len()
        && !verification.recommended_commands.is_empty();
    if covered_acceptance_exists
        && verification
            .latest_passing_command
            .as_ref()
            .is_none_or(|command| {
                !verification
                    .recommended_commands
                    .iter()
                    .any(|recommended| recommended == command)
            })
    {
        let acceptance_evidence_id =
            acceptance_criteria_evidence_id(intent_events, &verification.acceptance_criteria);
        vector.raise(
            EntropyKind::Verification,
            78,
            80,
            "acceptance criteria verifier has not passed",
            acceptance_evidence_id,
            Some("passing verifier for acceptance criterion".into()),
        );
    }

    if let Some(repo_audit) = repo_audit {
        let first_untraced = repo_audit
            .changes
            .iter()
            .find(|change| change.trace_status == RepoTraceStatus::Untraced);
        if let Some(change) = first_untraced {
            vector.raise(
                EntropyKind::RepoBlame,
                88,
                90,
                "dirty git hunks lack trace evidence",
                Some(format!("repo-audit-{}", safe_slug(&change.path))),
                Some("trace rationale for every dirty hunk".into()),
            );
        } else if let Some(change) = repo_audit
            .changes
            .iter()
            .find(|change| change.trace_status == RepoTraceStatus::MissingRationale)
        {
            vector.raise(
                EntropyKind::RepoBlame,
                78,
                88,
                "dirty git hunks have trace evidence without rationale",
                Some(format!("repo-audit-{}", safe_slug(&change.path))),
                Some("rationale for every traced dirty hunk".into()),
            );
        }
    }

    apply_dev_history_entropy_priors(&mut vector, snapshot, repo_audit);

    vector
}

struct DevHistoryEntropyPrior {
    kind: EntropyKind,
    score: u8,
    confidence: u8,
    cause: &'static str,
    missing_evidence: &'static str,
    recommended_observation: &'static str,
}

fn apply_dev_history_entropy_priors(
    vector: &mut EntropyVector,
    snapshot: &DashboardSnapshot,
    repo_audit: Option<&RepoAuditReport>,
) {
    for report in &snapshot.recent_dev_history {
        for (finding_index, finding) in report.findings.iter().enumerate() {
            let Some(prior) = dev_history_entropy_prior(finding.kind.as_str()) else {
                continue;
            };
            if prior.kind == EntropyKind::RepoBlame
                && !dev_history_blame_hotspot_overlaps_current_change(finding, snapshot, repo_audit)
            {
                continue;
            }
            let evidence_id = dev_history_finding_evidence_id(report, finding_index, finding);
            vector.raise(
                prior.kind,
                prior.score,
                prior.confidence,
                prior.cause,
                Some(evidence_id),
                Some(prior.missing_evidence.into()),
            );
            let score = vector.score_mut(prior.kind);
            if !score
                .recommended_observations
                .iter()
                .any(|observation| observation == prior.recommended_observation)
            {
                score
                    .recommended_observations
                    .push(prior.recommended_observation.into());
            }
        }
    }
}

fn dev_history_blame_hotspot_overlaps_current_change(
    finding: &DevHistoryFinding,
    snapshot: &DashboardSnapshot,
    repo_audit: Option<&RepoAuditReport>,
) -> bool {
    let mut current_paths = snapshot
        .recent_events
        .iter()
        .filter(|event| event_is_change_like(event))
        .filter_map(|event| event.file.as_deref())
        .map(normalize_path_for_match)
        .filter(|path| !path.is_empty())
        .collect::<Vec<_>>();

    if let Some(repo_audit) = repo_audit {
        current_paths.extend(
            repo_audit
                .changes
                .iter()
                .map(|change| normalize_path_for_match(&change.path))
                .filter(|path| !path.is_empty()),
        );
    }

    current_paths.iter().any(|path| {
        finding
            .evidence
            .iter()
            .any(|evidence| dev_history_hotspot_matches_path(evidence, path))
    })
}

fn dev_history_hotspot_matches_path(evidence: &str, current_path: &str) -> bool {
    let hotspot = evidence
        .split_once(" (")
        .map_or(evidence, |(path, _)| path)
        .trim();
    let hotspot = normalize_path_for_match(hotspot);
    if hotspot.is_empty() || current_path.is_empty() {
        return false;
    }
    hotspot == current_path
        || current_path
            .strip_suffix(&hotspot)
            .is_some_and(|prefix| prefix.ends_with('/'))
        || hotspot
            .strip_suffix(current_path)
            .is_some_and(|prefix| prefix.ends_with('/'))
}

fn dev_history_entropy_prior(kind: &str) -> Option<DevHistoryEntropyPrior> {
    match kind {
        "verification_entropy" => Some(DevHistoryEntropyPrior {
            kind: EntropyKind::Verification,
            score: 58,
            confidence: 60,
            cause: "local dev-history shows recurring verification uncertainty",
            missing_evidence: "fresh verifier evidence for the current run",
            recommended_observation: "check verifier freshness against the latest current-run write",
        }),
        "agent_health_entropy" => Some(DevHistoryEntropyPrior {
            kind: EntropyKind::AgentHealth,
            score: 55,
            confidence: 55,
            cause: "local dev-history shows recurring agent-health instability",
            missing_evidence: "current-run loop, provider, or tool-failure evidence",
            recommended_observation: "watch for repeated failures before retry or handoff",
        }),
        "blame_hotspots" => Some(DevHistoryEntropyPrior {
            kind: EntropyKind::RepoBlame,
            score: 55,
            confidence: 55,
            cause: "local dev-history shows recurring repo/blame hotspots",
            missing_evidence: "current-run hunk rationale for touched hotspot files",
            recommended_observation: "compare current dirty hunks with trace and rationale records",
        }),
        "subagent_lifecycle_entropy" => Some(DevHistoryEntropyPrior {
            kind: EntropyKind::Plan,
            score: 55,
            confidence: 55,
            cause: "local dev-history shows recurring subagent lifecycle fragmentation",
            missing_evidence: "current-run terminal worker outcomes and integration summaries",
            recommended_observation: "require joined_with_summary, cancelled_with_reason, timed_out, superseded, or failed outcomes before more fan-out",
        }),
        _ => None,
    }
}

fn acceptance_criteria_evidence_id(
    intent_events: &[Event],
    acceptance_criteria: &[String],
) -> Option<String> {
    if acceptance_criteria.is_empty() {
        return None;
    }
    intent_events
        .iter()
        .enumerate()
        .rev()
        .filter(|(_, event)| event.kind == EventKind::UserInstruction)
        .find_map(|(index, event)| {
            let content = event.content.as_deref()?;
            let extracted = extract_acceptance_criteria(content);
            let introduced = extracted
                .iter()
                .any(|criterion| acceptance_criteria.iter().any(|target| target == criterion));
            if introduced {
                Some(event_evidence_id(event, index))
            } else {
                None
            }
        })
}

fn verification_summary(
    snapshot: &DashboardSnapshot,
    intent_events: &[Event],
    verifiers: &[VerifierConfig],
    policy: &PolicyConfig,
    repo_audit: Option<&RepoAuditReport>,
) -> VerificationSummary {
    let acceptance_criteria = acceptance_criteria_from_events(intent_events);
    let uncovered_acceptance_criteria = acceptance_criteria
        .iter()
        .filter(|criterion| {
            !verifiers
                .iter()
                .any(|verifier| verifier_matches_acceptance(verifier, criterion))
        })
        .cloned()
        .collect::<Vec<_>>();
    let mut changed_source_files = Vec::new();
    for event in &snapshot.recent_events {
        if event_is_change_like(event)
            && event
                .file
                .as_deref()
                .is_some_and(|file| is_verification_relevant_file(file, policy))
            && let Some(file) = &event.file
        {
            push_changed_source_file(&mut changed_source_files, file);
        }
    }
    if let Some(repo_audit) = repo_audit {
        for change in &repo_audit.changes {
            if is_verification_relevant_file(&change.path, policy) {
                push_changed_source_file(&mut changed_source_files, &change.path);
            }
        }
    }

    let mut latest_passing: Option<((i64, usize), String)> = None;
    let mut latest_failing: Option<((i64, usize), String, Option<VerificationFailureClass>)> = None;
    for (index, event) in snapshot.recent_events.iter().enumerate() {
        if !event_is_verification_result(event) {
            continue;
        }
        let Some(time) = event.time.as_deref().and_then(parse_utc_seconds) else {
            continue;
        };
        let order = (time, index);
        let command = event.command.clone().unwrap_or_default();
        match event.exit_code {
            Some(0) => {
                if latest_passing
                    .as_ref()
                    .is_none_or(|(current, _)| order > *current)
                {
                    latest_passing = Some((order, command));
                }
            }
            Some(_)
                if latest_failing
                    .as_ref()
                    .is_none_or(|(current, _, _)| order > *current) =>
            {
                latest_failing = Some((order, command, None));
            }
            Some(_) => {}
            None => {}
        }
    }
    let verifier_sequence_base = snapshot.recent_events.len();
    for (index, run) in snapshot.recent_verifier_runs.iter().enumerate() {
        let Some(time) = verifier_run_time(run) else {
            continue;
        };
        let order = (time, verifier_sequence_base + index);
        match run.status {
            VerificationRunStatus::Passed => {
                if latest_passing
                    .as_ref()
                    .is_none_or(|(current, _)| order > *current)
                {
                    latest_passing = Some((order, run.command.clone()));
                }
            }
            VerificationRunStatus::Failed | VerificationRunStatus::TimedOut
                if latest_failing
                    .as_ref()
                    .is_none_or(|(current, _, _)| order > *current) =>
            {
                latest_failing = Some((order, run.command.clone(), run.failure_class));
            }
            VerificationRunStatus::Failed | VerificationRunStatus::TimedOut => {}
        }
    }
    let status = latest_verification_status(snapshot, verifiers, policy, repo_audit);
    let acceptance_coverage =
        acceptance_coverage_for_criteria(&acceptance_criteria, verifiers, snapshot, status);

    let mut recommended_commands = Vec::new();
    for verifier in verifiers {
        let matches_changed_path = verifier.paths.is_empty()
            || changed_source_files
                .iter()
                .any(|file| verifier_matches_path(verifier, file));
        let matches_acceptance = acceptance_criteria
            .iter()
            .any(|criterion| verifier_matches_acceptance(verifier, criterion));
        if (matches_changed_path || matches_acceptance)
            && !recommended_commands.contains(&verifier.command)
        {
            recommended_commands.push(verifier.command.clone());
        }
    }

    VerificationSummary {
        status,
        recommended_commands,
        changed_source_files,
        acceptance_criteria,
        uncovered_acceptance_criteria,
        acceptance_coverage,
        latest_passing_command: latest_passing.map(|(_, command)| command),
        latest_failing_command: latest_failing
            .as_ref()
            .map(|(_, command, _)| command.clone()),
        latest_failure_class: latest_failing.and_then(|(_, _, failure_class)| failure_class),
    }
}

fn acceptance_coverage_for_criteria(
    acceptance_criteria: &[String],
    verifiers: &[VerifierConfig],
    snapshot: &DashboardSnapshot,
    verification_status: VerificationStatus,
) -> Vec<AcceptanceCoverage> {
    let latest_status_by_key = latest_verification_status_by_key(snapshot);
    acceptance_criteria
        .iter()
        .map(|criterion| {
            let mapped = verifiers
                .iter()
                .filter(|verifier| verifier_matches_acceptance(verifier, criterion))
                .collect::<Vec<_>>();
            let latest_status = latest_status_for_verifiers(&mapped, &latest_status_by_key);
            AcceptanceCoverage {
                criterion: criterion.clone(),
                status: acceptance_coverage_status(
                    !mapped.is_empty(),
                    latest_status.map(|(_, status)| status),
                    verification_status,
                ),
                verifier_ids: mapped.iter().map(|verifier| verifier.id.clone()).collect(),
                verifier_commands: mapped
                    .iter()
                    .map(|verifier| verifier.command.clone())
                    .collect(),
                latest_status: latest_status.map(|(_, status)| status),
            }
        })
        .collect()
}

fn latest_verification_status_by_key(
    snapshot: &DashboardSnapshot,
) -> HashMap<String, ((i64, usize), VerificationStatus)> {
    let mut latest = HashMap::<String, ((i64, usize), VerificationStatus)>::new();
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
        let status = match event.exit_code {
            Some(0) => VerificationStatus::Passed,
            Some(_) => VerificationStatus::Failed,
            None => continue,
        };
        update_latest_verification_status(
            &mut latest,
            normalize_command_signature(command),
            (time, index),
            status,
        );
    }

    let verifier_sequence_base = snapshot.recent_events.len();
    for (index, run) in snapshot.recent_verifier_runs.iter().enumerate() {
        let Some(time) = verifier_run_time(run) else {
            continue;
        };
        let order = (time, verifier_sequence_base + index);
        let status = verifier_run_verification_status(run);
        update_latest_verification_status(
            &mut latest,
            normalize_command_signature(&run.command),
            order,
            status,
        );
        if let Some(verifier_id) = run.verifier_id.as_deref() {
            update_latest_verification_status(&mut latest, verifier_id.to_string(), order, status);
        }
    }
    latest
}

fn update_latest_verification_status(
    latest: &mut HashMap<String, ((i64, usize), VerificationStatus)>,
    key: String,
    order: (i64, usize),
    status: VerificationStatus,
) {
    if key.trim().is_empty() {
        return;
    }
    if latest
        .get(&key)
        .is_none_or(|(current_order, _)| order > *current_order)
    {
        latest.insert(key, (order, status));
    }
}

fn latest_status_for_verifiers(
    verifiers: &[&VerifierConfig],
    latest_status_by_key: &HashMap<String, ((i64, usize), VerificationStatus)>,
) -> Option<((i64, usize), VerificationStatus)> {
    verifiers
        .iter()
        .filter_map(|verifier| {
            let command_key = normalize_command_signature(&verifier.command);
            latest_status_by_key
                .get(&command_key)
                .or_else(|| latest_status_by_key.get(&verifier.id))
                .copied()
        })
        .max_by_key(|(order, _)| *order)
}

fn acceptance_coverage_status(
    has_mapping: bool,
    latest_status: Option<VerificationStatus>,
    verification_status: VerificationStatus,
) -> AcceptanceCoverageStatus {
    if !has_mapping {
        return AcceptanceCoverageStatus::Unmapped;
    }
    match latest_status {
        Some(VerificationStatus::Passed) if verification_status == VerificationStatus::Stale => {
            AcceptanceCoverageStatus::Stale
        }
        Some(VerificationStatus::Passed) => AcceptanceCoverageStatus::Covered,
        Some(VerificationStatus::Failed) => AcceptanceCoverageStatus::Failed,
        Some(VerificationStatus::Stale) => AcceptanceCoverageStatus::Stale,
        Some(VerificationStatus::NotRun) | None => AcceptanceCoverageStatus::Unverified,
    }
}

fn push_changed_source_file(files: &mut Vec<String>, file: &str) {
    if !files.iter().any(|existing| existing == file) {
        files.push(file.to_string());
    }
}

fn acceptance_criteria_from_events(events: &[Event]) -> Vec<String> {
    let mut criteria = Vec::new();
    for event in events {
        if event.kind != EventKind::UserInstruction {
            continue;
        }
        let Some(content) = event.content.as_deref() else {
            continue;
        };
        for criterion in extract_acceptance_criteria(content) {
            if !criteria.iter().any(|existing| existing == &criterion) {
                criteria.push(criterion);
            }
        }
    }
    criteria
}

fn extract_acceptance_criteria(content: &str) -> Vec<String> {
    let mut criteria = Vec::new();
    let mut in_acceptance_block = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            in_acceptance_block = false;
            continue;
        }

        if let Some(rest) = acceptance_prefix_rest(trimmed) {
            if rest.trim().is_empty() {
                in_acceptance_block = true;
            } else {
                append_inline_acceptance_criteria(&mut criteria, rest);
                in_acceptance_block = false;
            }
            continue;
        }

        if in_acceptance_block {
            let Some(item) = acceptance_block_item(trimmed) else {
                in_acceptance_block = false;
                continue;
            };
            let item = clean_acceptance_criterion(item);
            if !item.is_empty() {
                criteria.push(item);
            }
        }
    }
    criteria
}

fn append_inline_acceptance_criteria(criteria: &mut Vec<String>, value: &str) {
    for item in inline_acceptance_items(value) {
        let item = clean_acceptance_criterion(item);
        if !item.is_empty() {
            criteria.push(item);
        }
    }
}

fn inline_acceptance_items(value: &str) -> Vec<&str> {
    let trimmed = value.trim();
    if let Some(rest) = trimmed.strip_prefix("- ") {
        return rest.split(" - ").collect();
    }
    if let Some(rest) = trimmed.strip_prefix("* ") {
        return rest.split(" * ").collect();
    }
    vec![trimmed]
}

fn acceptance_prefix_rest(line: &str) -> Option<&str> {
    let lower = line.to_lowercase();
    for prefix in [
        "acceptance:",
        "acceptance criterion:",
        "acceptance criteria:",
        "acceptance criteria -",
        "acceptance criteria - ",
    ] {
        if lower.starts_with(prefix) {
            return Some(line[prefix.len()..].trim());
        }
    }
    None
}

fn acceptance_block_item(line: &str) -> Option<&str> {
    let line = line.trim_start();
    if let Some(rest) = line.strip_prefix("- ") {
        return Some(rest);
    }
    if let Some(rest) = line.strip_prefix("* ") {
        return Some(rest);
    }
    if let Some(rest) = line.strip_prefix("[ ] ") {
        return Some(rest);
    }
    if let Some(rest) = line.strip_prefix("[x] ") {
        return Some(rest);
    }
    if let Some((number, rest)) = line.split_once(". ")
        && number.chars().all(|ch| ch.is_ascii_digit())
    {
        return Some(rest);
    }
    None
}

fn clean_acceptance_criterion(value: &str) -> String {
    value
        .trim()
        .trim_start_matches("- ")
        .trim_start_matches("* ")
        .trim()
        .to_string()
}

#[cfg(test)]
mod acceptance_extraction_tests {
    use super::*;

    #[test]
    fn extracts_acceptance_block_after_goal_sentence() {
        assert_eq!(
            extract_acceptance_criteria(
                "Build the monitor supervisor.\nAcceptance criteria:\n- parser handles nested calls\n- advisor packets cite evidence",
            ),
            vec![
                "parser handles nested calls".to_string(),
                "advisor packets cite evidence".to_string()
            ]
        );
    }
}

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

const USER_DECISION_ASK_USER_THRESHOLD: u8 = 80;
const RETRY_AGENT_HEALTH_THRESHOLD: u8 = 75;
const FOLLOW_UP_PLAN_THRESHOLD: u8 = 60;
const FOLLOW_UP_REPO_BLAME_THRESHOLD: u8 = 75;
const SPAWN_FRESH_CONTEXT_THRESHOLD: u8 = 80;
const SPAWN_FRESH_AGENT_HEALTH_THRESHOLD: u8 = 90;
const SWITCH_AGENT_HEALTH_THRESHOLD: u8 = 90;
const SUBAGENT_WIP_CAP: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum FailureLayer {
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
    fn as_str(self) -> &'static str {
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
enum SubagentLifecycleAction {
    Spawned,
    Terminal,
}

#[derive(Debug, Clone, Default)]
struct VerifierFailureLoop {
    command: String,
    repeated_after_edits: usize,
    edits_since_last_failure: usize,
    hypothesis_since_last_failure: bool,
    evidence_id: String,
}

fn verifier_run_verification_status(run: &VerifierRun) -> VerificationStatus {
    match run.status {
        VerificationRunStatus::Passed => VerificationStatus::Passed,
        VerificationRunStatus::Failed | VerificationRunStatus::TimedOut => {
            VerificationStatus::Failed
        }
    }
}

fn normalize_command_signature(command: &str) -> String {
    command.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn event_is_verification_result(event: &Event) -> bool {
    matches!(event.kind, EventKind::CommandResult | EventKind::TestResult)
        && event
            .command
            .as_deref()
            .is_some_and(is_verification_command)
}

fn event_is_intended_environment_validation_result(event: &Event) -> bool {
    matches!(event.kind, EventKind::CommandResult | EventKind::TestResult)
        && event
            .command
            .as_deref()
            .is_some_and(|command| !validation_surfaces_for_command(command).is_empty())
}

fn verifier_failure_signature(event: &Event) -> Option<String> {
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

fn verifier_failure_output_signature(output: &str) -> String {
    let first_line = output
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("<empty>");
    truncate_evidence(&normalize_command_signature(first_line)).to_ascii_lowercase()
}

fn event_records_failure_hypothesis(event: &Event, content: &str) -> bool {
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

fn event_establishes_bug_reproduction_or_localization(event: &Event, content: &str) -> bool {
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

fn is_localization_probe_command(command: &str) -> bool {
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

fn event_is_change_like(event: &Event) -> bool {
    matches!(event.kind, EventKind::FileChange | EventKind::RepoDiff)
}

fn event_breaks_rediscovery_loop(event: &Event) -> bool {
    event_is_change_like(event)
        || event_is_verification_result(event)
        || matches!(
            event.kind,
            EventKind::UserInstruction | EventKind::DesignThought | EventKind::HandoffSummary
        )
}

fn inspection_loop_target(event: &Event) -> Option<String> {
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

fn inspection_command_target(command: &str) -> Option<String> {
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

fn subagent_lifecycle_action(event: &Event) -> Option<SubagentLifecycleAction> {
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

fn subagent_lifecycle_text(event: &Event) -> String {
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

fn subagent_ownership_paths(event: &Event) -> Vec<String> {
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

fn collect_subagent_ownership_paths_from_text(paths: &mut Vec<String>, text: &str) {
    for token in text.split_whitespace() {
        let candidate = token
            .split_once('=')
            .map(|(_, value)| value)
            .unwrap_or(token);
        push_subagent_ownership_path(paths, candidate);
    }
}

fn push_subagent_ownership_path(paths: &mut Vec<String>, candidate: &str) {
    let path = normalize_subagent_ownership_path(candidate);
    if path.is_empty() || paths.contains(&path) {
        return;
    }
    paths.push(path);
}

fn normalize_subagent_ownership_path(candidate: &str) -> String {
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

fn subagent_task_tool_started(event: &Event) -> bool {
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

fn event_is_successful_result(event: &Event) -> bool {
    matches!(
        event.kind,
        EventKind::CommandResult | EventKind::ToolResult | EventKind::TestResult
    ) && event.exit_code == Some(0)
}

fn event_can_clear_service_failure(event: &Event, content: &str) -> bool {
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

fn retry_agent_entropy_allowed(case_file: &ControlCaseFile) -> bool {
    case_file
        .entropy
        .score(EntropyKind::AgentHealth)
        .is_some_and(|score| score.score >= RETRY_AGENT_HEALTH_THRESHOLD)
}

fn send_follow_up_entropy_allowed(case_file: &ControlCaseFile) -> bool {
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

fn run_probe_entropy_allowed(case_file: &ControlCaseFile) -> bool {
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

fn probe_spec_for_case_file(case_file: &ControlCaseFile) -> ProbeSpec {
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

fn spawn_judge_entropy_allowed(case_file: &ControlCaseFile) -> bool {
    case_file
        .entropy
        .score(EntropyKind::RepoBlame)
        .is_some_and(|score| score.score >= FOLLOW_UP_REPO_BLAME_THRESHOLD)
}

fn switch_agent_entropy_allowed(case_file: &ControlCaseFile) -> bool {
    case_file
        .entropy
        .score(EntropyKind::AgentHealth)
        .is_some_and(|score| score.score >= SWITCH_AGENT_HEALTH_THRESHOLD)
}

fn spawn_fresh_entropy_allowed(case_file: &ControlCaseFile) -> bool {
    case_file
        .entropy
        .score(EntropyKind::Context)
        .is_some_and(|score| score.score >= SPAWN_FRESH_CONTEXT_THRESHOLD)
        || case_file
            .entropy
            .score(EntropyKind::AgentHealth)
            .is_some_and(|score| score.score >= SPAWN_FRESH_AGENT_HEALTH_THRESHOLD)
}

fn deterministic_control_action(case_file: &ControlCaseFile) -> ControlAction {
    deterministic_control_action_with_calibration(case_file, &ControlCalibration::default())
}

fn deterministic_control_action_with_calibration(
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

fn force_verification_control_action(case_file: &ControlCaseFile) -> ControlAction {
    ControlAction::ForceVerification {
        suite: if case_file.verification.recommended_commands.is_empty() {
            VerificationSuite::Full
        } else {
            VerificationSuite::Targeted
        },
        blocking: true,
    }
}

fn trace_and_verification_block_required(case_file: &ControlCaseFile) -> bool {
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

fn trace_and_verification_block_action(case_file: &ControlCaseFile) -> ControlAction {
    let reason = case_file
        .entropy
        .score(EntropyKind::RepoBlame)
        .and_then(|score| score.top_causes.first())
        .cloned()
        .unwrap_or_else(|| "repo/blame entropy requires trace and verification repair".into());
    ControlAction::BlockProgressUntilTraceAndVerification { reason }
}

#[derive(Debug)]
struct ControlActionCandidate {
    action: ControlAction,
    utility: i32,
    priority: i32,
}

#[derive(Debug, Clone, Default)]
struct ControlCalibration {
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

fn load_control_calibration(workspace: &Path) -> Result<ControlCalibration, StoreError> {
    let report = load_calibration_report(
        workspace,
        CalibrationQuery {
            limit: 0,
            action: None,
        },
    )?;
    Ok(control_calibration_from_report(&report))
}

fn control_calibration_from_report(report: &CalibrationReport) -> ControlCalibration {
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

fn calibrated_expected_deltas_from_history(
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

fn shrink_calibrated_delta(prior: i32, observed_average: i32, outcome_count: i32) -> i32 {
    const PRIOR_WEIGHT: i32 = 3;
    rounded_div(
        observed_average * outcome_count + prior * PRIOR_WEIGHT,
        outcome_count + PRIOR_WEIGHT,
    )
}

fn rounded_div(numerator: i32, denominator: i32) -> i32 {
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

fn calibration_underperformance_error(expected: &[EntropyDelta], observed: &[EntropyDelta]) -> i32 {
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

fn entropy_delta_map(deltas: &[EntropyDelta]) -> BTreeMap<EntropyKind, i32> {
    let mut map = BTreeMap::new();
    for delta in deltas {
        *map.entry(delta.kind).or_default() += i32::from(delta.delta);
    }
    map
}

fn calibration_penalty(
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

fn calibration_skips_action(action: ControlActionKind) -> bool {
    matches!(
        action,
        ControlActionKind::ForceVerification
            | ControlActionKind::BlockProgressUntilTraceAndVerification
            | ControlActionKind::Pause
    )
}

fn utility_ranked_control_action(
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

fn push_control_candidate(
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

fn control_action_utility(
    action: &ControlAction,
    case_file: &ControlCaseFile,
    dominant: Option<EntropyKind>,
    calibration: &ControlCalibration,
) -> i32 {
    expected_entropy_reduction_score(action, case_file, dominant, Some(calibration))
        - control_action_cost(action)
        - calibration.penalty_for(action)
}

fn expected_entropy_reduction_score(
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

fn entropy_score_points(case_file: &ControlCaseFile, kind: EntropyKind) -> i32 {
    case_file
        .entropy
        .score(kind)
        .map(|score| i32::from(score.score))
        .unwrap_or_default()
}

fn control_action_cost(action: &ControlAction) -> i32 {
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

fn control_action_tie_priority(kind: ControlActionKind) -> i32 {
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

fn control_rationale_for_action(
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

fn requirement_ids_for_control_rationale(
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

fn action_enforces_project_contract_requirement(
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

fn project_contract_is_stale_verification_invariant(text: &str) -> bool {
    text.contains("do not continue after source/test changes")
        && text.contains("verification is stale")
}

fn dominant_entropy(case_file: &ControlCaseFile) -> Option<EntropyKind> {
    case_file
        .entropy
        .scores
        .iter()
        .filter(|score| score.score > 0)
        .max_by_key(|score| score.score)
        .map(|score| score.kind)
}

fn expected_entropy_delta_for_control_action(
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

fn calibrated_expected_entropy_delta_for_control_action(
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

fn merge_calibrated_expected_deltas(
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

fn spawn_fresh_agent_health_target_is_stronger(
    case_file: &ControlCaseFile,
    dominant: Option<EntropyKind>,
) -> bool {
    let agent_health = entropy_score_points(case_file, EntropyKind::AgentHealth);
    let context = entropy_score_points(case_file, EntropyKind::Context);
    agent_health >= i32::from(SPAWN_FRESH_AGENT_HEALTH_THRESHOLD)
        && (matches!(dominant, Some(EntropyKind::AgentHealth)) || agent_health >= context)
}

fn send_follow_up_repo_blame_target_is_stronger(case_file: &ControlCaseFile) -> bool {
    let repo_blame = entropy_score_points(case_file, EntropyKind::RepoBlame);
    let plan = entropy_score_points(case_file, EntropyKind::Plan);
    repo_blame >= i32::from(FOLLOW_UP_REPO_BLAME_THRESHOLD)
        && (plan < i32::from(FOLLOW_UP_PLAN_THRESHOLD) || repo_blame >= plan)
}

fn evidence_ids_for_control_rationale(
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

fn control_rationale_reason(
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

fn control_calibration_rationale_suffix(
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

fn calibrated_delta_description(deltas: &[EntropyDelta]) -> String {
    let mut descriptions = deltas
        .iter()
        .map(|delta| format!("{}={}", entropy_kind_label(delta.kind), delta.delta))
        .collect::<Vec<_>>();
    descriptions.sort();
    descriptions.join(", ")
}

fn entropy_kind_label(kind: EntropyKind) -> &'static str {
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

fn ask_user_question_for_case_file(case_file: &ControlCaseFile) -> String {
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

fn agent_for_entropy(case_file: &ControlCaseFile, kind: EntropyKind) -> Option<String> {
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

fn fallback_agent_for_case_file(
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

fn judge_agent_for_case_file(case_file: &ControlCaseFile) -> Option<String> {
    ["claude-code", "opencode", "codex", "pi"]
        .into_iter()
        .find(|agent| adapter_can_receive_readonly_judge(case_file, agent))
        .map(str::to_string)
}

fn adapter_can_receive_writable_handoff(case_file: &ControlCaseFile, agent: &str) -> bool {
    adapter_capabilities_for_case_file(case_file, agent)
        .is_some_and(adapter_capability_allows_writable_handoff)
}

fn adapter_can_receive_readonly_judge(case_file: &ControlCaseFile, agent: &str) -> bool {
    adapter_capabilities_for_case_file(case_file, agent)
        .is_some_and(adapter_capability_allows_readonly_judge)
}

fn adapter_capability_allows_writable_handoff(capabilities: &AdapterCapabilities) -> bool {
    capabilities.enabled
        && capabilities.supports_workspace_write_mode
        && !capabilities.requires_external_sandbox
}

fn adapter_capability_allows_readonly_judge(capabilities: &AdapterCapabilities) -> bool {
    capabilities.enabled && capabilities.can_inject_context && capabilities.supports_readonly_mode
}

fn adapter_capabilities_for_case_file<'a>(
    case_file: &'a ControlCaseFile,
    agent: &str,
) -> Option<&'a AdapterCapabilities> {
    AgentKind::from_str(agent)
        .ok()
        .and_then(|kind| case_file.adapter_capabilities.get(agent_kind_label(kind)))
}

fn unsafe_writable_handoff_reason(case_file: &ControlCaseFile, agent: &str) -> String {
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

fn control_packet_for_action(action: &ControlAction, case_file: &ControlCaseFile) -> ControlPacket {
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

fn verification_packet_summary(case_file: &ControlCaseFile) -> String {
    if verification_entropy_mentions_completion_claim(case_file) {
        "The monitor found an agent completion claim without objective verification evidence."
            .into()
    } else {
        "Verification evidence is missing, stale, or failed for the current work.".into()
    }
}

fn run_probe_packet_instructions(probe: &ProbeSpec) -> Vec<PacketInstruction> {
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

fn verification_entropy_mentions_completion_claim(case_file: &ControlCaseFile) -> bool {
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

fn verification_entropy_mentions_unresolved_subagents(case_file: &ControlCaseFile) -> bool {
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

fn verification_entropy_mentions_intended_environment_validation(
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

fn intended_environment_validation_packet_instruction(
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

fn intended_environment_validation_surfaces(case_file: &ControlCaseFile) -> Vec<ValidationSurface> {
    let mut surfaces = Vec::new();
    for file in &case_file.verification.changed_source_files {
        if let Some(surface) = validation_surface_for_path(file) {
            push_validation_surface(&mut surfaces, surface);
        }
    }
    if !surfaces.is_empty() {
        return surfaces;
    }

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

fn verification_entropy_mentions_test_oracle_authority(case_file: &ControlCaseFile) -> bool {
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

fn verification_entropy_mentions_repeated_failure_signature(case_file: &ControlCaseFile) -> bool {
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

fn plan_entropy_mentions_subagent_wip_cap(case_file: &ControlCaseFile) -> bool {
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

fn plan_entropy_mentions_overlapping_subagent_path_ownership(case_file: &ControlCaseFile) -> bool {
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

fn plan_entropy_mentions_routine_agent_question(case_file: &ControlCaseFile) -> bool {
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

fn plan_entropy_mentions_bug_fix_pre_edit_gap(case_file: &ControlCaseFile) -> bool {
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

fn plan_entropy_mentions_repeated_inspection_loop(case_file: &ControlCaseFile) -> bool {
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

fn context_entropy_mentions_rejected_alternative(case_file: &ControlCaseFile) -> bool {
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

fn target_agent_for_action(action: &ControlAction, case_file: &ControlCaseFile) -> String {
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

fn acceptance_coverage_packet_instruction(
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

fn handoff_packet_for_agent(target_agent: AgentKind, case_file: &ControlCaseFile) -> ControlPacket {
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

fn verification_status_label(status: VerificationStatus) -> &'static str {
    match status {
        VerificationStatus::Passed => "passed",
        VerificationStatus::Failed => "failed",
        VerificationStatus::Stale => "stale",
        VerificationStatus::NotRun => "not_run",
    }
}

fn verification_instruction_text(case_file: &ControlCaseFile) -> String {
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

fn verification_failure_class_packet_guidance(
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

fn belief_instruction_text(case_file: &ControlCaseFile) -> String {
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

fn failure_hypothesis_label(kind: FailureHypothesisKind) -> &'static str {
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

fn memory_instruction_text(case_file: &ControlCaseFile) -> String {
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

fn trace_instruction_text(case_file: &ControlCaseFile) -> String {
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

fn packet_preconditions_for_case_file(
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

fn latest_evidence_value_for_agent<F>(
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

fn render_control_packet(packet: &ControlPacket) -> String {
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

fn validate_control_packet_is_clean(packet: &ControlPacket) -> Result<(), StoreError> {
    for (field, value) in control_packet_text_fields(packet) {
        if packet_text_is_tainted(&value) {
            return Err(StoreError::SecretLikePacket { field });
        }
    }
    Ok(())
}

fn packet_evidence_refs(packet: &ControlPacket) -> Vec<&str> {
    packet
        .evidence_refs
        .iter()
        .map(|evidence| evidence.trim())
        .filter(|evidence| !evidence.is_empty())
        .collect()
}

fn case_file_known_evidence_ids(case_file: &ControlCaseFile) -> HashSet<&str> {
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

fn packet_has_preconditions(preconditions: &PacketPreconditions) -> bool {
    preconditions.adapter.is_some()
        || preconditions.run_id.is_some()
        || preconditions.agent_session_id.is_some()
        || preconditions.worktree.is_some()
        || preconditions.git_head.is_some()
}

fn control_packet_text_fields(packet: &ControlPacket) -> Vec<(String, String)> {
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

fn adapter_packet_heading(agent: AgentKind, urgency: PacketUrgency) -> &'static str {
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

fn default_adapter_enabled() -> bool {
    true
}

fn normalize_agent_label(value: &str) -> String {
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

fn worktree_lock_path(store_root: &Path, worktree: &str) -> PathBuf {
    store_root.join("locks").join("worktrees").join(format!(
        "{}.json",
        safe_slug(&normalize_path_text(worktree))
    ))
}

fn read_worktree_lock(path: &Path) -> Result<WorktreeLock, StoreError> {
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

static NEXT_ID_SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn current_id_fragment() -> String {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default();
    let sequence = NEXT_ID_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("{millis}-{}-{sequence}", std::process::id())
}

fn current_git_head(workspace: &Path) -> Option<String> {
    let output = Command::new("git")
        .current_dir(workspace)
        .args(["rev-parse", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let head = String::from_utf8(output.stdout).ok()?;
    let head = head.trim();
    if head.is_empty() {
        None
    } else {
        Some(head.to_string())
    }
}

fn current_git_branch(workspace: &Path) -> Option<String> {
    let output = Command::new("git")
        .current_dir(workspace)
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let branch = String::from_utf8(output.stdout).ok()?;
    let branch = branch.trim();
    if branch.is_empty() {
        None
    } else {
        Some(branch.to_string())
    }
}

fn current_git_dirty(workspace: &Path) -> Option<bool> {
    let output = Command::new("git")
        .current_dir(workspace)
        .args(["status", "--porcelain"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    Some(!output.stdout.is_empty())
}

fn is_source_or_test_file(path: &str) -> bool {
    let lower = path.replace('\\', "/").to_lowercase();
    if lower.ends_with(".md") || lower.ends_with(".txt") {
        return false;
    }
    lower.starts_with("src/")
        || lower.starts_with("tests/")
        || lower.contains("/src/")
        || lower.contains("/tests/")
        || [
            ".rs", ".ts", ".tsx", ".js", ".jsx", ".py", ".go", ".java", ".cs", ".cpp", ".c", ".h",
            ".hpp", ".toml", ".json", ".yaml", ".yml",
        ]
        .iter()
        .any(|extension| lower.ends_with(extension))
}

fn test_oracle_change_lacks_authority(event: &Event, file: &str) -> bool {
    is_test_oracle_file(file)
        && test_oracle_change_is_authority_sensitive(event, file)
        && !test_oracle_change_has_authority(event)
}

fn is_test_oracle_file(path: &str) -> bool {
    let lower = path.replace('\\', "/").to_lowercase();
    let file_name = lower.rsplit('/').next().unwrap_or(lower.as_str());
    lower.starts_with("tests/")
        || lower.starts_with("test/")
        || lower.contains("/tests/")
        || lower.contains("/test/")
        || lower.contains("__snapshots__")
        || lower.contains("/fixtures/")
        || lower.contains("/testdata/")
        || file_name.ends_with(".snap")
        || file_name.ends_with(".snapshot")
        || file_name.contains(".spec.")
        || file_name.contains(".test.")
}

fn test_oracle_change_is_authority_sensitive(event: &Event, file: &str) -> bool {
    let text = test_oracle_change_text(event, file);
    is_snapshot_or_fixture_path(file)
        || contains_failure_signal(
            &text,
            &[
                "expected value",
                "expected output",
                "expectation",
                "assertion",
                "assert ",
                "snapshot",
                "fixture",
                "golden",
                "baseline",
                "skip",
                "ignored test",
                "ignore test",
                "delete test",
                "remove test",
                "update expected",
                "refresh snapshot",
                "match implementation",
                "match current output",
                "weaken assertion",
            ],
        )
}

fn is_snapshot_or_fixture_path(path: &str) -> bool {
    let lower = path.replace('\\', "/").to_lowercase();
    let file_name = lower.rsplit('/').next().unwrap_or(lower.as_str());
    lower.contains("__snapshots__")
        || lower.contains("/fixtures/")
        || lower.contains("/testdata/")
        || file_name.ends_with(".snap")
        || file_name.ends_with(".snapshot")
}

fn test_oracle_change_has_authority(event: &Event) -> bool {
    let text = test_oracle_change_text(event, "");
    contains_failure_signal(
        &text,
        &[
            "user-authorized",
            "user authorized",
            "user requested",
            "accepted requirement",
            "authorized requirement",
            "acceptance",
            "requirement",
            "spec authority",
            "product requirement",
            "old oracle invalid",
            "old behavior invalid",
            "changed requirement",
        ],
    )
}

fn test_oracle_change_text(event: &Event, file: &str) -> String {
    let mut text = String::new();
    text.push_str(file);
    text.push('\n');
    if let Some(rationale) = event.rationale.as_deref() {
        text.push_str(rationale);
        text.push('\n');
    }
    if let Some(content) = event.content.as_deref() {
        text.push_str(content);
        text.push('\n');
    }
    if let Some(command) = event.command.as_deref() {
        text.push_str(command);
    }
    text.to_ascii_lowercase()
}

fn is_verification_relevant_file(path: &str, policy: &PolicyConfig) -> bool {
    if !policy.require_verification_after_source_change {
        return false;
    }
    if policy.allow_docs_only_continue_without_tests && is_documentation_file(path) {
        return false;
    }
    is_source_or_test_file(path)
        || (!policy.allow_docs_only_continue_without_tests && is_documentation_file(path))
}

fn is_documentation_file(path: &str) -> bool {
    let lower = path.replace('\\', "/").to_lowercase();
    let file_name = lower.rsplit('/').next().unwrap_or(lower.as_str());
    lower.starts_with("docs/")
        || lower.starts_with("doc/")
        || lower.starts_with("documentation/")
        || lower.contains("/docs/")
        || lower.contains("/doc/")
        || lower.contains("/documentation/")
        || file_name.starts_with("readme")
        || file_name.starts_with("changelog")
        || file_name.starts_with("license")
        || [".md", ".mdx", ".txt", ".rst", ".adoc"]
            .iter()
            .any(|extension| lower.ends_with(extension))
}

pub(crate) fn security_path_user_decision_cause(
    path: &str,
    security: &SecurityConfig,
) -> Option<String> {
    if security.redact_env && is_env_path(path) {
        return Some(format!(
            "security deny path `{}` requires explicit user authorization",
            normalize_path_for_match(path)
        ));
    }

    if security.redact_auth_files && is_auth_file_path(path) {
        return Some(format!(
            "security deny path `{}` requires explicit user authorization",
            normalize_path_for_match(path)
        ));
    }

    if security
        .deny_paths
        .iter()
        .any(|pattern| path_matches_security_pattern(path, pattern))
    {
        return Some(format!(
            "security deny path `{}` requires explicit user authorization",
            normalize_path_for_match(path)
        ));
    }

    if security
        .protected_paths
        .iter()
        .any(|pattern| path_matches_security_pattern(path, pattern))
    {
        return Some(format!(
            "security protected path `{}` requires explicit user authorization",
            normalize_path_for_match(path)
        ));
    }

    None
}

fn is_env_path(path: &str) -> bool {
    let normalized = normalize_path_for_match(path);
    let file_name = normalized.rsplit('/').next().unwrap_or(normalized.as_str());
    file_name == ".env" || file_name.starts_with(".env.")
}

fn is_auth_file_path(path: &str) -> bool {
    let normalized = normalize_path_for_match(path);
    let file_name = normalized.rsplit('/').next().unwrap_or(normalized.as_str());
    file_name == "auth.json" || file_name == "id_rsa" || file_name.ends_with(".pem")
}

fn path_matches_security_pattern(path: &str, pattern: &str) -> bool {
    let path = normalize_path_for_match(path);
    let pattern = normalize_path_for_match(pattern);
    if path == pattern {
        return true;
    }

    if let Some(prefix) = pattern.strip_suffix("/**") {
        return path == prefix || path.starts_with(&format!("{prefix}/"));
    }

    if let Some(suffix) = pattern.strip_prefix("**/") {
        if suffix.contains('*') {
            return path
                .rsplit('/')
                .next()
                .is_some_and(|file_name| wildcard_match(suffix, file_name))
                || wildcard_match(suffix, &path);
        }
        return path == suffix || path.ends_with(&format!("/{suffix}"));
    }

    if pattern.contains('*') {
        return wildcard_match(&pattern, &path);
    }

    false
}

fn wildcard_match(pattern: &str, value: &str) -> bool {
    let pattern = pattern.as_bytes();
    let value = value.as_bytes();
    let mut pattern_index = 0;
    let mut value_index = 0;
    let mut star_index = None;
    let mut value_after_star = 0;

    while value_index < value.len() {
        if pattern_index < pattern.len() && pattern[pattern_index] == value[value_index] {
            pattern_index += 1;
            value_index += 1;
        } else if pattern_index < pattern.len() && pattern[pattern_index] == b'*' {
            star_index = Some(pattern_index);
            pattern_index += 1;
            value_after_star = value_index;
        } else if let Some(star) = star_index {
            pattern_index = star + 1;
            value_after_star += 1;
            value_index = value_after_star;
        } else {
            return false;
        }
    }

    while pattern_index < pattern.len() && pattern[pattern_index] == b'*' {
        pattern_index += 1;
    }

    pattern_index == pattern.len()
}

fn verifier_matches_path(verifier: &VerifierConfig, file: &str) -> bool {
    let file = normalize_path_for_match(file);
    verifier.paths.iter().any(|pattern| {
        let pattern = normalize_path_for_match(pattern);
        file == pattern
            || file.starts_with(pattern.trim_end_matches('/'))
            || pattern.ends_with("/**")
                && file.starts_with(pattern.trim_end_matches("/**").trim_end_matches('/'))
    })
}

fn verifier_matches_acceptance(verifier: &VerifierConfig, criterion: &str) -> bool {
    if verifier
        .acceptance_patterns
        .iter()
        .any(|pattern| acceptance_pattern_matches(pattern, criterion))
    {
        return true;
    }

    let criterion_tokens = meaningful_acceptance_tokens(criterion);
    if criterion_tokens.is_empty() {
        return false;
    }
    let verifier_text = std::iter::once(verifier.id.as_str())
        .chain(std::iter::once(verifier.command.as_str()))
        .chain(verifier.paths.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    let verifier_tokens = text_tokens(&verifier_text);
    let matched = criterion_tokens
        .iter()
        .filter(|token| verifier_tokens.contains(token) || verifier_text.contains(token.as_str()))
        .count();
    matched >= 2 || (matched == 1 && criterion_tokens.len() == 1)
}

fn acceptance_pattern_matches(pattern: &str, criterion: &str) -> bool {
    let pattern = pattern.trim();
    if pattern.is_empty() {
        return false;
    }
    let pattern_text = pattern.to_lowercase();
    let criterion_text = criterion.to_lowercase();
    if criterion_text.contains(&pattern_text) || pattern_text.contains(&criterion_text) {
        return true;
    }

    let criterion_tokens = text_tokens(&criterion_text);
    let pattern_tokens = meaningful_acceptance_tokens(pattern);
    !pattern_tokens.is_empty()
        && pattern_tokens.iter().all(|token| {
            criterion_tokens.contains(token) || criterion_text.contains(token.as_str())
        })
}

fn meaningful_acceptance_tokens(text: &str) -> Vec<String> {
    text_tokens(text)
        .into_iter()
        .filter(|token| token.len() >= 3)
        .filter(|token| !acceptance_stop_word(token))
        .collect()
}

fn text_tokens(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|token| !token.is_empty())
        .map(str::to_string)
        .collect()
}

fn acceptance_stop_word(token: &str) -> bool {
    matches!(
        token,
        "acceptance"
            | "criterion"
            | "criteria"
            | "should"
            | "must"
            | "pass"
            | "passes"
            | "passing"
            | "verify"
            | "verified"
            | "test"
            | "tests"
            | "behavior"
            | "feature"
            | "work"
            | "works"
            | "with"
            | "without"
            | "and"
            | "the"
            | "for"
            | "from"
            | "that"
            | "this"
    )
}

fn normalize_path_for_match(path: &str) -> String {
    normalize_path_text(path)
        .trim_start_matches("./")
        .to_string()
}

pub(crate) fn normalize_blame_path(workspace: &Path, path: &str) -> String {
    let workspace = normalize_path_text(&workspace.display().to_string());
    let normalized = normalize_path_text(path);
    if normalized == workspace {
        return String::new();
    }
    let workspace_prefix = format!("{workspace}/");
    normalized
        .strip_prefix(&workspace_prefix)
        .unwrap_or(&normalized)
        .to_string()
}

pub(crate) fn blame_match_workspace(workspace: &Path) -> PathBuf {
    if workspace.is_absolute() {
        workspace.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|current| current.join(workspace))
            .unwrap_or_else(|_| workspace.to_path_buf())
    }
}

fn normalize_path_text(path: &str) -> String {
    let path = path.replace('\\', "/").to_lowercase();
    let rooted = path.starts_with('/');
    let mut components = Vec::<&str>::new();
    for component in path.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                if components.last().is_some_and(|last| *last != "..") {
                    components.pop();
                } else if !rooted {
                    components.push(component);
                }
            }
            _ => components.push(component),
        }
    }
    let normalized = components.join("/");
    if rooted {
        format!("/{normalized}")
    } else {
        normalized
    }
}

fn is_verification_command(command: &str) -> bool {
    let lower = command.to_lowercase();
    [
        "test",
        "cargo check",
        "cargo build",
        "gradle build",
        "gradlew build",
        "xcodebuild build",
        "flutter build",
        "swift build",
        "npm run build",
        "pnpm build",
        "yarn build",
        "pytest",
        "vitest",
        "jest",
        "tsc",
    ]
    .iter()
    .any(|signal| lower.contains(signal))
}

pub(crate) fn fnv1a64_digest(bytes: &[u8]) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("fnv1a64:{hash:016x}")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrappedCommand {
    pub agent: AgentKind,
    pub session: Option<String>,
    pub command: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrappedCommandResult {
    pub exit_code: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrappedLaunch {
    pub program: String,
    pub args: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum WrappedCommandError {
    #[error("wrapped command is empty")]
    EmptyCommand,
    #[error("agent {agent} requires an external sandbox; generic wrap cannot launch it directly")]
    ExternalSandboxRequired { agent: String },
    #[error("spawn wrapped command {program}: {source}")]
    Spawn {
        program: String,
        #[source]
        source: std::io::Error,
    },
    #[error("read wrapped command output: {0}")]
    Read(#[source] std::io::Error),
    #[error("capture thread panicked")]
    CaptureThreadPanicked,
    #[error("persist wrapped command event: {0}")]
    Persist(#[from] StoreError),
    #[error("wrapped command control loop: {0}")]
    ControlLoop(#[from] AdviceError),
}

pub fn run_wrapped_command(
    wrapped: WrappedCommand,
    store: &mut ProjectStore,
    stdout: impl Write,
    stderr: impl Write,
) -> Result<WrappedCommandResult, WrappedCommandError> {
    if adapter_capabilities_for(wrapped.agent).requires_external_sandbox {
        return Err(WrappedCommandError::ExternalSandboxRequired {
            agent: agent_kind_label(wrapped.agent).into(),
        });
    }

    let lock = acquire_wrapped_command_lock(&wrapped, store)?;
    let result = run_wrapped_command_with_lock(&wrapped, store, stdout, stderr);
    let release_result = store
        .release_worktree_lock(&lock.worktree, &lock.lock_id)
        .map_err(WrappedCommandError::Persist);
    match (result, release_result) {
        (Ok(result), Ok(_)) => Ok(result),
        (Err(error), Ok(_)) => Err(error),
        (Ok(_), Err(error)) => Err(error),
        (Err(error), Err(_release_error)) => Err(error),
    }
}

fn acquire_wrapped_command_lock(
    wrapped: &WrappedCommand,
    store: &mut ProjectStore,
) -> Result<WorktreeLock, WrappedCommandError> {
    let owner_agent = agent_kind_label(wrapped.agent).to_string();
    match store.try_acquire_worktree_lock(&WorktreeLockRequest {
        worktree: store.workspace_root.display().to_string(),
        owner_agent,
        session: wrapped.session.clone(),
    })? {
        WorktreeLockResult::Acquired(lock) => Ok(lock),
        WorktreeLockResult::Conflict { existing } => Err(StoreError::WorktreeLockConflict {
            worktree: existing.worktree,
            existing_owner: existing.owner_agent,
            requested_owner: agent_kind_label(wrapped.agent).into(),
        }
        .into()),
    }
}

fn run_wrapped_command_with_lock(
    wrapped: &WrappedCommand,
    store: &mut ProjectStore,
    mut stdout: impl Write,
    mut stderr: impl Write,
) -> Result<WrappedCommandResult, WrappedCommandError> {
    let launch = prepare_wrapped_launch(&wrapped.command)?;
    let mut child = Command::new(&launch.program)
        .args(&launch.args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|source| WrappedCommandError::Spawn {
            program: launch.program.clone(),
            source,
        })?;

    let child_stdout = child.stdout.take().expect("stdout should be piped");
    let child_stderr = child.stderr.take().expect("stderr should be piped");
    let (sender, receiver) = mpsc::channel();
    let stdout_reader = spawn_capture_reader(CapturedStream::Stdout, child_stdout, sender.clone());
    let stderr_reader = spawn_capture_reader(CapturedStream::Stderr, child_stderr, sender);
    let agent = agent_kind_label(wrapped.agent).to_string();
    let mut monitor = Monitor::new(Config::default());

    for (stream, line) in receiver {
        let event = command_output_event(
            current_utc_timestamp(),
            agent.clone(),
            wrapped.session.clone(),
            stream,
            line.clone(),
        );
        store.append_event(&event)?;
        record_event_outcome_for_latest_advice(store, &event)?;
        let trigger_control_evaluation = wrapped_event_triggers_control_evaluation(store, &event)?;
        for intervention in monitor.ingest(event) {
            store.append_intervention(&intervention)?;
        }
        if trigger_control_evaluation {
            advise_workspace(store.workspace_root.clone())?;
        }
        match stream {
            CapturedStream::Stdout => {
                writeln!(stdout, "{line}").map_err(WrappedCommandError::Read)?;
            }
            CapturedStream::Stderr => {
                writeln!(stderr, "{line}").map_err(WrappedCommandError::Read)?;
            }
        }
    }

    join_capture_reader(stdout_reader)?;
    join_capture_reader(stderr_reader)?;

    let status = child.wait().map_err(WrappedCommandError::Read)?;
    let exit_code = status.code();
    let result_event = command_result_event(
        current_utc_timestamp(),
        agent,
        wrapped.session.clone(),
        wrapped.command.join(" "),
        exit_code,
    );
    store.append_event(&result_event)?;
    record_event_outcome_for_latest_advice(store, &result_event)?;
    if wrapped_event_triggers_control_evaluation(store, &result_event)? {
        advise_workspace(store.workspace_root.clone())?;
    }

    Ok(WrappedCommandResult { exit_code })
}

fn wrapped_event_triggers_control_evaluation(
    store: &ProjectStore,
    event: &Event,
) -> Result<bool, StoreError> {
    if event_is_low_signal_message_delta(event) {
        return Ok(false);
    }
    event_triggers_streaming_control_evaluation(store, event)
}

pub fn prepare_wrapped_launch(command: &[String]) -> Result<WrappedLaunch, WrappedCommandError> {
    let program = command
        .first()
        .ok_or(WrappedCommandError::EmptyCommand)?
        .clone();
    let args = command.iter().skip(1).cloned().collect::<Vec<_>>();
    prepare_wrapped_launch_parts(program, args)
}

#[cfg(windows)]
fn prepare_wrapped_launch_parts(
    program: String,
    args: Vec<String>,
) -> Result<WrappedLaunch, WrappedCommandError> {
    let program = resolve_windows_program(&program).unwrap_or(program);
    let extension = Path::new(&program)
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase);

    match extension.as_deref() {
        Some("cmd" | "bat") => {
            let mut launch_args = vec!["/C".into(), program];
            launch_args.extend(args);
            Ok(WrappedLaunch {
                program: "cmd.exe".into(),
                args: launch_args,
            })
        }
        Some("ps1") => {
            let mut launch_args = vec![
                "-NoProfile".into(),
                "-ExecutionPolicy".into(),
                "Bypass".into(),
                "-File".into(),
                program,
            ];
            launch_args.extend(args);
            Ok(WrappedLaunch {
                program: "powershell.exe".into(),
                args: launch_args,
            })
        }
        _ => Ok(WrappedLaunch { program, args }),
    }
}

#[cfg(windows)]
fn resolve_windows_program(program: &str) -> Option<String> {
    let path = Path::new(program);
    if path.is_absolute() || program.contains('\\') || program.contains('/') {
        return path.exists().then(|| program.to_string());
    }

    let extensions = std::env::var("PATHEXT")
        .ok()
        .map(|value| {
            value
                .split(';')
                .filter(|extension| !extension.is_empty())
                .map(|extension| extension.to_ascii_lowercase())
                .collect::<Vec<_>>()
        })
        .filter(|extensions| !extensions.is_empty())
        .unwrap_or_else(|| {
            vec![
                ".com".into(),
                ".exe".into(),
                ".bat".into(),
                ".cmd".into(),
                ".ps1".into(),
            ]
        });

    let has_extension = Path::new(program).extension().is_some();

    for dir in std::env::split_paths(&std::env::var_os("PATH")?) {
        if has_extension {
            let candidate = dir.join(program);
            if candidate.is_file() {
                return Some(candidate.display().to_string());
            }
        } else {
            for extension in &extensions {
                let candidate = dir.join(format!("{program}{extension}"));
                if candidate.is_file() {
                    return Some(candidate.display().to_string());
                }
            }
            let candidate = dir.join(program);
            if candidate.is_file() {
                return Some(candidate.display().to_string());
            }
        }
    }

    None
}

#[cfg(not(windows))]
fn prepare_wrapped_launch_parts(
    program: String,
    args: Vec<String>,
) -> Result<WrappedLaunch, WrappedCommandError> {
    Ok(WrappedLaunch { program, args })
}

fn spawn_capture_reader<R: Read + Send + 'static>(
    stream: CapturedStream,
    reader: R,
    sender: mpsc::Sender<(CapturedStream, String)>,
) -> thread::JoinHandle<Result<(), std::io::Error>> {
    thread::spawn(move || {
        // Read raw bytes and decode lossily rather than using `.lines()`, which
        // hard-errors on the first non-UTF-8 byte. Agent CLIs on Windows often
        // emit output in the OEM/ANSI code page, so strict UTF-8 would drop the
        // entire capture stream the moment a non-ASCII byte appeared.
        let mut buffered = BufReader::new(reader);
        let mut line = Vec::new();
        loop {
            line.clear();
            let read = buffered.read_until(b'\n', &mut line)?;
            if read == 0 {
                break;
            }
            while matches!(line.last(), Some(b'\n' | b'\r')) {
                line.pop();
            }
            let decoded = decode_console_line(&line);
            if sender.send((stream, decoded)).is_err() {
                break;
            }
        }
        Ok(())
    })
}

/// Decode one captured output line. Agent CLIs increasingly emit UTF-8, so try
/// that first; only fall back to the platform console code page when the bytes
/// are not valid UTF-8. This keeps modern UTF-8 output pristine while still
/// rendering legacy code-page output (e.g. GBK/CP936 on a Chinese Windows)
/// correctly instead of as replacement characters.
fn decode_console_line(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_string(),
        Err(_) => decode_console_code_page(bytes),
    }
}

#[cfg(windows)]
fn decode_console_code_page(bytes: &[u8]) -> String {
    use windows_sys::Win32::Globalization::{GetOEMCP, MultiByteToWideChar};

    if bytes.is_empty() {
        return String::new();
    }
    // Console subprocesses write in the OEM code page; decode against it.
    let code_page = unsafe { GetOEMCP() };
    let wide_len = unsafe {
        MultiByteToWideChar(
            code_page,
            0,
            bytes.as_ptr(),
            bytes.len() as i32,
            std::ptr::null_mut(),
            0,
        )
    };
    if wide_len <= 0 {
        return String::from_utf8_lossy(bytes).into_owned();
    }
    let mut wide = vec![0u16; wide_len as usize];
    let written = unsafe {
        MultiByteToWideChar(
            code_page,
            0,
            bytes.as_ptr(),
            bytes.len() as i32,
            wide.as_mut_ptr(),
            wide_len,
        )
    };
    if written <= 0 {
        return String::from_utf8_lossy(bytes).into_owned();
    }
    wide.truncate(written as usize);
    String::from_utf16_lossy(&wide)
}

#[cfg(not(windows))]
fn decode_console_code_page(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn join_capture_reader(
    handle: thread::JoinHandle<Result<(), std::io::Error>>,
) -> Result<(), WrappedCommandError> {
    handle
        .join()
        .map_err(|_| WrappedCommandError::CaptureThreadPanicked)?
        .map_err(WrappedCommandError::Read)
}

#[derive(Debug)]
pub struct Monitor {
    config: Config,
    service_failures: HashMap<String, usize>,
    robustness_scores: HashMap<String, i32>,
    design: Vec<DesignEntry>,
    trace: Vec<TraceEntry>,
}

impl Monitor {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            service_failures: HashMap::new(),
            robustness_scores: HashMap::new(),
            design: Vec::new(),
            trace: Vec::new(),
        }
    }

    pub fn ingest(&mut self, event: Event) -> Vec<Intervention> {
        match event.kind {
            EventKind::DesignThought => {
                if let Some(content) = event.content.clone().filter(|content| !content.is_empty()) {
                    self.design.push(DesignEntry {
                        time: event.time.clone(),
                        agent: event.agent.clone(),
                        session: event.session.clone(),
                        content,
                    });
                }
            }
            EventKind::FileChange | EventKind::RepoDiff => {
                if let Some(file) = event.file.clone().filter(|file| !file.is_empty()) {
                    self.trace.push(TraceEntry {
                        time: event.time.clone(),
                        event_id: event.event_id.clone(),
                        agent: event.agent.clone(),
                        provider: event.provider.clone(),
                        model: event.model.clone(),
                        session: event.session.clone(),
                        file,
                        line: event.line,
                        line_end: event.line_end,
                        rationale: event.rationale.clone(),
                        related_event_ids: event.related_event_ids.clone(),
                        requirement_ids: event.requirement_ids.clone(),
                    });
                }
            }
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
            | EventKind::InterventionResult => {}
        }

        let mut interventions = Vec::new();
        let content = event.content.as_deref().unwrap_or_default();
        if self.config.open_work && looks_like_premature_stop(content) {
            self.adjust_robustness(&event.agent, -2);
            interventions.push(Intervention {
                kind: InterventionKind::PrematureStop,
                action: Action::ContinueWorking,
                agent: Some(event.agent.clone()),
                reason: "remaining work is open; continue obvious next steps instead of asking the user to decide"
                    .into(),
            });
        }

        if looks_like_service_failure(content) {
            self.adjust_robustness(&event.agent, -1);
            let failures = self
                .service_failures
                .entry(event.agent.clone())
                .and_modify(|count| *count += 1)
                .or_insert(1);

            if *failures <= self.config.retry_limit {
                interventions.push(Intervention {
                    kind: InterventionKind::ServiceFailure,
                    action: Action::RetrySameAgent,
                    agent: Some(event.agent.clone()),
                    reason: "transient service failure; retry the same agent before switching"
                        .into(),
                });
            } else if let Some(fallback_agent) = self.next_fallback(&event.agent) {
                interventions.push(Intervention {
                    kind: InterventionKind::ServiceFailure,
                    action: Action::SwitchAgent,
                    agent: Some(fallback_agent),
                    reason: "retry limit exceeded; switch to a fallback agent".into(),
                });
            } else {
                interventions.push(Intervention {
                    kind: InterventionKind::ServiceFailure,
                    action: Action::RetrySameAgent,
                    agent: Some(event.agent.clone()),
                    reason: "retry limit exceeded but no fallback agent is available; keep the same agent under monitor supervision".into(),
                });
            }
        } else if event_can_clear_service_failure(&event, content) {
            self.service_failures.remove(&event.agent);
        }

        if looks_like_forgetting_design_memory(content) {
            self.adjust_robustness(&event.agent, -3);
            interventions.push(Intervention {
                kind: InterventionKind::AgentDegraded,
                action: Action::SpawnFreshAgent,
                agent: Some(event.agent.clone()),
                reason: "agent appears to have lost design memory; spawn a fresh agent with durable project context"
                    .into(),
            });
        }

        interventions
    }

    pub fn design_record(&self) -> &[DesignEntry] {
        &self.design
    }

    pub fn trace(&self) -> &[TraceEntry] {
        &self.trace
    }

    pub fn robustness_score(&self, agent: &str) -> i32 {
        self.robustness_scores
            .get(agent)
            .copied()
            .unwrap_or_default()
    }

    fn adjust_robustness(&mut self, agent: &str, delta: i32) {
        self.robustness_scores
            .entry(agent.into())
            .and_modify(|score| *score += delta)
            .or_insert(delta);
    }

    fn next_fallback(&self, current: &str) -> Option<String> {
        self.config
            .fallback_agents
            .iter()
            .find(|agent| agent.as_str() != current)
            .cloned()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum JsonlError {
    #[error("line {line}: decode event: {source}")]
    Decode {
        line: usize,
        #[source]
        source: serde_json::Error,
    },
    #[error("load adapter ingest project config: {0}")]
    ProjectConfig(#[from] ProjectConfigError),
    #[error("adapter {agent} is disabled in project config; refusing adapter ingest")]
    AdapterDisabled { agent: String },
    #[error("line {line}: encode intervention: {source}")]
    Encode {
        line: usize,
        #[source]
        source: serde_json::Error,
    },
    #[error("read input: {0}")]
    Read(#[from] std::io::Error),
    #[error("line {line}: persist monitor event: {source}")]
    Persist {
        line: usize,
        #[source]
        source: StoreError,
    },
    #[error("line {line}: run streaming control advice: {source}")]
    ControlLoop {
        line: usize,
        #[source]
        source: AdviceError,
    },
}

pub fn run_jsonl(
    input: impl Read,
    mut output: impl Write,
    config: Config,
) -> Result<(), JsonlError> {
    run_jsonl_inner(input, &mut output, config, None)
}

pub fn run_jsonl_with_store(
    input: impl Read,
    mut output: impl Write,
    config: Config,
    store: &mut ProjectStore,
) -> Result<(), JsonlError> {
    let project_config = ProjectConfig::load(store.root())?;
    let config = filter_disabled_fallback_agents(config, &project_config);
    run_jsonl_inner(input, &mut output, config, Some(store))
}

pub fn run_adapter_jsonl_with_store(
    input: impl Read,
    mut output: impl Write,
    options: AdapterIngestOptions,
    store: &mut ProjectStore,
) -> Result<(), JsonlError> {
    let project_config = ProjectConfig::load(store.root())?;
    ensure_adapter_ingest_enabled(options.adapter, &project_config)?;
    let options = AdapterIngestOptions {
        config: filter_disabled_fallback_agents(options.config, &project_config),
        ..options
    };
    run_adapter_jsonl_inner(input, &mut output, options, Some(store))
}

fn ensure_adapter_ingest_enabled(
    adapter: AgentKind,
    project_config: &ProjectConfig,
) -> Result<(), JsonlError> {
    let capabilities = adapter_capabilities_for_config(adapter, &project_config.adapters);
    if !capabilities.enabled {
        return Err(JsonlError::AdapterDisabled {
            agent: agent_kind_label(adapter).into(),
        });
    }
    Ok(())
}

fn filter_disabled_fallback_agents(mut config: Config, project_config: &ProjectConfig) -> Config {
    config.fallback_agents.retain(|agent| {
        AgentKind::from_str(agent).is_ok_and(|kind| {
            let capabilities = adapter_capabilities_for_config(kind, &project_config.adapters);
            adapter_capability_allows_writable_handoff(&capabilities)
        })
    });
    config
}

fn run_adapter_jsonl_inner(
    input: impl Read,
    mut output: impl Write,
    options: AdapterIngestOptions,
    mut store: Option<&mut ProjectStore>,
) -> Result<(), JsonlError> {
    let reader = BufReader::new(input);
    let mut monitor = Monitor::new(options.config.clone());

    for (index, line) in reader.lines().enumerate() {
        let line_number = index + 1;
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }

        let raw: serde_json::Value = match serde_json::from_str(&line) {
            Ok(raw) => raw,
            Err(_) => {
                let mut event = adapter_ingest_warning_event(
                    options.adapter,
                    options.session.as_deref(),
                    line_number,
                );
                stamp_adapter_line_provenance(std::slice::from_mut(&mut event), line_number, &line);
                process_monitor_event(
                    event,
                    line_number,
                    &mut monitor,
                    store.as_deref_mut(),
                    &mut output,
                )?;
                continue;
            }
        };

        let event_name = adapter_event_name(&raw);
        let mut events =
            normalize_adapter_events(options.adapter, options.session.as_deref(), &raw);
        if events.is_empty() {
            if let Some(event_name) = event_name {
                let mut event = adapter_ignored_event(
                    options.adapter,
                    options.session.as_deref(),
                    line_number,
                    &event_name,
                );
                stamp_adapter_line_provenance(std::slice::from_mut(&mut event), line_number, &line);
                process_monitor_event(
                    event,
                    line_number,
                    &mut monitor,
                    store.as_deref_mut(),
                    &mut output,
                )?;
            }
            continue;
        }

        stamp_adapter_line_provenance(&mut events, line_number, &line);
        process_monitor_events(
            events,
            line_number,
            &mut monitor,
            store.as_deref_mut(),
            &mut output,
        )?;
    }

    Ok(())
}

fn stamp_adapter_line_provenance(events: &mut [Event], line_number: usize, raw_line: &str) {
    let source_hash = fnv1a64_digest(raw_line.as_bytes());
    for event in events {
        fill_empty_string(&mut event.source_type, "adapter_jsonl".into());
        if event.source_offset.is_none() {
            event.source_offset = Some(line_number as u64);
        }
        fill_empty_string(&mut event.source_hash, source_hash.clone());
    }
}

fn run_jsonl_inner(
    input: impl Read,
    mut output: impl Write,
    config: Config,
    mut store: Option<&mut ProjectStore>,
) -> Result<(), JsonlError> {
    let reader = BufReader::new(input);
    let mut monitor = Monitor::new(config);

    for (index, line) in reader.lines().enumerate() {
        let line_number = index + 1;
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }

        let event: Event = serde_json::from_str(&line).map_err(|source| JsonlError::Decode {
            line: line_number,
            source,
        })?;

        process_monitor_event(
            event,
            line_number,
            &mut monitor,
            store.as_deref_mut(),
            &mut output,
        )?;
    }

    Ok(())
}

fn process_monitor_event(
    event: Event,
    line_number: usize,
    monitor: &mut Monitor,
    store: Option<&mut ProjectStore>,
    output: &mut impl Write,
) -> Result<(), JsonlError> {
    process_monitor_events(vec![event], line_number, monitor, store, output)
}

fn process_monitor_events(
    events: Vec<Event>,
    line_number: usize,
    monitor: &mut Monitor,
    mut store: Option<&mut ProjectStore>,
    output: &mut impl Write,
) -> Result<(), JsonlError> {
    let mut trigger_control_evaluation = false;

    for event in events {
        let event = if let Some(store) = store.as_deref_mut() {
            store
                .append_event_and_return(&event)
                .map_err(|source| JsonlError::Persist {
                    line: line_number,
                    source,
                })?
        } else {
            event
        };

        if let Some(store) = store.as_deref_mut() {
            record_event_outcome_for_latest_advice(store, &event).map_err(|source| {
                JsonlError::Persist {
                    line: line_number,
                    source,
                }
            })?;
            if let Some(entry) = design_entry_from_event(&event) {
                store
                    .append_design(&entry)
                    .map_err(|source| JsonlError::Persist {
                        line: line_number,
                        source,
                    })?;
            }
            if let Some(entry) = trace_entry_from_event(&event) {
                store
                    .append_trace(&entry)
                    .map_err(|source| JsonlError::Persist {
                        line: line_number,
                        source,
                    })?;
            }
        }

        let low_signal_message_delta = event_is_low_signal_message_delta(&event);
        let event_triggers_control_evaluation = if low_signal_message_delta {
            false
        } else if let Some(store) = store.as_deref() {
            event_triggers_streaming_control_evaluation(store, &event).map_err(|source| {
                JsonlError::Persist {
                    line: line_number,
                    source,
                }
            })?
        } else {
            false
        };
        trigger_control_evaluation |= event_triggers_control_evaluation;

        let suppress_legacy_interventions = store.is_some() && event_triggers_control_evaluation;
        let interventions = if low_signal_message_delta {
            Vec::new()
        } else {
            let interventions = monitor.ingest(event);
            if suppress_legacy_interventions {
                Vec::new()
            } else {
                interventions
            }
        };

        for intervention in interventions {
            if let Some(store) = store.as_deref_mut() {
                store
                    .append_intervention(&intervention)
                    .map_err(|source| JsonlError::Persist {
                        line: line_number,
                        source,
                    })?;
            }
            serde_json::to_writer(&mut *output, &intervention).map_err(|source| {
                JsonlError::Encode {
                    line: line_number,
                    source,
                }
            })?;
            writeln!(output)?;
        }
    }

    if trigger_control_evaluation && let Some(store) = store {
        let workspace = store.workspace_root.clone();
        advise_workspace(workspace).map_err(|source| JsonlError::ControlLoop {
            line: line_number,
            source,
        })?;
    }

    Ok(())
}

fn event_is_low_signal_message_delta(event: &Event) -> bool {
    event.kind == EventKind::CommandOutput
        && event
            .content
            .as_deref()
            .is_some_and(|content| content.trim_start().starts_with("message delta:"))
}

fn event_triggers_streaming_control_evaluation(
    store: &ProjectStore,
    event: &Event,
) -> Result<bool, StoreError> {
    if event_is_change_like(event) || event_is_verification_result(event) {
        return Ok(true);
    }
    if event_is_content_control_trigger(event) {
        return Ok(true);
    }
    if event_is_lifecycle_control_trigger(event) {
        return Ok(true);
    }

    let events = read_all_jsonl::<Event>(&store.root.join("events.jsonl"))?;
    Ok(repeated_command_failure_crossed_threshold(&events, event)
        || repeated_service_failure_crossed_threshold(&events, event)
        || repeated_permission_lifecycle_crossed_threshold(&events, event))
}

fn event_is_content_control_trigger(event: &Event) -> bool {
    let Some(content) = event.content.as_deref() else {
        return false;
    };
    looks_like_premature_stop(content)
        || looks_like_completion_claim(content)
        || looks_like_unverified_completion(content)
}

fn repeated_command_failure_crossed_threshold(events: &[Event], current: &Event) -> bool {
    let Some(current_command) = repeated_command_failure_signature(current) else {
        return false;
    };
    let mut count = 0;
    for event in events.iter().filter(|event| event.agent == current.agent) {
        if event.kind == EventKind::CommandResult && event.exit_code == Some(0) {
            count = 0;
            continue;
        }
        if repeated_command_failure_signature(event).as_deref() == Some(current_command.as_str()) {
            count += 1;
        }
    }
    count == 3
}

fn repeated_command_failure_signature(event: &Event) -> Option<String> {
    if event.kind != EventKind::CommandResult || event.exit_code.is_none_or(|code| code == 0) {
        return None;
    }
    let command = event.command.as_deref().map(normalize_command_signature)?;
    if is_verification_command(&command) {
        return None;
    }
    Some(command)
}

fn repeated_service_failure_crossed_threshold(events: &[Event], current: &Event) -> bool {
    repeated_content_failure_count(
        events,
        current,
        looks_like_service_failure,
        event_can_clear_service_failure,
    )
    .is_some_and(|count| count == 3)
}

fn repeated_permission_lifecycle_crossed_threshold(events: &[Event], current: &Event) -> bool {
    repeated_content_failure_count(
        events,
        current,
        permission_lifecycle_is_blocked,
        event_can_clear_service_failure,
    )
    .is_some_and(|count| count == 2)
}

fn repeated_content_failure_count(
    events: &[Event],
    current: &Event,
    looks_like_failure: fn(&str) -> bool,
    can_clear_failure: fn(&Event, &str) -> bool,
) -> Option<usize> {
    if !current.content.as_deref().is_some_and(looks_like_failure) {
        return None;
    }

    let mut count = 0;
    for event in events.iter().filter(|event| event.agent == current.agent) {
        let content = event.content.as_deref().unwrap_or_default();
        if looks_like_failure(content) {
            count += 1;
        } else if can_clear_failure(event, content) {
            count = 0;
        }
    }
    Some(count)
}

fn event_is_lifecycle_control_trigger(event: &Event) -> bool {
    let content = event.content.as_deref().unwrap_or_default();
    matches!(event.kind, EventKind::AgentHealth)
        && (looks_like_session_idle_or_stop(content)
            || looks_like_session_error(content)
            || looks_like_context_compaction(content)
            || looks_like_forgetting_design_memory(content))
        || matches!(event.kind, EventKind::InterventionResult)
            && permission_lifecycle_is_blocked(content)
}

fn design_entry_from_event(event: &Event) -> Option<DesignEntry> {
    if event.kind != EventKind::DesignThought {
        return None;
    }
    let content = event
        .content
        .as_ref()
        .filter(|content| !content.is_empty())?
        .clone();
    Some(DesignEntry {
        time: event.time.clone(),
        agent: event.agent.clone(),
        session: event.session.clone(),
        content,
    })
}

fn trace_entry_from_event(event: &Event) -> Option<TraceEntry> {
    if !event_is_change_like(event) {
        return None;
    }
    let file = event.file.as_ref().filter(|file| !file.is_empty())?.clone();
    Some(TraceEntry {
        time: event.time.clone(),
        event_id: event.event_id.clone(),
        agent: event.agent.clone(),
        provider: event.provider.clone(),
        model: event.model.clone(),
        session: event.session.clone(),
        file,
        line: event.line,
        line_end: event.line_end,
        rationale: event.rationale.clone(),
        related_event_ids: event.related_event_ids.clone(),
        requirement_ids: event.requirement_ids.clone(),
    })
}

struct JsonlSummary<T> {
    count: usize,
    recent: Vec<T>,
}

struct JsonlLine {
    line_number: usize,
    line: String,
    terminated: bool,
}

fn read_jsonl_summary<T, F>(
    path: &Path,
    recent_limit: usize,
    mut observe: F,
) -> Result<JsonlSummary<T>, StoreError>
where
    T: DeserializeOwned,
    F: FnMut(&T),
{
    if !path.exists() {
        return Ok(JsonlSummary {
            count: 0,
            recent: Vec::new(),
        });
    }

    let mut count = 0;
    let mut recent = Vec::new();
    let lines = read_non_empty_jsonl_lines(path)?;
    let last_line_number = lines.last().map(|line| line.line_number);
    for JsonlLine {
        line_number,
        line,
        terminated,
    } in lines
    {
        let value = match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(source)
                if Some(line_number) == last_line_number && source.is_eof() && !terminated =>
            {
                break;
            }
            Err(source) => {
                return Err(StoreError::Decode {
                    path: path.to_path_buf(),
                    line: line_number,
                    source,
                });
            }
        };
        observe(&value);
        count += 1;
        if recent_limit > 0 {
            if recent.len() == recent_limit {
                recent.remove(0);
            }
            recent.push(value);
        }
    }

    Ok(JsonlSummary { count, recent })
}

pub(crate) fn read_all_jsonl<T>(path: &Path) -> Result<Vec<T>, StoreError>
where
    T: DeserializeOwned,
{
    if !path.exists() {
        return Ok(Vec::new());
    }

    let mut values = Vec::new();
    let lines = read_non_empty_jsonl_lines(path)?;
    let last_line_number = lines.last().map(|line| line.line_number);
    for JsonlLine {
        line_number,
        line,
        terminated,
    } in lines
    {
        match serde_json::from_str(&line) {
            Ok(value) => values.push(value),
            Err(source)
                if Some(line_number) == last_line_number && source.is_eof() && !terminated =>
            {
                break;
            }
            Err(source) => {
                return Err(StoreError::Decode {
                    path: path.to_path_buf(),
                    line: line_number,
                    source,
                });
            }
        }
    }
    Ok(values)
}

fn read_non_empty_jsonl_lines(path: &Path) -> Result<Vec<JsonlLine>, StoreError> {
    let file = fs::File::open(path).map_err(|source| StoreError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    let mut reader = BufReader::new(file);
    let mut lines = Vec::new();
    let mut line_number = 0;

    loop {
        let mut line = String::new();
        let bytes = reader
            .read_line(&mut line)
            .map_err(|source| StoreError::Read {
                path: path.to_path_buf(),
                source,
            })?;
        if bytes == 0 {
            break;
        }

        line_number += 1;
        let terminated = line.ends_with('\n');
        let line = line.trim_end_matches(['\r', '\n']).to_string();
        if line.trim().is_empty() {
            continue;
        }
        lines.push(JsonlLine {
            line_number,
            line,
            terminated,
        });
    }

    Ok(lines)
}

fn count_jsonl_lines(path: &Path) -> Result<usize, StoreError> {
    if !path.exists() {
        return Ok(0);
    }
    let lines = read_non_empty_jsonl_lines(path)?;
    let last_line_number = lines.last().map(|line| line.line_number);
    Ok(lines
        .into_iter()
        .filter(|line| Some(line.line_number) != last_line_number || line.terminated)
        .count())
}

fn dashboard_advisor_status(store_root: &Path) -> DashboardAdvisorStatus {
    match ProjectConfig::load(store_root) {
        Ok(config) => dashboard_advisor_status_from_config(store_root, &config),
        Err(error) => DashboardAdvisorStatus {
            enabled: false,
            credential_kind: DashboardAdvisorCredentialKind::InvalidProfile,
            severity: DashboardSeverity::Critical,
            message: format!("advisor config unreadable: {error}"),
            ..DashboardAdvisorStatus::default()
        },
    }
}

fn dashboard_advisor_status_from_config(
    store_root: &Path,
    config: &ProjectConfig,
) -> DashboardAdvisorStatus {
    let provider = &config.advisor.provider;
    let mut status = DashboardAdvisorStatus {
        enabled: config.advisor.enabled,
        credential_source: provider.credential_source,
        credential_kind: DashboardAdvisorCredentialKind::None,
        uses_dedicated_profile: provider.credential_source == AdvisorCredentialSource::CodingPlan,
        endpoint: provider.endpoint.clone(),
        endpoint_host: advisor_endpoint_host(&provider.endpoint),
        model: provider.model.clone(),
        credential_file: provider.credential_file.clone(),
        severity: DashboardSeverity::Healthy,
        message: "advisor disabled".into(),
    };

    if !config.advisor.enabled {
        return status;
    }

    if provider.endpoint.trim().is_empty() {
        status.severity = DashboardSeverity::Warning;
        status.message = "advisor endpoint is not configured".into();
        return status;
    }
    if provider.model.trim().is_empty() {
        status.severity = DashboardSeverity::Warning;
        status.message = "advisor model is not configured".into();
        return status;
    }

    match provider.credential_source {
        AdvisorCredentialSource::Env => {
            status.credential_kind = DashboardAdvisorCredentialKind::Env;
            status.message = format!(
                "advisor uses environment credential {}",
                provider.api_key_env
            );
        }
        AdvisorCredentialSource::CodingPlan => {
            let (kind, severity, message) = dashboard_coding_plan_credential_status(
                store_root,
                provider.credential_file.as_deref(),
                &provider.endpoint,
            );
            status.credential_kind = kind;
            status.severity = severity;
            status.message = message;
        }
        AdvisorCredentialSource::ClaudePlan => {
            status.credential_kind = DashboardAdvisorCredentialKind::UnsupportedSource;
            status.severity = DashboardSeverity::Critical;
            status.message =
                "advisor credential_source claude_plan is unsupported; use coding_plan".into();
        }
    }

    status
}

fn dashboard_coding_plan_credential_status(
    store_root: &Path,
    credential_file: Option<&str>,
    endpoint: &str,
) -> (DashboardAdvisorCredentialKind, DashboardSeverity, String) {
    let Some(credential_file) = credential_file
        .map(str::trim)
        .filter(|file| !file.is_empty())
    else {
        return (
            DashboardAdvisorCredentialKind::MissingProfile,
            DashboardSeverity::Critical,
            "coding-plan advisor credential profile is not configured".into(),
        );
    };
    let path = advisor_dashboard_credential_path(credential_file, store_root);
    if let Some(cli_dir) = local_cli_auth_profile_dir_for_dashboard(&path) {
        return (
            DashboardAdvisorCredentialKind::UnsupportedSource,
            DashboardSeverity::Critical,
            format!(
                "advisor credential profile points at local CLI auth directory {cli_dir}; use a dedicated coding-plan profile"
            ),
        );
    }
    let content = match fs::read_to_string(&path) {
        Ok(content) => content,
        Err(_) => {
            return (
                DashboardAdvisorCredentialKind::MissingProfile,
                DashboardSeverity::Critical,
                "coding-plan advisor credential profile is missing or unreadable".into(),
            );
        }
    };
    let value: serde_json::Value = match serde_json::from_str(&content) {
        Ok(value) => value,
        Err(_) => {
            return (
                DashboardAdvisorCredentialKind::InvalidProfile,
                DashboardSeverity::Critical,
                "coding-plan advisor credential profile is not valid JSON".into(),
            );
        }
    };
    let Some(token) = coding_plan_dashboard_token(&value) else {
        return (
            DashboardAdvisorCredentialKind::InvalidProfile,
            DashboardSeverity::Critical,
            "coding-plan advisor credential profile has no supported advisor token".into(),
        );
    };

    if looks_like_jwt_bearer_token(&token) {
        if is_public_openai_endpoint(endpoint) {
            return (
                DashboardAdvisorCredentialKind::JwtBearer,
                DashboardSeverity::Critical,
                "JWT/OAuth-style coding-plan credential is incompatible with api.openai.com; configure a dedicated provider/proxy endpoint".into(),
            );
        }
        return (
            DashboardAdvisorCredentialKind::JwtBearer,
            DashboardSeverity::Healthy,
            "dedicated coding-plan advisor endpoint configured".into(),
        );
    }

    (
        DashboardAdvisorCredentialKind::ApiKey,
        DashboardSeverity::Healthy,
        "dedicated coding-plan advisor API-key profile configured".into(),
    )
}

fn advisor_dashboard_credential_path(credential_file: &str, store_root: &Path) -> PathBuf {
    let path = Path::new(credential_file);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        store_root.join(path)
    }
}

fn coding_plan_dashboard_token(value: &serde_json::Value) -> Option<String> {
    credential_string_at_any_json_pointer(
        value,
        &[
            "/OPENAI_API_KEY",
            "/api_key",
            "/apiKey",
            "/credentials/OPENAI_API_KEY",
            "/credentials/api_key",
            "/tokens/access_token",
            "/access_token",
        ],
    )
}

fn credential_string_at_any_json_pointer(
    value: &serde_json::Value,
    pointers: &[&str],
) -> Option<String> {
    pointers
        .iter()
        .find_map(|pointer| value.pointer(pointer).and_then(serde_json::Value::as_str))
        .map(|token| token.trim().to_string())
        .filter(|token| !token.is_empty())
}

fn looks_like_jwt_bearer_token(token: &str) -> bool {
    let mut parts = token.trim().split('.');
    let Some(header) = parts.next() else {
        return false;
    };
    let Some(payload) = parts.next() else {
        return false;
    };
    let Some(signature) = parts.next() else {
        return false;
    };

    parts.next().is_none()
        && header.starts_with("eyJ")
        && !payload.is_empty()
        && !signature.is_empty()
}

fn is_public_openai_endpoint(endpoint: &str) -> bool {
    advisor_endpoint_host(endpoint)
        .as_deref()
        .is_some_and(|host| host.eq_ignore_ascii_case("api.openai.com"))
}

fn advisor_endpoint_host(endpoint: &str) -> Option<String> {
    let endpoint = endpoint.trim();
    let rest = endpoint
        .strip_prefix("https://")
        .or_else(|| endpoint.strip_prefix("http://"))?;
    let host_port = rest.split('/').next()?.trim();
    if host_port.is_empty() {
        return None;
    }
    let host = host_port
        .rsplit_once(':')
        .map(|(host, _)| host)
        .unwrap_or(host_port)
        .trim();
    (!host.is_empty()).then(|| host.to_string())
}

fn local_cli_auth_profile_dir_for_dashboard(path: &Path) -> Option<&'static str> {
    path.components().find_map(|component| {
        let std::path::Component::Normal(value) = component else {
            return None;
        };
        match value.to_string_lossy().to_ascii_lowercase().as_str() {
            ".codex" => Some(".codex"),
            ".claude" => Some(".claude"),
            _ => None,
        }
    })
}

fn dashboard_severity(
    agent_health: &[AgentHealth],
    intervention_count: usize,
    rows: &[DashboardRow],
) -> DashboardSeverity {
    let worst_score = agent_health
        .iter()
        .map(|health| health.score)
        .min()
        .unwrap_or_default();
    let row_has_critical = rows
        .iter()
        .any(|row| row.severity == DashboardSeverity::Critical);
    let row_has_warning = rows
        .iter()
        .any(|row| row.severity == DashboardSeverity::Warning);

    if worst_score <= -6 || row_has_critical {
        DashboardSeverity::Critical
    } else if worst_score < 0 || intervention_count > 0 || row_has_warning {
        DashboardSeverity::Warning
    } else {
        DashboardSeverity::Healthy
    }
}

fn max_dashboard_severity(left: DashboardSeverity, right: DashboardSeverity) -> DashboardSeverity {
    if dashboard_severity_rank(left) >= dashboard_severity_rank(right) {
        left
    } else {
        right
    }
}

fn dashboard_severity_rank(severity: DashboardSeverity) -> u8 {
    match severity {
        DashboardSeverity::Healthy => 0,
        DashboardSeverity::Warning => 1,
        DashboardSeverity::Critical => 2,
    }
}

fn agent_sessions(
    scores: &HashMap<String, i32>,
    event_counts: &HashMap<String, usize>,
    intervention_counts: &HashMap<String, usize>,
    last_seen: &HashMap<String, String>,
    now: Option<&str>,
    stale_after_secs: Option<i64>,
) -> Vec<AgentSession> {
    let now_epoch = now.and_then(parse_utc_seconds);
    let mut sessions = scores
        .iter()
        .map(|(agent, score)| {
            let interventions = intervention_counts.get(agent).copied().unwrap_or_default();
            let last_seen_text = last_seen.get(agent).cloned();
            let stale = match (
                now_epoch,
                stale_after_secs,
                last_seen_text.as_deref().and_then(parse_utc_seconds),
            ) {
                (Some(now), Some(stale_after), Some(last_seen)) => now - last_seen > stale_after,
                _ => false,
            };
            AgentSession {
                agent: agent.clone(),
                status: if *score < 0 || interventions > 0 {
                    AgentActivityStatus::Degraded
                } else if stale {
                    AgentActivityStatus::Stale
                } else {
                    AgentActivityStatus::Active
                },
                score: *score,
                events: event_counts.get(agent).copied().unwrap_or_default(),
                interventions,
                last_seen: last_seen_text,
            }
        })
        .collect::<Vec<_>>();
    sessions.sort_by(|left, right| {
        status_rank(left.status)
            .cmp(&status_rank(right.status))
            .then_with(|| left.score.cmp(&right.score))
            .then_with(|| left.agent.cmp(&right.agent))
    });
    sessions
}

fn status_rank(status: AgentActivityStatus) -> u8 {
    match status {
        AgentActivityStatus::Degraded => 0,
        AgentActivityStatus::Stale => 1,
        AgentActivityStatus::Active => 2,
    }
}

fn parse_utc_seconds(value: &str) -> Option<i64> {
    let date_time = value.strip_suffix('Z')?;
    let (date, time) = date_time.split_once('T')?;
    let mut date_parts = date.split('-');
    let year = date_parts.next()?.parse::<i32>().ok()?;
    let month = date_parts.next()?.parse::<u32>().ok()?;
    let day = date_parts.next()?.parse::<u32>().ok()?;
    let mut time_parts = time.split(':');
    let hour = time_parts.next()?.parse::<u32>().ok()?;
    let minute = time_parts.next()?.parse::<u32>().ok()?;
    let second = time_parts.next()?.parse::<u32>().ok()?;
    Some(
        days_from_civil(year, month, day) * 86_400
            + i64::from(hour) * 3_600
            + i64::from(minute) * 60
            + i64::from(second),
    )
}

fn current_utc_timestamp() -> Option<String> {
    current_utc_seconds().map(format_utc_seconds)
}

fn current_utc_seconds() -> Option<i64> {
    Some(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_secs() as i64,
    )
}

fn format_utc_seconds(seconds: i64) -> String {
    let days = seconds.div_euclid(86_400);
    let seconds_of_day = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = seconds_of_day / 3_600;
    let minute = seconds_of_day % 3_600 / 60;
    let second = seconds_of_day % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

fn civil_from_days(days: i64) -> (i32, u32, u32) {
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

fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let year = year - if month <= 2 { 1 } else { 0 };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let month = month as i32;
    let day = day as i32;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    i64::from(era * 146_097 + doe - 719_468)
}

struct DashboardRowsInput<'a> {
    events: &'a [Event],
    interventions: &'a [Intervention],
    verifier_runs: &'a [VerifierRun],
    probe_runs: &'a [ProbeRun],
    repo_hunks: &'a [RepoHunkHistoryEntry],
    repo_hunk_files: &'a [RepoHunkFileSummary],
    requirements: &'a [RequirementNode],
    requirement_proofs: &'a [RequirementProofStep],
    dev_history: &'a [DevHistoryReport],
    decision_trails: &'a [DecisionTrail],
    worktree_lock_events: &'a [WorktreeLockEvent],
}

fn dashboard_rows(input: DashboardRowsInput<'_>) -> Vec<DashboardRow> {
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

fn worktree_lock_event_summary(event: &WorktreeLockEvent) -> String {
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

fn worktree_lock_event_severity(event: &WorktreeLockEvent) -> DashboardSeverity {
    match event.kind.as_str() {
        "conflict" | "expired" => DashboardSeverity::Warning,
        _ => DashboardSeverity::Healthy,
    }
}

fn decision_trail_summary(trail: &DecisionTrail) -> String {
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

fn decision_trail_row_severity(trail: &DecisionTrail) -> DashboardSeverity {
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

fn dispatch_status_label(status: DispatchStatus) -> &'static str {
    match status {
        DispatchStatus::OutboxWritten => "outbox_written",
        DispatchStatus::SuppressedDuplicate => "suppressed_duplicate",
        DispatchStatus::Failed => "failed",
    }
}

#[derive(Serialize)]
struct DevHistoryFindingDashboardDetail<'a> {
    workspace: &'a str,
    generated_at: &'a str,
    sources: &'a [DevHistorySourceReport],
    finding: &'a DevHistoryFinding,
}

fn dev_history_finding_detail(report: &DevHistoryReport, finding: &DevHistoryFinding) -> String {
    serde_json::to_string_pretty(&DevHistoryFindingDashboardDetail {
        workspace: &report.workspace,
        generated_at: &report.generated_at,
        sources: &report.sources,
        finding,
    })
    .unwrap_or_else(|_| String::new())
}

fn dev_history_finding_summary(finding: &DevHistoryFinding) -> String {
    format!("{}: {}", finding.kind, finding.summary)
}

fn dev_history_finding_severity(finding: &DevHistoryFinding) -> DashboardSeverity {
    match finding.severity.as_str() {
        "critical" => DashboardSeverity::Critical,
        "warning" => DashboardSeverity::Warning,
        _ => DashboardSeverity::Healthy,
    }
}

#[derive(Serialize)]
struct RequirementDashboardDetail<'a> {
    requirement: &'a RequirementNode,
    proofs: Vec<&'a RequirementProofStep>,
}

fn requirement_dashboard_detail(
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

fn latest_requirement_proof<'a>(
    requirement: &RequirementNode,
    requirement_proofs: &'a [RequirementProofStep],
) -> Option<&'a RequirementProofStep> {
    requirement_proofs
        .iter()
        .find(|proof| proof.requirement_id == requirement.requirement_id)
}

fn requirement_summary(
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

fn requirement_row_severity(
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

fn requirement_status_severity(status: AcceptanceCoverageStatus) -> DashboardSeverity {
    match status {
        AcceptanceCoverageStatus::Covered => DashboardSeverity::Healthy,
        AcceptanceCoverageStatus::Failed
        | AcceptanceCoverageStatus::Stale
        | AcceptanceCoverageStatus::Unverified
        | AcceptanceCoverageStatus::Unmapped => DashboardSeverity::Warning,
    }
}

fn acceptance_coverage_status_label(status: AcceptanceCoverageStatus) -> &'static str {
    match status {
        AcceptanceCoverageStatus::Covered => "covered",
        AcceptanceCoverageStatus::Stale => "stale",
        AcceptanceCoverageStatus::Failed => "failed",
        AcceptanceCoverageStatus::Unverified => "unverified",
        AcceptanceCoverageStatus::Unmapped => "unmapped",
    }
}

fn repo_hunk_history_summary(hunk: &RepoHunkHistoryEntry) -> String {
    format!(
        "{} hunk {}: {}, {} matching trace(s)",
        hunk.path,
        hunk.hunk_index,
        repo_trace_status_label(hunk.trace_status),
        hunk.matching_trace_count
    )
}

fn repo_hunk_file_summary(file: &RepoHunkFileSummary) -> String {
    format!(
        "{}: {} hunk(s), worst {}, latest {}, {} matching trace(s)",
        file.path,
        file.entry_count,
        repo_trace_status_label(file.worst_trace_status),
        repo_trace_status_label(file.latest_trace_status),
        file.matching_trace_count
    )
}

fn repo_trace_status_label(status: RepoTraceStatus) -> &'static str {
    match status {
        RepoTraceStatus::Traced => "traced",
        RepoTraceStatus::MissingRationale => "missing rationale",
        RepoTraceStatus::Untraced => "untraced",
    }
}

fn verifier_run_summary(run: &VerifierRun) -> String {
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

fn probe_run_summary(run: &ProbeRun) -> String {
    format!(
        "{}: {}, {}",
        probe_spec_kind_label(&run.probe),
        outcome_status_label(run.status),
        run.summary
    )
}

fn probe_run_severity(run: &ProbeRun) -> DashboardSeverity {
    match run.status {
        OutcomeStatus::Succeeded => DashboardSeverity::Healthy,
        OutcomeStatus::Failed | OutcomeStatus::Unknown => DashboardSeverity::Warning,
    }
}

fn probe_spec_kind_label(probe: &ProbeSpec) -> String {
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

fn outcome_status_label(status: OutcomeStatus) -> &'static str {
    match status {
        OutcomeStatus::Succeeded => "succeeded",
        OutcomeStatus::Failed => "failed",
        OutcomeStatus::Unknown => "unknown",
    }
}

fn verification_failure_class_label(failure_class: VerificationFailureClass) -> &'static str {
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

fn event_summary(event: &Event) -> String {
    if let Some(file) = &event.file {
        if let Some(rationale) = &event.rationale {
            return format!("{file}: {rationale}");
        }
        return file.clone();
    }
    event.content.clone().unwrap_or_default()
}

fn parse_row_kind(value: &str) -> Result<DashboardRowKind, DashboardFilterError> {
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

fn parse_dashboard_severity(value: &str) -> Result<DashboardSeverity, DashboardFilterError> {
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

fn looks_like_premature_stop(content: &str) -> bool {
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

fn event_asks_routine_user_question(event: &Event, content: &str) -> bool {
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

fn looks_like_unverified_completion(content: &str) -> bool {
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

fn truncate_evidence(content: &str) -> String {
    const MAX_CHARS: usize = 240;
    let trimmed = content.trim();
    if trimmed.chars().count() <= MAX_CHARS {
        return trimmed.to_string();
    }
    let prefix = trimmed.chars().take(MAX_CHARS - 1).collect::<String>();
    format!("{}…", prefix.trim_end())
}

fn classify_command_failure_layer(event: &Event, content: &str) -> FailureLayer {
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

fn classify_service_failure_layer(content: &str) -> Option<FailureLayer> {
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

fn failure_layer_text(event: &Event, content: &str) -> String {
    let mut text = String::new();
    if let Some(command) = event.command.as_deref() {
        text.push_str(command);
        text.push('\n');
    }
    text.push_str(content);
    text.to_lowercase()
}

fn contains_failure_signal(text: &str, signals: &[&str]) -> bool {
    signals.iter().any(|signal| text.contains(signal))
}

fn contains_5xx_status(text: &str) -> bool {
    ["500", "502", "503", "504"]
        .iter()
        .any(|status| text.contains(status))
}

fn looks_like_service_failure(content: &str) -> bool {
    classify_service_failure_layer(content).is_some()
}

fn looks_like_permission_denial(content: &str) -> bool {
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

fn looks_like_permission_request(content: &str) -> bool {
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

fn permission_lifecycle_is_blocked(content: &str) -> bool {
    looks_like_permission_denial(content) || looks_like_permission_request(content)
}

fn looks_like_forgetting_design_memory(content: &str) -> bool {
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

fn looks_like_context_compaction(content: &str) -> bool {
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

fn looks_like_session_error(content: &str) -> bool {
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

fn looks_like_session_idle_or_stop(content: &str) -> bool {
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

fn user_decision_cause(content: &str) -> Option<&'static str> {
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

fn user_decision_cause_for_event(event: &Event, content: &str) -> Option<&'static str> {
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

fn shell_delete_tokens_are_recursive_force(tokens: &[&str]) -> bool {
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

fn git_clean_token_has_force_flag(token: &str) -> bool {
    token == "--force"
        || token == "-f"
        || (token.starts_with('-')
            && !token.starts_with("--")
            && token.chars().skip(1).any(|ch| ch == 'f'))
}

#[cfg(test)]
mod console_decode_tests {
    use super::{AdvisorClientError, current_id_fragment, decode_console_line, post_json_http};
    use std::collections::HashSet;

    #[test]
    fn valid_utf8_is_passed_through_unchanged() {
        assert_eq!(
            decode_console_line("版本 build ok".as_bytes()),
            "版本 build ok"
        );
    }

    #[test]
    fn empty_input_decodes_to_empty_string() {
        assert_eq!(decode_console_line(&[]), "");
    }

    #[test]
    fn invalid_utf8_does_not_panic_and_yields_text() {
        // 0x80 is not valid standalone UTF-8; decoding must fall back gracefully
        // (OEM code page on Windows, lossy elsewhere) without crashing.
        let decoded = decode_console_line(&[b'o', b'k', 0x80]);
        assert!(decoded.starts_with("ok"));
    }

    #[test]
    fn https_advisor_endpoint_is_not_rejected_before_transport() {
        let error = post_json_http(
            "https://127.0.0.1:9/v1/chat/completions",
            "test-key",
            "{}",
            1,
        )
        .expect_err("unreachable local endpoint should fail");

        assert!(!matches!(error, AdvisorClientError::InvalidEndpoint(_)));
        assert!(!error.to_string().contains("https transport"));
    }

    #[test]
    fn generated_id_fragments_are_unique_inside_one_millisecond_window() {
        let ids = (0..100)
            .map(|_| current_id_fragment())
            .collect::<HashSet<_>>();

        assert_eq!(ids.len(), 100);
    }

    #[test]
    fn generated_id_fragments_include_process_id() {
        let id = current_id_fragment();

        assert!(id.contains(&std::process::id().to_string()));
    }
}
