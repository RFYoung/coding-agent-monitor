//! CLI entrypoint for `agent-monitor`.
//!
//! Argument parsing lives in `cli_parse`; this file owns command dispatch,
//! process-facing hook responses, and external judge invocation.

use coding_agent_monitor::{
    AcceptanceCoverageStatus, Action, AdapterHookDecision, AdapterHookResponse,
    AdapterIngestOptions, AdvisorCredentialSource, AdvisorEndpointConfigUpdate, AgentKind,
    AgentReviewAction, AgentReviewReport, BlameQuery, CalibrationQuery, Config, ControlActionKind,
    DashboardSnapshot, DevHistoryAnalysisOptions, DevHistoryRawExportOptions, Event, EventKind,
    InstallMode, Intervention, InterventionKind, LocalAgentConfigImportOptions, MemorySource,
    ProjectConfig, ProjectStore, RepoHunkHistoryQuery, RequirementGraphQuery, RuntimeAuthConfig,
    RuntimeAuthStyle, TraceEntry, VerificationScope, VerifierConfig, WrappedCommand,
    adapter_capabilities_for_config, adapter_hook_response, advise_workspace, agent_kind_label,
    analyze_local_dev_history, create_demo_workspace, detect_running_agents_from_system,
    export_raw_dev_history, handoff_workspace, import_coding_plan_advisor_credentials,
    import_local_agent_configs, injection_plan_for_workspace, install_agent_injection,
    judge_snapshot, load_blame_report, load_calibration_report, load_completion_certificate_report,
    load_decision_trails, load_repo_hunk_history, load_requirement_graph, normalize_adapter_event,
    prepare_wrapped_launch, promote_memory_candidate, record_repo_audit_history,
    record_trace_entry, run_adapter_jsonl_with_store, run_jsonl, run_jsonl_with_store, run_probe,
    run_verifier, run_wrapped_command, write_adapter_runtime_auth_config,
    write_advisor_endpoint_config, write_verifier_config,
};
use std::env;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

// Keep parsing pure and testable; command arms below should only perform I/O
// and call library entrypoints.
mod cli_parse;
use cli_parse::*;

const EXTERNAL_JUDGE_PROMPT: &str = "You are a read-only external judge for a coding-agent supervisor.\n\
Output exactly one line: decision=<continue | force_verification | handoff | restart>; evidence=<ids/files/tests>; risk=<short reason>.\n\
Judge the control loop, not code style. Prioritize unverified completion, stale verification, lost durable intent, unsafe edits, and telemetry gaps.\n\
Treat intended-environment validation as first-class: web may use browser/Playwright, but mobile/native/system/ML need platform runtime evidence.\n\
Prefer force_verification over handoff when verification is stale or missing.\n\
Do not propose broad refactors, new product scope, or implementation patches.";

fn main() -> ExitCode {
    let command = match parse_cli(env::args().skip(1)) {
        Ok(command) => command,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::from(2);
        }
    };

    match command {
        CliCommand::Monitor { config, workspace } => {
            let result = if let Some(workspace) = workspace {
                match ProjectStore::open(&workspace) {
                    Ok(mut store) => run_jsonl_with_store(
                        std::io::stdin(),
                        std::io::stdout(),
                        config,
                        &mut store,
                    ),
                    Err(error) => {
                        eprintln!("{error}");
                        return ExitCode::from(1);
                    }
                }
            } else {
                run_jsonl(std::io::stdin(), std::io::stdout(), config)
            };

            if let Err(error) = result {
                eprintln!("{error}");
                return ExitCode::from(1);
            }
        }
        CliCommand::InstallInjection {
            agent,
            workspace,
            mode,
        } => match install_agent_injection(&workspace, agent, mode) {
            Ok(paths) => {
                for path in paths {
                    println!("{}", path.display());
                }
            }
            Err(error) => {
                eprintln!("{error}");
                return ExitCode::from(1);
            }
        },
        CliCommand::InjectRunning {
            workspace,
            mode,
            apply,
        } => {
            let agents = detect_running_agents_from_system();
            if agents.is_empty() {
                eprintln!("no running coding agents detected");
                return ExitCode::from(1);
            }

            for agent in agents {
                let Some(target_workspace) = injection_workspace_for(&agent, workspace.as_ref())
                else {
                    eprintln!(
                        "pid {} {:?}: skipped because process cwd is unavailable or is an agent support directory; pass --workspace=<path>",
                        agent.pid, agent.agent
                    );
                    continue;
                };
                let plan = match injection_plan_for_workspace(&target_workspace, agent.agent) {
                    Ok(plan) => plan,
                    Err(error) => {
                        eprintln!("pid {} {:?}: {error}", agent.pid, agent.agent);
                        continue;
                    }
                };
                if !apply {
                    for file in &plan.files {
                        println!(
                            "pid {} {:?} would write {}",
                            agent.pid,
                            agent.agent,
                            target_workspace.join(&file.relative_path).display()
                        );
                    }
                    continue;
                }
                match install_agent_injection(&target_workspace, agent.agent, mode) {
                    Ok(paths) => {
                        for path in paths {
                            println!("pid {} {:?} -> {}", agent.pid, agent.agent, path.display());
                        }
                    }
                    Err(error) => {
                        eprintln!("pid {} {:?}: {error}", agent.pid, agent.agent);
                    }
                }
            }
        }
        CliCommand::Wrap {
            agent,
            workspace,
            session,
            command,
        } => {
            let mut store = match ProjectStore::open(&workspace) {
                Ok(store) => store,
                Err(error) => {
                    eprintln!("{error}");
                    return ExitCode::from(1);
                }
            };
            let result = run_wrapped_command(
                WrappedCommand {
                    agent,
                    session,
                    command,
                },
                &mut store,
                std::io::stdout(),
                std::io::stderr(),
            );
            match result {
                Ok(result) => {
                    if let Some(code) = result.exit_code {
                        return ExitCode::from(u8::try_from(code).unwrap_or(1));
                    }
                    return ExitCode::from(1);
                }
                Err(error) => {
                    eprintln!("{error}");
                    return ExitCode::from(1);
                }
            }
        }
        CliCommand::Judge {
            workspace,
            write_intervention,
            external_command,
        } => {
            let mut store = match ProjectStore::open(&workspace) {
                Ok(store) => store,
                Err(error) => {
                    eprintln!("{error}");
                    return ExitCode::from(1);
                }
            };
            let snapshot = match DashboardSnapshot::load(store.root(), 500) {
                Ok(snapshot) => snapshot,
                Err(error) => {
                    eprintln!("{error}");
                    return ExitCode::from(1);
                }
            };
            let running_agents = detect_running_agents_from_system();
            let report = judge_snapshot(&workspace, &snapshot, &running_agents);
            if write_intervention {
                for intervention in interventions_from_review(&report) {
                    if let Err(error) = store.append_intervention(&intervention) {
                        eprintln!("{error}");
                        return ExitCode::from(1);
                    }
                }
            }
            if external_command.is_empty() {
                if let Err(error) = serde_json::to_writer_pretty(std::io::stdout(), &report) {
                    eprintln!("encode judge report: {error}");
                    return ExitCode::from(1);
                }
                println!();
            } else if let Err(error) = run_external_judge(&external_command, &report) {
                eprintln!("{error}");
                return ExitCode::from(1);
            }
        }
        CliCommand::Ingest {
            adapter,
            workspace,
            session,
            config,
        } => {
            let mut store = match ProjectStore::open(&workspace) {
                Ok(store) => store,
                Err(error) => {
                    eprintln!("{error}");
                    return ExitCode::from(1);
                }
            };
            if let Err(error) = run_adapter_jsonl_with_store(
                std::io::stdin(),
                std::io::stdout(),
                AdapterIngestOptions {
                    adapter,
                    session,
                    config,
                },
                &mut store,
            ) {
                eprintln!("{error}");
                return ExitCode::from(1);
            }
        }
        CliCommand::HookResponse {
            adapter,
            workspace,
            session,
            format,
        } => {
            if let Err(error) = run_hook_response(
                std::io::stdin(),
                std::io::stdout(),
                adapter,
                &workspace,
                session.as_deref(),
                format,
            ) {
                eprintln!("{error}");
                return ExitCode::from(1);
            }
        }
        CliCommand::ConfigAdvisor { workspace, update } => {
            match write_advisor_endpoint_config(&workspace, update) {
                Ok(config) => {
                    if let Err(error) = serde_json::to_writer_pretty(std::io::stdout(), &config) {
                        eprintln!("encode project config: {error}");
                        return ExitCode::from(1);
                    }
                    println!();
                }
                Err(error) => {
                    eprintln!("{error}");
                    return ExitCode::from(1);
                }
            }
        }
        CliCommand::ConfigVerifier {
            workspace,
            verifier,
        } => match write_verifier_config(&workspace, verifier) {
            Ok(config) => {
                if let Err(error) = serde_json::to_writer_pretty(std::io::stdout(), &config) {
                    eprintln!("encode project config: {error}");
                    return ExitCode::from(1);
                }
                println!();
            }
            Err(error) => {
                eprintln!("{error}");
                return ExitCode::from(1);
            }
        },
        CliCommand::ConfigRuntimeAuth {
            workspace,
            agent,
            runtime_auth,
        } => match write_adapter_runtime_auth_config(&workspace, agent, runtime_auth) {
            Ok(config) => {
                if let Err(error) = serde_json::to_writer_pretty(std::io::stdout(), &config) {
                    eprintln!("encode project config: {error}");
                    return ExitCode::from(1);
                }
                println!();
            }
            Err(error) => {
                eprintln!("{error}");
                return ExitCode::from(1);
            }
        },
        CliCommand::ConfigImportLocal {
            workspace,
            home,
            options,
        } => match import_local_agent_configs(&workspace, &home, options) {
            Ok(config) => {
                if let Err(error) = serde_json::to_writer_pretty(std::io::stdout(), &config) {
                    eprintln!("encode project config: {error}");
                    return ExitCode::from(1);
                }
                println!();
            }
            Err(error) => {
                eprintln!("{error}");
                return ExitCode::from(1);
            }
        },
        CliCommand::ConfigImportCodingPlanCredentials {
            workspace,
            source_file,
            endpoint,
            model,
        } => match import_coding_plan_advisor_credentials(
            &workspace,
            &source_file,
            endpoint.as_deref(),
            model.as_deref(),
        ) {
            Ok(config) => {
                if let Err(error) = serde_json::to_writer_pretty(std::io::stdout(), &config) {
                    eprintln!("encode project config: {error}");
                    return ExitCode::from(1);
                }
                println!();
            }
            Err(error) => {
                eprintln!("{error}");
                return ExitCode::from(1);
            }
        },
        CliCommand::Advise { workspace } => match advise_workspace(&workspace) {
            Ok(advice) => {
                if let Err(error) = serde_json::to_writer_pretty(std::io::stdout(), &advice) {
                    eprintln!("encode advice: {error}");
                    return ExitCode::from(1);
                }
                println!();
            }
            Err(error) => {
                eprintln!("{error}");
                return ExitCode::from(1);
            }
        },
        CliCommand::Trail { workspace } => {
            let store = match ProjectStore::open(&workspace) {
                Ok(store) => store,
                Err(error) => {
                    eprintln!("{error}");
                    return ExitCode::from(1);
                }
            };
            match load_decision_trails(store.root()) {
                Ok(trails) => {
                    if let Err(error) = serde_json::to_writer_pretty(std::io::stdout(), &trails) {
                        eprintln!("encode decision trails: {error}");
                        return ExitCode::from(1);
                    }
                    println!();
                }
                Err(error) => {
                    eprintln!("{error}");
                    return ExitCode::from(1);
                }
            }
        }
        CliCommand::DevHistory {
            workspace,
            codex_sessions_root,
            claude_projects_root,
            top_limit,
            write,
            export_raw,
            raw_output_root,
            raw_package_name,
        } => {
            if export_raw {
                match export_raw_dev_history(DevHistoryRawExportOptions {
                    workspace: workspace.clone(),
                    codex_sessions_root: Some(codex_sessions_root),
                    claude_projects_root: Some(claude_projects_root),
                    output_root: raw_output_root
                        .unwrap_or_else(|| workspace.join(".agent-monitor").join("exports")),
                    package_name: raw_package_name,
                }) {
                    Ok(report) => {
                        if let Err(error) = serde_json::to_writer_pretty(std::io::stdout(), &report)
                        {
                            eprintln!("encode raw dev history export report: {error}");
                            return ExitCode::from(1);
                        }
                        println!();
                    }
                    Err(error) => {
                        eprintln!("{error}");
                        return ExitCode::from(1);
                    }
                }
            } else {
                match analyze_local_dev_history(DevHistoryAnalysisOptions {
                    workspace: workspace.clone(),
                    codex_sessions_root: Some(codex_sessions_root),
                    claude_projects_root: Some(claude_projects_root),
                    top_limit,
                }) {
                    Ok(report) => {
                        if write {
                            let mut store = match ProjectStore::open(&workspace) {
                                Ok(store) => store,
                                Err(error) => {
                                    eprintln!("{error}");
                                    return ExitCode::from(1);
                                }
                            };
                            if let Err(error) = store.append_dev_history_report(&report) {
                                eprintln!("{error}");
                                return ExitCode::from(1);
                            }
                        }
                        if let Err(error) = serde_json::to_writer_pretty(std::io::stdout(), &report)
                        {
                            eprintln!("encode dev history report: {error}");
                            return ExitCode::from(1);
                        }
                        println!();
                    }
                    Err(error) => {
                        eprintln!("{error}");
                        return ExitCode::from(1);
                    }
                }
            }
        }
        CliCommand::Calibration { workspace, query } => {
            match load_calibration_report(&workspace, query) {
                Ok(report) => {
                    if let Err(error) = serde_json::to_writer_pretty(std::io::stdout(), &report) {
                        eprintln!("encode calibration report: {error}");
                        return ExitCode::from(1);
                    }
                    println!();
                }
                Err(error) => {
                    eprintln!("{error}");
                    return ExitCode::from(1);
                }
            }
        }
        CliCommand::Blame {
            workspace,
            file,
            line,
            limit,
        } => match load_blame_report(&workspace, BlameQuery { file, line, limit }) {
            Ok(report) => {
                if let Err(error) = serde_json::to_writer_pretty(std::io::stdout(), &report) {
                    eprintln!("encode blame report: {error}");
                    return ExitCode::from(1);
                }
                println!();
            }
            Err(error) => {
                eprintln!("{error}");
                return ExitCode::from(1);
            }
        },
        CliCommand::RepoHunks {
            workspace,
            file,
            line,
            limit,
        } => match load_repo_hunk_history(&workspace, RepoHunkHistoryQuery { file, line, limit }) {
            Ok(report) => {
                if let Err(error) = serde_json::to_writer_pretty(std::io::stdout(), &report) {
                    eprintln!("encode repo hunk history: {error}");
                    return ExitCode::from(1);
                }
                println!();
            }
            Err(error) => {
                eprintln!("{error}");
                return ExitCode::from(1);
            }
        },
        CliCommand::Requirements { workspace, query } => {
            match load_requirement_graph(&workspace, query) {
                Ok(report) => {
                    if let Err(error) = serde_json::to_writer_pretty(std::io::stdout(), &report) {
                        eprintln!("encode requirements report: {error}");
                        return ExitCode::from(1);
                    }
                    println!();
                }
                Err(error) => {
                    eprintln!("{error}");
                    return ExitCode::from(1);
                }
            }
        }
        CliCommand::CompletionCertificate { workspace, query } => {
            match load_completion_certificate_report(&workspace, query) {
                Ok(report) => {
                    if let Err(error) = serde_json::to_writer_pretty(std::io::stdout(), &report) {
                        eprintln!("encode completion certificate report: {error}");
                        return ExitCode::from(1);
                    }
                    println!();
                }
                Err(error) => {
                    eprintln!("{error}");
                    return ExitCode::from(1);
                }
            }
        }
        CliCommand::Trace { workspace, entry } => match record_trace_entry(&workspace, entry) {
            Ok(entry) => {
                if let Err(error) = serde_json::to_writer_pretty(std::io::stdout(), &entry) {
                    eprintln!("encode trace entry: {error}");
                    return ExitCode::from(1);
                }
                println!();
            }
            Err(error) => {
                eprintln!("{error}");
                return ExitCode::from(1);
            }
        },
        CliCommand::Handoff { workspace, agent } => match handoff_workspace(&workspace, agent) {
            Ok(handoff) => {
                if let Err(error) = serde_json::to_writer_pretty(std::io::stdout(), &handoff) {
                    eprintln!("encode handoff: {error}");
                    return ExitCode::from(1);
                }
                println!();
            }
            Err(error) => {
                eprintln!("{error}");
                return ExitCode::from(1);
            }
        },
        CliCommand::MemoryPromote {
            workspace,
            memory_id,
            source,
        } => match promote_memory_candidate(&workspace, &memory_id, source) {
            Ok(memory) => {
                if let Err(error) = serde_json::to_writer_pretty(std::io::stdout(), &memory) {
                    eprintln!("encode memory: {error}");
                    return ExitCode::from(1);
                }
                println!();
            }
            Err(error) => {
                eprintln!("{error}");
                return ExitCode::from(1);
            }
        },
        CliCommand::RepoAudit { workspace } => match record_repo_audit_history(&workspace) {
            Ok(report) => {
                if let Err(error) = serde_json::to_writer_pretty(std::io::stdout(), &report) {
                    eprintln!("encode repo audit: {error}");
                    return ExitCode::from(1);
                }
                println!();
            }
            Err(error) => {
                eprintln!("{error}");
                return ExitCode::from(1);
            }
        },
        CliCommand::Verify {
            workspace,
            verifier_id,
        } => match run_verifier(&workspace, &verifier_id) {
            Ok(run) => {
                if let Err(error) = serde_json::to_writer_pretty(std::io::stdout(), &run) {
                    eprintln!("encode verifier run: {error}");
                    return ExitCode::from(1);
                }
                println!();
            }
            Err(error) => {
                eprintln!("{error}");
                return ExitCode::from(1);
            }
        },
        CliCommand::Probe { workspace } => match run_probe(&workspace) {
            Ok(run) => {
                if let Err(error) = serde_json::to_writer_pretty(std::io::stdout(), &run) {
                    eprintln!("encode probe run: {error}");
                    return ExitCode::from(1);
                }
                println!();
            }
            Err(error) => {
                eprintln!("{error}");
                return ExitCode::from(1);
            }
        },
        CliCommand::Demo { workspace } => {
            if let Err(error) = create_demo_workspace(&workspace) {
                eprintln!("{error}");
                return ExitCode::from(1);
            }
            println!("{}", workspace.display());
        }
    }

    ExitCode::SUCCESS
}

#[derive(Debug)]
enum CliCommand {
    Monitor {
        config: Config,
        workspace: Option<PathBuf>,
    },
    InstallInjection {
        agent: AgentKind,
        workspace: PathBuf,
        mode: InstallMode,
    },
    InjectRunning {
        workspace: Option<PathBuf>,
        mode: InstallMode,
        apply: bool,
    },
    Wrap {
        agent: AgentKind,
        workspace: PathBuf,
        session: Option<String>,
        command: Vec<String>,
    },
    Judge {
        workspace: PathBuf,
        write_intervention: bool,
        external_command: Vec<String>,
    },
    Ingest {
        adapter: AgentKind,
        workspace: PathBuf,
        session: Option<String>,
        config: Config,
    },
    HookResponse {
        adapter: AgentKind,
        workspace: PathBuf,
        session: Option<String>,
        format: HookResponseFormat,
    },
    ConfigAdvisor {
        workspace: PathBuf,
        update: AdvisorEndpointConfigUpdate,
    },
    ConfigVerifier {
        workspace: PathBuf,
        verifier: VerifierConfig,
    },
    ConfigRuntimeAuth {
        workspace: PathBuf,
        agent: AgentKind,
        runtime_auth: RuntimeAuthConfig,
    },
    ConfigImportLocal {
        workspace: PathBuf,
        home: PathBuf,
        options: LocalAgentConfigImportOptions,
    },
    ConfigImportCodingPlanCredentials {
        workspace: PathBuf,
        source_file: PathBuf,
        endpoint: Option<String>,
        model: Option<String>,
    },
    Advise {
        workspace: PathBuf,
    },
    Trail {
        workspace: PathBuf,
    },
    DevHistory {
        workspace: PathBuf,
        codex_sessions_root: PathBuf,
        claude_projects_root: PathBuf,
        top_limit: usize,
        write: bool,
        export_raw: bool,
        raw_output_root: Option<PathBuf>,
        raw_package_name: Option<String>,
    },
    Calibration {
        workspace: PathBuf,
        query: CalibrationQuery,
    },
    Blame {
        workspace: PathBuf,
        file: String,
        line: Option<u32>,
        limit: usize,
    },
    RepoHunks {
        workspace: PathBuf,
        file: Option<String>,
        line: Option<u32>,
        limit: usize,
    },
    Requirements {
        workspace: PathBuf,
        query: RequirementGraphQuery,
    },
    CompletionCertificate {
        workspace: PathBuf,
        query: RequirementGraphQuery,
    },
    Trace {
        workspace: PathBuf,
        entry: TraceEntry,
    },
    Handoff {
        workspace: PathBuf,
        agent: AgentKind,
    },
    MemoryPromote {
        workspace: PathBuf,
        memory_id: String,
        source: MemorySource,
    },
    RepoAudit {
        workspace: PathBuf,
    },
    Verify {
        workspace: PathBuf,
        verifier_id: String,
    },
    Probe {
        workspace: PathBuf,
    },
    Demo {
        workspace: PathBuf,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HookResponseFormat {
    Generic,
    Codex,
    ClaudeCode,
    OpenCode,
}

fn default_home_dir() -> PathBuf {
    env::var_os("USERPROFILE")
        .or_else(|| env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn default_coding_plan_credential_source() -> PathBuf {
    default_home_dir().join(".coding-plan").join("auth.json")
}

fn reject_local_cli_auth_source(path: &Path, label: &str) -> Result<(), String> {
    if let Some(cli_dir) = local_cli_auth_dir(path) {
        return Err(format!(
            "{label} {} points at local CLI auth directory {cli_dir}; use a dedicated coding-plan credential profile outside Codex/Claude CLI config",
            path.display()
        ));
    }
    Ok(())
}

fn local_cli_auth_dir(path: &Path) -> Option<&'static str> {
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

fn injection_workspace_for(
    agent: &coding_agent_monitor::RunningAgent,
    requested: Option<&PathBuf>,
) -> Option<PathBuf> {
    if let Some(requested) = requested {
        return Some(requested.clone());
    }

    let cwd = agent.cwd.clone()?;
    if is_agent_support_directory(&cwd) {
        None
    } else {
        Some(cwd)
    }
}

fn is_agent_support_directory(path: &std::path::Path) -> bool {
    let normalized = path.to_string_lossy().replace('/', "\\").to_lowercase();
    [
        "\\.codex\\plugins\\cache\\",
        "\\.claude\\skills\\",
        "\\node_modules\\",
        "\\.agent-monitor\\tmp\\",
    ]
    .iter()
    .any(|segment| normalized.contains(segment))
}

fn interventions_from_review(report: &AgentReviewReport) -> Vec<Intervention> {
    report
        .findings
        .iter()
        .filter_map(|finding| {
            let (kind, action) = match finding.recommended_action {
                AgentReviewAction::Continue => return None,
                AgentReviewAction::ContinueWorking | AgentReviewAction::InstallTelemetry => {
                    (InterventionKind::PrematureStop, Action::ContinueWorking)
                }
                AgentReviewAction::ForceVerification => {
                    (InterventionKind::PrematureStop, Action::ContinueWorking)
                }
                AgentReviewAction::SpawnJudgeAgent => {
                    (InterventionKind::SuspiciousChange, Action::SpawnJudgeAgent)
                }
                AgentReviewAction::SpawnFreshAgent => {
                    (InterventionKind::AgentDegraded, Action::SpawnFreshAgent)
                }
            };
            Some(Intervention {
                kind,
                action,
                agent: finding.agent.clone(),
                reason: format!("judge {}: {}", finding.category, finding.evidence),
            })
        })
        .collect()
}

fn run_hook_response(
    input: impl std::io::Read,
    mut output: impl Write,
    adapter: AgentKind,
    workspace: &Path,
    session: Option<&str>,
    format: HookResponseFormat,
) -> Result<(), String> {
    let raw: serde_json::Value =
        serde_json::from_reader(input).map_err(|error| format!("decode hook JSON: {error}"))?;
    let project_config = ProjectConfig::load(workspace.join(".agent-monitor"))
        .map_err(|error| format!("load project config: {error}"))?;
    let capabilities = adapter_capabilities_for_config(adapter, &project_config.adapters);
    if !capabilities.enabled {
        return Err(format!(
            "adapter {} is disabled in project config; refusing hook response",
            agent_kind_label(adapter)
        ));
    }
    if !capabilities.hook_pre_tool || !capabilities.can_block_tool {
        return Err(format!(
            "adapter {} does not support pre-tool blocking",
            agent_kind_label(adapter)
        ));
    }
    let response = adapter_hook_response(adapter, session, &raw, &project_config.security)
        .unwrap_or_else(|| AdapterHookResponse {
            decision: AdapterHookDecision::Allow,
            reason: None,
            session: session.map(str::to_string),
        });
    let response = apply_control_packet_hook_policy(adapter, workspace, &raw, response)?;
    persist_hook_response_events(adapter, workspace, session, &raw, &response)?;

    match format {
        HookResponseFormat::Generic => {
            serde_json::to_writer(&mut output, &response)
                .map_err(|error| format!("encode hook response: {error}"))?;
            writeln!(output).map_err(|error| format!("finish hook response: {error}"))?;
        }
        HookResponseFormat::Codex => {
            write_codex_hook_response(&mut output, &response)?;
        }
        HookResponseFormat::ClaudeCode => {
            write_claude_code_hook_response(&mut output, &response)?;
        }
        HookResponseFormat::OpenCode => {
            write_opencode_hook_response(&mut output, &response)?;
        }
    }
    Ok(())
}

fn apply_control_packet_hook_policy(
    adapter: AgentKind,
    workspace: &Path,
    raw: &serde_json::Value,
    response: AdapterHookResponse,
) -> Result<AdapterHookResponse, String> {
    if response.decision == AdapterHookDecision::Block || !hook_touches_mutating_file(raw) {
        return Ok(response);
    }
    let Some(packet) = latest_hook_control_packet(workspace, adapter, response.session.as_deref())?
    else {
        return Ok(response);
    };
    if !control_packet_requires_read_only_judge(&packet) {
        return Ok(response);
    }

    Ok(AdapterHookResponse {
        decision: AdapterHookDecision::Block,
        reason: Some(read_only_judge_hook_block_reason(&packet, raw)),
        session: response.session,
    })
}

fn latest_hook_control_packet(
    workspace: &Path,
    adapter: AgentKind,
    session: Option<&str>,
) -> Result<Option<coding_agent_monitor::ControlPacket>, String> {
    let path = workspace.join(".agent-monitor").join("packets.jsonl");
    let contents = match std::fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("read control packet log: {error}")),
    };

    let mut latest = None;
    for line in contents.lines().filter(|line| !line.trim().is_empty()) {
        let packet: coding_agent_monitor::ControlPacket = serde_json::from_str(line)
            .map_err(|error| format!("decode control packet from {}: {error}", path.display()))?;
        if control_packet_matches_hook_target(&packet, adapter, workspace, session) {
            latest = Some(packet);
        }
    }
    Ok(latest)
}

fn control_packet_matches_hook_target(
    packet: &coding_agent_monitor::ControlPacket,
    adapter: AgentKind,
    workspace: &Path,
    session: Option<&str>,
) -> bool {
    if !agent_labels_equivalent(&packet.target_agent, agent_kind_label(adapter)) {
        return false;
    }
    if let Some(expected_adapter) = packet.preconditions.adapter.as_deref()
        && !agent_labels_equivalent(expected_adapter, agent_kind_label(adapter))
    {
        return false;
    }
    if let Some(expected_session) = packet.preconditions.agent_session_id.as_deref()
        && session != Some(expected_session)
    {
        return false;
    }
    if let Some(expected_worktree) = packet.preconditions.worktree.as_deref()
        && normalize_hook_path_for_match(expected_worktree)
            != normalize_hook_path_for_match(&workspace.display().to_string())
    {
        return false;
    }
    if let Some(expected_head) = packet.preconditions.git_head.as_deref() {
        let actual_head =
            hook_current_git_head(workspace).unwrap_or_else(|| "<unavailable>".into());
        if expected_head != actual_head {
            return false;
        }
    }
    true
}

fn hook_current_git_head(workspace: &Path) -> Option<String> {
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

fn normalize_hook_path_for_match(path: &str) -> String {
    let path = path.replace('\\', "/").to_lowercase();
    let rooted = path.starts_with('/');
    let mut components = Vec::new();
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
    let normalized = if rooted {
        format!("/{normalized}")
    } else {
        normalized
    };
    normalized.trim_start_matches("./").to_string()
}

fn agent_labels_equivalent(left: &str, right: &str) -> bool {
    normalize_agent_label_for_hook(left) == normalize_agent_label_for_hook(right)
}

fn normalize_agent_label_for_hook(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn control_packet_requires_read_only_judge(packet: &coding_agent_monitor::ControlPacket) -> bool {
    let text = std::iter::once(packet.title.as_str())
        .chain(std::iter::once(packet.summary.as_str()))
        .chain(
            packet
                .instructions
                .iter()
                .map(|instruction| instruction.text.as_str()),
        )
        .chain(packet.forbidden.iter().map(String::as_str))
        .collect::<Vec<_>>()
        .join("\n")
        .to_ascii_lowercase();

    text.contains("read-only judge")
        || (text.contains("judge")
            && (text.contains("do not edit")
                || text.contains("without editing")
                || text.contains("do not write")
                || text.contains("do not mutate")))
}

fn hook_touches_mutating_file(raw: &serde_json::Value) -> bool {
    let Some(tool_name) = hook_tool_name(raw).map(|tool| tool.to_ascii_lowercase()) else {
        return false;
    };
    matches!(
        tool_name.as_str(),
        "write"
            | "edit"
            | "multiedit"
            | "notebookedit"
            | "apply_patch"
            | "patch"
            | "delete"
            | "move"
            | "rename"
            | "create"
    ) || hook_command_looks_like_patch(raw)
}

fn hook_tool_name(raw: &serde_json::Value) -> Option<&str> {
    raw.get("tool_name")
        .and_then(serde_json::Value::as_str)
        .or_else(|| raw.get("tool").and_then(serde_json::Value::as_str))
        .or_else(|| raw.get("name").and_then(serde_json::Value::as_str))
}

fn hook_command_looks_like_patch(raw: &serde_json::Value) -> bool {
    hook_input_string(raw, "command")
        .map(|command| {
            let command = command.to_ascii_lowercase();
            command.contains("apply_patch") || command.contains("git apply")
        })
        .unwrap_or(false)
}

fn read_only_judge_hook_block_reason(
    packet: &coding_agent_monitor::ControlPacket,
    raw: &serde_json::Value,
) -> String {
    let target = hook_mutating_target(raw)
        .map(|target| format!(" for {target}"))
        .unwrap_or_default();
    format!(
        "blocked by read-only judge packet {}: {}{} would mutate the worktree",
        packet.packet_id,
        hook_tool_name(raw).unwrap_or("tool"),
        target
    )
}

fn hook_mutating_target(raw: &serde_json::Value) -> Option<String> {
    [
        "file_path",
        "path",
        "filename",
        "target_file",
        "notebook_path",
    ]
    .into_iter()
    .find_map(|key| hook_input_string(raw, key).map(str::to_string))
}

fn hook_input_string<'a>(raw: &'a serde_json::Value, key: &str) -> Option<&'a str> {
    raw.pointer(&format!("/tool_input/{key}"))
        .and_then(serde_json::Value::as_str)
        .or_else(|| {
            raw.pointer(&format!("/input/{key}"))
                .and_then(serde_json::Value::as_str)
        })
        .or_else(|| raw.get(key).and_then(serde_json::Value::as_str))
}

fn persist_hook_response_events(
    adapter: AgentKind,
    workspace: &Path,
    session: Option<&str>,
    raw: &serde_json::Value,
    response: &AdapterHookResponse,
) -> Result<(), String> {
    let mut store = ProjectStore::open(workspace)
        .map_err(|error| format!("open hook response store: {error}"))?;
    let Some(mut attempt) = normalize_adapter_event(adapter, session, raw) else {
        return Ok(());
    };
    let attempt_id = ensure_hook_event_id(&mut attempt, "hook-attempt");
    store
        .append_event(&attempt)
        .map_err(|error| format!("persist hook attempt event: {error}"))?;

    if response.decision != AdapterHookDecision::Allow {
        let decision = hook_response_decision_event(adapter, &attempt, &attempt_id, response);
        store
            .append_event(&decision)
            .map_err(|error| format!("persist hook response event: {error}"))?;
    }

    Ok(())
}

fn ensure_hook_event_id(event: &mut Event, fallback_prefix: &str) -> String {
    if let Some(event_id) = event
        .event_id
        .as_deref()
        .filter(|event_id| !event_id.is_empty())
    {
        return event_id.to_string();
    }
    let event_id = format!(
        "{}-{}",
        fallback_prefix,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default()
    );
    event.event_id = Some(event_id.clone());
    event_id
}

fn hook_response_decision_event(
    adapter: AgentKind,
    attempt: &Event,
    attempt_id: &str,
    response: &AdapterHookResponse,
) -> Event {
    Event {
        time: attempt.time.clone(),
        event_id: Some(format!("{attempt_id}:hook-response")),
        project_id: attempt.project_id.clone(),
        run_id: attempt.run_id.clone(),
        workspace: attempt.workspace.clone(),
        adapter: attempt
            .adapter
            .clone()
            .or_else(|| Some(agent_kind_label(adapter).into())),
        adapter_version: attempt.adapter_version.clone(),
        observed_at: attempt.observed_at.clone(),
        occurred_at: attempt.occurred_at.clone(),
        cwd: attempt.cwd.clone(),
        worktree: attempt.worktree.clone(),
        git_head: attempt.git_head.clone(),
        git_branch: attempt.git_branch.clone(),
        git_dirty: attempt.git_dirty,
        agent: agent_kind_label(adapter).into(),
        actor: Some("monitor".into()),
        provider: attempt.provider.clone(),
        model: attempt.model.clone(),
        session: response.session.clone().or_else(|| attempt.session.clone()),
        agent_session_id: response
            .session
            .clone()
            .or_else(|| attempt.agent_session_id.clone()),
        kind: EventKind::InterventionResult,
        content: Some(hook_response_decision_content(response)),
        command: attempt.command.clone(),
        related_event_ids: vec![attempt_id.to_string()],
        ..Event::default()
    }
}

fn hook_response_decision_content(response: &AdapterHookResponse) -> String {
    let reason = response
        .reason
        .as_deref()
        .unwrap_or(match response.decision {
            AdapterHookDecision::Allow => "allowed by coding-agent-monitor policy",
            AdapterHookDecision::Ask => {
                "user authorization requested by coding-agent-monitor policy"
            }
            AdapterHookDecision::Block => "blocked by coding-agent-monitor policy",
        });
    match response.decision {
        AdapterHookDecision::Allow => format!("hook response allowed tool execution: {reason}"),
        AdapterHookDecision::Ask => {
            format!("hook response requested user authorization: {reason}")
        }
        AdapterHookDecision::Block => format!("hook response blocked tool execution: {reason}"),
    }
}

fn write_codex_hook_response(
    mut output: impl Write,
    response: &AdapterHookResponse,
) -> Result<(), String> {
    if response.decision == AdapterHookDecision::Allow {
        return Ok(());
    }
    let value = serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "deny",
            "permissionDecisionReason": response
                .reason
                .as_deref()
                .unwrap_or("blocked by coding-agent-monitor policy"),
        }
    });
    serde_json::to_writer(&mut output, &value)
        .map_err(|error| format!("encode Codex hook response: {error}"))?;
    writeln!(output).map_err(|error| format!("finish Codex hook response: {error}"))?;
    Ok(())
}

fn write_claude_code_hook_response(
    mut output: impl Write,
    response: &AdapterHookResponse,
) -> Result<(), String> {
    if response.decision == AdapterHookDecision::Allow {
        return Ok(());
    }
    let permission_decision = match response.decision {
        AdapterHookDecision::Ask => "ask",
        AdapterHookDecision::Allow | AdapterHookDecision::Block => "deny",
    };
    let value = serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": permission_decision,
            "permissionDecisionReason": response
                .reason
                .as_deref()
                .unwrap_or("blocked by coding-agent-monitor policy"),
        }
    });
    serde_json::to_writer(&mut output, &value)
        .map_err(|error| format!("encode Claude Code hook response: {error}"))?;
    writeln!(output).map_err(|error| format!("finish Claude Code hook response: {error}"))?;
    Ok(())
}

fn write_opencode_hook_response(
    mut output: impl Write,
    response: &AdapterHookResponse,
) -> Result<(), String> {
    if response.decision == AdapterHookDecision::Allow {
        return Ok(());
    }
    let reason = response
        .reason
        .as_deref()
        .unwrap_or("blocked by coding-agent-monitor policy");
    let value = serde_json::json!({
        "action": "block",
        "decision": "block",
        "message": reason,
        "reason": reason,
    });
    serde_json::to_writer(&mut output, &value)
        .map_err(|error| format!("encode OpenCode hook response: {error}"))?;
    writeln!(output).map_err(|error| format!("finish OpenCode hook response: {error}"))?;
    Ok(())
}

fn run_external_judge(command: &[String], report: &AgentReviewReport) -> Result<(), String> {
    let launch = prepare_wrapped_launch(command).map_err(|error| error.to_string())?;
    let mut child = Command::new(&launch.program)
        .args(&launch.args)
        .stdin(Stdio::piped())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .map_err(|error| format!("spawn external judge {}: {error}", launch.program))?;

    if let Some(mut stdin) = child.stdin.take() {
        writeln!(stdin, "{EXTERNAL_JUDGE_PROMPT}\n")
            .map_err(|error| format!("write external judge prompt: {error}"))?;
        serde_json::to_writer_pretty(&mut stdin, report)
            .map_err(|error| format!("encode external judge packet: {error}"))?;
        writeln!(stdin).map_err(|error| format!("finish external judge prompt: {error}"))?;
    }

    let status = child
        .wait()
        .map_err(|error| format!("wait external judge: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("external judge exited with {status}"))
    }
}

#[cfg(test)]
#[path = "main_tests.rs"]
mod tests;
