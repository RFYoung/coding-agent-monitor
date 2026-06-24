use crate::{
    AgentActivityStatus, AgentKind, DashboardSeverity, DashboardSnapshot, DesignEntry, Event,
    EventKind, ProjectStore, RepoTraceStatus, StoreError, agent_kind_label, current_utc_timestamp,
    load_repo_audit, looks_like_forgetting_design_memory, looks_like_premature_stop,
    looks_like_unverified_completion, truncate_evidence,
};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use sysinfo::System;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentReviewStatus {
    Ok,
    Watch,
    Intervene,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentReviewAction {
    Continue,
    ContinueWorking,
    ForceVerification,
    SpawnJudgeAgent,
    SpawnFreshAgent,
    InstallTelemetry,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentReviewFinding {
    pub severity: DashboardSeverity,
    pub category: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    pub evidence: String,
    pub recommended_action: AgentReviewAction,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentReviewReport {
    pub workspace: String,
    pub status: AgentReviewStatus,
    pub findings: Vec<AgentReviewFinding>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunningProcess {
    pub pid: u32,
    pub name: String,
    pub command: String,
    pub cwd: Option<PathBuf>,
}

impl RunningProcess {
    pub fn new(pid: u32, name: impl Into<String>, command: impl Into<String>) -> Self {
        Self {
            pid,
            name: name.into(),
            command: command.into(),
            cwd: None,
        }
    }

    pub fn with_cwd(mut self, cwd: PathBuf) -> Self {
        self.cwd = Some(cwd);
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunningAgent {
    pub pid: u32,
    pub agent: AgentKind,
    pub process_name: String,
    pub cwd: Option<PathBuf>,
}

impl RunningAgent {
    pub fn new(pid: u32, agent: AgentKind, process_name: impl Into<String>) -> Self {
        Self {
            pid,
            agent,
            process_name: process_name.into(),
            cwd: None,
        }
    }

    pub fn with_cwd(mut self, cwd: Option<PathBuf>) -> Self {
        self.cwd = cwd;
        self
    }
}

pub fn detect_running_agents(processes: &[RunningProcess]) -> Vec<RunningAgent> {
    processes
        .iter()
        .filter_map(|process| {
            classify_process(process).map(|agent| {
                RunningAgent::new(process.pid, agent, process.name.clone())
                    .with_cwd(process.cwd.clone())
            })
        })
        .collect()
}

pub fn detect_running_agents_from_system() -> Vec<RunningAgent> {
    detect_running_agents(&running_processes_from_system())
}

pub fn judge_snapshot(
    workspace: impl AsRef<Path>,
    snapshot: &DashboardSnapshot,
    running_agents: &[RunningAgent],
) -> AgentReviewReport {
    let mut findings = Vec::new();
    let workspace = workspace.as_ref();

    for session in &snapshot.agent_sessions {
        if session.status == AgentActivityStatus::Degraded {
            findings.push(AgentReviewFinding {
                severity: DashboardSeverity::Critical,
                category: "agent_degraded".into(),
                agent: Some(session.agent.clone()),
                evidence: format!(
                    "{} has score {} and {} interventions",
                    session.agent, session.score, session.interventions
                ),
                recommended_action: AgentReviewAction::SpawnFreshAgent,
            });
        } else if session.status == AgentActivityStatus::Stale {
            findings.push(AgentReviewFinding {
                severity: DashboardSeverity::Warning,
                category: "agent_stale".into(),
                agent: Some(session.agent.clone()),
                evidence: format!(
                    "{} last seen {}",
                    session.agent,
                    session.last_seen.as_deref().unwrap_or("unknown")
                ),
                recommended_action: AgentReviewAction::ContinueWorking,
            });
        }
    }

    for event in &snapshot.recent_events {
        let content = event.content.as_deref().unwrap_or_default();
        if looks_like_unverified_completion(content) {
            findings.push(AgentReviewFinding {
                severity: DashboardSeverity::Critical,
                category: "unverified_completion".into(),
                agent: Some(event.agent.clone()),
                evidence: truncate_evidence(content),
                recommended_action: AgentReviewAction::ForceVerification,
            });
        } else if looks_like_premature_stop(content) {
            findings.push(AgentReviewFinding {
                severity: DashboardSeverity::Warning,
                category: "premature_stop".into(),
                agent: Some(event.agent.clone()),
                evidence: truncate_evidence(content),
                recommended_action: AgentReviewAction::ContinueWorking,
            });
        } else if looks_like_forgetting_design_memory(content) {
            findings.push(AgentReviewFinding {
                severity: DashboardSeverity::Critical,
                category: "lost_design_memory".into(),
                agent: Some(event.agent.clone()),
                evidence: truncate_evidence(content),
                recommended_action: AgentReviewAction::SpawnFreshAgent,
            });
        }
    }

    if let Ok(repo_audit) = load_repo_audit(workspace) {
        for change in repo_audit
            .changes
            .iter()
            .filter(|change| change.trace_status == RepoTraceStatus::Untraced)
            .take(3)
        {
            findings.push(AgentReviewFinding {
                severity: DashboardSeverity::Critical,
                category: "suspicious_untraced_change".into(),
                agent: None,
                evidence: format!("{} has dirty git hunks without trace evidence", change.path),
                recommended_action: AgentReviewAction::SpawnJudgeAgent,
            });
        }
        for change in repo_audit
            .changes
            .iter()
            .filter(|change| change.trace_status == RepoTraceStatus::MissingRationale)
            .take(3)
        {
            findings.push(AgentReviewFinding {
                severity: DashboardSeverity::Warning,
                category: "suspicious_missing_rationale".into(),
                agent: None,
                evidence: format!(
                    "{} has trace evidence without a durable rationale",
                    change.path
                ),
                recommended_action: AgentReviewAction::SpawnJudgeAgent,
            });
        }
    }

    if snapshot.event_count == 0 && snapshot.intervention_count == 0 {
        for agent in running_agents {
            findings.push(AgentReviewFinding {
                severity: DashboardSeverity::Warning,
                category: "telemetry_gap".into(),
                agent: Some(agent_kind_label(agent.agent).into()),
                evidence: format!(
                    "running pid {} has no monitor telemetry in this workspace",
                    agent.pid
                ),
                recommended_action: AgentReviewAction::InstallTelemetry,
            });
        }
    }

    let status = if findings
        .iter()
        .any(|finding| finding.severity == DashboardSeverity::Critical)
    {
        AgentReviewStatus::Intervene
    } else if findings.is_empty() {
        AgentReviewStatus::Ok
    } else {
        AgentReviewStatus::Watch
    };

    AgentReviewReport {
        workspace: workspace.display().to_string(),
        status,
        findings,
    }
}

fn running_processes_from_system() -> Vec<RunningProcess> {
    let mut system = System::new_all();
    system.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
    system
        .processes()
        .iter()
        .map(|(pid, sys_process)| {
            let command = sys_process
                .cmd()
                .iter()
                .map(|part| part.to_string_lossy())
                .collect::<Vec<_>>()
                .join(" ");
            let process =
                RunningProcess::new(pid.as_u32(), sys_process.name().to_string_lossy(), command);
            if let Some(cwd) = sys_process.cwd() {
                process.with_cwd(cwd.to_path_buf())
            } else {
                process
            }
        })
        .collect()
}

#[derive(Debug, thiserror::Error)]
pub enum DemoWorkspaceError {
    #[error("demo workspace already exists and is not empty: {path}")]
    NotEmpty { path: PathBuf },
    #[error("create demo workspace {path}: {source}")]
    CreateDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("write demo file {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("open demo monitor store: {0}")]
    Store(#[from] StoreError),
}

pub fn create_demo_workspace(workspace: impl AsRef<Path>) -> Result<(), DemoWorkspaceError> {
    let workspace = workspace.as_ref();
    if workspace.exists()
        && workspace
            .read_dir()
            .map_err(|source| DemoWorkspaceError::CreateDir {
                path: workspace.to_path_buf(),
                source,
            })?
            .next()
            .is_some()
    {
        return Err(DemoWorkspaceError::NotEmpty {
            path: workspace.to_path_buf(),
        });
    }

    fs::create_dir_all(workspace).map_err(|source| DemoWorkspaceError::CreateDir {
        path: workspace.to_path_buf(),
        source,
    })?;
    let readme = workspace.join("README.md");
    fs::write(
        &readme,
        "# Coding Agent Monitor Demo\n\nThis workspace is disposable. Use it to test wrapping, judging, and UI monitoring.\n",
    )
    .map_err(|source| DemoWorkspaceError::Write {
        path: readme,
        source,
    })?;

    let mut store = ProjectStore::open(workspace)?;
    store.append_event(&Event {
        time: current_utc_timestamp(),
        agent: "codex".into(),
        kind: EventKind::ModelMessage,
        content: Some("Implementation complete. I did not run tests.".into()),
        ..Event::default()
    })?;
    store.append_event(&Event {
        time: current_utc_timestamp(),
        agent: "codex".into(),
        kind: EventKind::DesignThought,
        content: Some(
            "The monitor must judge agents externally and preserve durable design memory.".into(),
        ),
        ..Event::default()
    })?;
    store.append_design(&DesignEntry {
        time: current_utc_timestamp(),
        agent: "codex".into(),
        session: Some("demo".into()),
        content: "The monitor must judge agents externally and preserve durable design memory."
            .into(),
    })?;
    Ok(())
}

fn classify_process(process: &RunningProcess) -> Option<AgentKind> {
    let haystack = format!("{} {}", process.name, process.command).to_lowercase();
    if contains_agent_token(&haystack, &["claude-code", "@anthropic-ai/claude-code"]) {
        Some(AgentKind::ClaudeCode)
    } else if contains_agent_token(&haystack, &["opencode", "open-code"]) {
        Some(AgentKind::OpenCode)
    } else if contains_agent_token(&haystack, &["pi_agent", " pi ", " pi.exe", "\\pi.exe"]) {
        Some(AgentKind::Pi)
    } else if contains_agent_token(&haystack, &["codex", "openai-codex"]) {
        Some(AgentKind::Codex)
    } else {
        None
    }
}

fn contains_agent_token(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}
