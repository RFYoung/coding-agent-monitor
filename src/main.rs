use coding_agent_monitor::{
    AcceptanceCoverageStatus, Action, AdapterHookDecision, AdapterHookResponse,
    AdapterIngestOptions, AdvisorCredentialSource, AdvisorEndpointConfigUpdate, AgentKind,
    AgentReviewAction, AgentReviewReport, BlameQuery, CalibrationQuery, Config, ControlActionKind,
    DashboardSnapshot, DevHistoryAnalysisOptions, DevHistoryRawExportOptions, Event, EventKind,
    InstallMode, Intervention, InterventionKind, LocalAgentConfigImportOptions, MemorySource,
    ProjectConfig, ProjectStore, RepoHunkHistoryQuery, RequirementGraphQuery, TraceEntry,
    WrappedCommand, adapter_capabilities_for_config, adapter_hook_response, advise_workspace,
    agent_kind_label, analyze_local_dev_history, create_demo_workspace,
    detect_running_agents_from_system, export_raw_dev_history, handoff_workspace,
    import_coding_plan_advisor_credentials, import_local_agent_configs,
    injection_plan_for_workspace, install_agent_injection, judge_snapshot, load_blame_report,
    load_calibration_report, load_completion_certificate_report, load_decision_trails,
    load_repo_hunk_history, load_requirement_graph, normalize_adapter_event,
    prepare_wrapped_launch, promote_memory_candidate, record_repo_audit_history,
    record_trace_entry, run_adapter_jsonl_with_store, run_jsonl, run_jsonl_with_store, run_probe,
    run_verifier, run_wrapped_command, write_advisor_endpoint_config,
};
use std::env;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

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

fn parse_cli(args: impl IntoIterator<Item = impl Into<String>>) -> Result<CliCommand, String> {
    let args = args.into_iter().map(Into::into).collect::<Vec<_>>();
    if args.first().is_some_and(|arg| arg == "inject") {
        parse_inject_args(args.into_iter().skip(1))
    } else if args.first().is_some_and(|arg| arg == "inject-running") {
        parse_inject_running_args(args.into_iter().skip(1))
    } else if args.first().is_some_and(|arg| arg == "wrap") {
        parse_wrap_args(args.into_iter().skip(1))
    } else if args.first().is_some_and(|arg| arg == "judge") {
        parse_judge_args(args.into_iter().skip(1))
    } else if args.first().is_some_and(|arg| arg == "ingest") {
        parse_ingest_args(args.into_iter().skip(1))
    } else if args.first().is_some_and(|arg| arg == "hook-response") {
        parse_hook_response_args(args.into_iter().skip(1))
    } else if args.first().is_some_and(|arg| arg == "config") {
        parse_config_args(args.into_iter().skip(1))
    } else if args.first().is_some_and(|arg| arg == "advise") {
        parse_advise_args(args.into_iter().skip(1))
    } else if args.first().is_some_and(|arg| arg == "trail") {
        parse_trail_args(args.into_iter().skip(1))
    } else if args.first().is_some_and(|arg| arg == "dev-history") {
        parse_dev_history_args(args.into_iter().skip(1))
    } else if args.first().is_some_and(|arg| arg == "calibration") {
        parse_calibration_args(args.into_iter().skip(1))
    } else if args.first().is_some_and(|arg| arg == "blame") {
        parse_blame_args(args.into_iter().skip(1))
    } else if args.first().is_some_and(|arg| arg == "repo-hunks") {
        parse_repo_hunks_args(args.into_iter().skip(1))
    } else if args.first().is_some_and(|arg| arg == "requirements") {
        parse_requirements_args(args.into_iter().skip(1))
    } else if args
        .first()
        .is_some_and(|arg| arg == "completion-certificate")
    {
        parse_completion_certificate_args(args.into_iter().skip(1))
    } else if args.first().is_some_and(|arg| arg == "trace") {
        parse_trace_args(args.into_iter().skip(1))
    } else if args.first().is_some_and(|arg| arg == "handoff") {
        parse_handoff_args(args.into_iter().skip(1))
    } else if args.first().is_some_and(|arg| arg == "memory") {
        parse_memory_args(args.into_iter().skip(1))
    } else if args.first().is_some_and(|arg| arg == "repo-audit") {
        parse_repo_audit_args(args.into_iter().skip(1))
    } else if args.first().is_some_and(|arg| arg == "verify") {
        parse_verify_args(args.into_iter().skip(1))
    } else if args.first().is_some_and(|arg| arg == "probe") {
        parse_probe_args(args.into_iter().skip(1))
    } else if args.first().is_some_and(|arg| arg == "demo") {
        parse_demo_args(args.into_iter().skip(1))
    } else {
        parse_monitor_args(args)
    }
}

fn parse_monitor_args(args: impl IntoIterator<Item = String>) -> Result<CliCommand, String> {
    let mut config = Config::default();
    let mut workspace = None;

    for arg in args {
        if let Some(value) = arg.strip_prefix("--open-work=") {
            config.open_work = parse_bool("--open-work", value)?;
        } else if let Some(value) = arg.strip_prefix("--retry-limit=") {
            config.retry_limit = value
                .parse()
                .map_err(|_| "--retry-limit must be a non-negative integer".to_string())?;
        } else if let Some(value) = arg.strip_prefix("--fallbacks=") {
            config.fallback_agents = value
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .collect();
            if config.fallback_agents.is_empty() {
                return Err("--fallbacks must include at least one agent".into());
            }
        } else if let Some(value) = arg.strip_prefix("--workspace=") {
            workspace = Some(PathBuf::from(value));
        } else {
            return Err(format!("unknown argument: {arg}"));
        }
    }

    Ok(CliCommand::Monitor { config, workspace })
}

fn parse_inject_args(args: impl IntoIterator<Item = String>) -> Result<CliCommand, String> {
    let mut agent = None;
    let mut workspace = PathBuf::from(".");
    let mut mode = InstallMode::MergeManagedBlock;

    for arg in args {
        if let Some(value) = arg.strip_prefix("--agent=") {
            agent = Some(value.parse().map_err(|error| format!("{error}"))?);
        } else if let Some(value) = arg.strip_prefix("--workspace=") {
            workspace = PathBuf::from(value);
        } else if let Some(value) = arg.strip_prefix("--overwrite=") {
            mode = if parse_bool("--overwrite", value)? {
                InstallMode::CreateOrOverwrite
            } else {
                InstallMode::MergeManagedBlock
            };
        } else {
            return Err(format!("unknown inject argument: {arg}"));
        }
    }

    let agent = agent
        .ok_or_else(|| "inject requires --agent=<codex|claude-code|pi|opencode>".to_string())?;
    Ok(CliCommand::InstallInjection {
        agent,
        workspace,
        mode,
    })
}

fn parse_inject_running_args(args: impl IntoIterator<Item = String>) -> Result<CliCommand, String> {
    let mut workspace = None;
    let mut mode = InstallMode::MergeManagedBlock;
    let mut apply = false;

    for arg in args {
        if let Some(value) = arg.strip_prefix("--workspace=") {
            workspace = Some(PathBuf::from(value));
        } else if let Some(value) = arg.strip_prefix("--overwrite=") {
            mode = if parse_bool("--overwrite", value)? {
                InstallMode::CreateOrOverwrite
            } else {
                InstallMode::MergeManagedBlock
            };
        } else if arg == "--apply" {
            apply = true;
        } else if let Some(value) = arg.strip_prefix("--apply=") {
            apply = parse_bool("--apply", value)?;
        } else {
            return Err(format!("unknown inject-running argument: {arg}"));
        }
    }

    Ok(CliCommand::InjectRunning {
        workspace,
        mode,
        apply,
    })
}

fn parse_wrap_args(args: impl IntoIterator<Item = String>) -> Result<CliCommand, String> {
    let mut agent = None;
    let mut workspace = PathBuf::from(".");
    let mut session = None;
    let mut command = Vec::new();
    let mut after_separator = false;

    for arg in args {
        if after_separator {
            command.push(arg);
        } else if arg == "--" {
            after_separator = true;
        } else if let Some(value) = arg.strip_prefix("--agent=") {
            agent = Some(value.parse().map_err(|error| format!("{error}"))?);
        } else if let Some(value) = arg.strip_prefix("--workspace=") {
            workspace = PathBuf::from(value);
        } else if let Some(value) = arg.strip_prefix("--session=") {
            session = Some(value.to_string());
        } else {
            return Err(format!("unknown wrap argument: {arg}"));
        }
    }

    let agent =
        agent.ok_or_else(|| "wrap requires --agent=<codex|claude-code|pi|opencode>".to_string())?;
    if command.is_empty() {
        return Err("wrap requires -- <command> [args...]".into());
    }

    Ok(CliCommand::Wrap {
        agent,
        workspace,
        session,
        command,
    })
}

fn parse_judge_args(args: impl IntoIterator<Item = String>) -> Result<CliCommand, String> {
    let mut workspace = PathBuf::from(".");
    let mut write_intervention = false;
    let mut external_command = Vec::new();
    let mut after_separator = false;

    for arg in args {
        if after_separator {
            external_command.push(arg);
        } else if arg == "--" {
            after_separator = true;
        } else if let Some(value) = arg.strip_prefix("--workspace=") {
            workspace = PathBuf::from(value);
        } else if let Some(value) = arg.strip_prefix("--write-intervention=") {
            write_intervention = parse_bool("--write-intervention", value)?;
        } else {
            return Err(format!("unknown judge argument: {arg}"));
        }
    }

    Ok(CliCommand::Judge {
        workspace,
        write_intervention,
        external_command,
    })
}

fn parse_ingest_args(args: impl IntoIterator<Item = String>) -> Result<CliCommand, String> {
    let mut adapter = None;
    let mut workspace = PathBuf::from(".");
    let mut session = None;
    let mut config = Config::default();

    for arg in args {
        if let Some(value) = arg.strip_prefix("--adapter=") {
            adapter = Some(value.parse().map_err(|error| format!("{error}"))?);
        } else if let Some(value) = arg.strip_prefix("--workspace=") {
            workspace = PathBuf::from(value);
        } else if let Some(value) = arg.strip_prefix("--session=") {
            session = Some(value.to_string());
        } else if let Some(value) = arg.strip_prefix("--open-work=") {
            config.open_work = parse_bool("--open-work", value)?;
        } else if let Some(value) = arg.strip_prefix("--retry-limit=") {
            config.retry_limit = value
                .parse()
                .map_err(|_| "--retry-limit must be a non-negative integer".to_string())?;
        } else if let Some(value) = arg.strip_prefix("--fallbacks=") {
            config.fallback_agents = value
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .collect();
            if config.fallback_agents.is_empty() {
                return Err("--fallbacks must include at least one agent".into());
            }
        } else {
            return Err(format!("unknown ingest argument: {arg}"));
        }
    }

    let adapter = adapter
        .ok_or_else(|| "ingest requires --adapter=<codex|claude-code|pi|opencode>".to_string())?;
    Ok(CliCommand::Ingest {
        adapter,
        workspace,
        session,
        config,
    })
}

fn parse_hook_response_args(args: impl IntoIterator<Item = String>) -> Result<CliCommand, String> {
    let mut adapter = None;
    let mut workspace = PathBuf::from(".");
    let mut session = None;
    let mut format = HookResponseFormat::Generic;

    for arg in args {
        if let Some(value) = arg.strip_prefix("--adapter=") {
            adapter = Some(
                value
                    .parse::<AgentKind>()
                    .map_err(|error| error.to_string())?,
            );
        } else if let Some(value) = arg.strip_prefix("--workspace=") {
            workspace = PathBuf::from(value);
        } else if let Some(value) = arg.strip_prefix("--session=") {
            let value = value.trim();
            if value.is_empty() {
                return Err("--session must not be empty".into());
            }
            session = Some(value.to_string());
        } else if let Some(value) = arg.strip_prefix("--format=") {
            format = parse_hook_response_format(value)?;
        } else {
            return Err(format!("unknown hook-response argument: {arg}"));
        }
    }

    let adapter = adapter.ok_or_else(|| {
        "hook-response requires --adapter=<codex|claude-code|pi|opencode>".to_string()
    })?;
    Ok(CliCommand::HookResponse {
        adapter,
        workspace,
        session,
        format,
    })
}

fn parse_hook_response_format(value: &str) -> Result<HookResponseFormat, String> {
    match value.trim() {
        "generic" => Ok(HookResponseFormat::Generic),
        "codex" => Ok(HookResponseFormat::Codex),
        "claude-code" | "claude" => Ok(HookResponseFormat::ClaudeCode),
        "opencode" | "open-code" => Ok(HookResponseFormat::OpenCode),
        "" => Err("--format must not be empty".into()),
        _ => Err("--format must be generic, codex, claude-code, or opencode".into()),
    }
}

fn parse_config_args(args: impl IntoIterator<Item = String>) -> Result<CliCommand, String> {
    let args = args.into_iter().collect::<Vec<_>>();
    match args.first().map(String::as_str) {
        Some("advisor") => parse_config_advisor_args(args.into_iter().skip(1)),
        Some("import-local") => parse_config_import_local_args(args.into_iter().skip(1)),
        Some("import-coding-plan-credentials") => {
            parse_config_import_coding_plan_credentials_args(args.into_iter().skip(1))
        }
        _ => Err(
            "config requires subcommand: advisor|import-local|import-coding-plan-credentials"
                .into(),
        ),
    }
}

fn parse_config_advisor_args(args: impl IntoIterator<Item = String>) -> Result<CliCommand, String> {
    let mut workspace = PathBuf::from(".");
    let mut endpoint = None;
    let mut model = None;
    let mut api_key_env = None;
    let mut credential_source = None;
    let mut credential_file = None;
    let mut enabled = true;

    for arg in args {
        if let Some(value) = arg.strip_prefix("--workspace=") {
            workspace = PathBuf::from(value);
        } else if let Some(value) = arg.strip_prefix("--endpoint=") {
            endpoint = Some(value.trim().to_string());
        } else if let Some(value) = arg.strip_prefix("--model=") {
            model = Some(value.trim().to_string());
        } else if let Some(value) = arg.strip_prefix("--api-key-env=") {
            api_key_env = Some(parse_api_key_env_name(value)?);
        } else if let Some(value) = arg.strip_prefix("--credential-source=") {
            credential_source = Some(parse_advisor_credential_source(value)?);
        } else if let Some(value) = arg.strip_prefix("--credential-file=") {
            let value = value.trim();
            if value.is_empty() {
                return Err("--credential-file must not be empty".into());
            }
            credential_file = Some(value.to_string());
        } else if let Some(value) = arg.strip_prefix("--enabled=") {
            enabled = parse_bool("--enabled", value)?;
        } else {
            return Err(format!("unknown config advisor argument: {arg}"));
        }
    }

    let endpoint =
        endpoint.ok_or_else(|| "config advisor requires --endpoint=<url>".to_string())?;
    let model = model.ok_or_else(|| "config advisor requires --model=<name>".to_string())?;
    let api_key_env = match api_key_env {
        Some(api_key_env) => api_key_env,
        None if credential_source.is_some_and(|source| source != AdvisorCredentialSource::Env) => {
            "OPENAI_API_KEY".into()
        }
        None => return Err("config advisor requires --api-key-env=<ENV_NAME>".into()),
    };
    if endpoint.is_empty() {
        return Err("--endpoint must not be empty".into());
    }
    if model.is_empty() {
        return Err("--model must not be empty".into());
    }
    match credential_source {
        Some(AdvisorCredentialSource::Env) if credential_file.is_some() => {
            return Err("--credential-file requires a non-env --credential-source".into());
        }
        Some(source) if source != AdvisorCredentialSource::Env && credential_file.is_none() => {
            return Err("--credential-source requires --credential-file=<path>".into());
        }
        None if credential_file.is_some() => {
            return Err("--credential-file requires --credential-source=<source>".into());
        }
        _ => {}
    }

    Ok(CliCommand::ConfigAdvisor {
        workspace,
        update: AdvisorEndpointConfigUpdate {
            endpoint,
            model,
            api_key_env,
            credential_source,
            credential_file,
            enabled,
        },
    })
}

fn parse_advisor_credential_source(value: &str) -> Result<AdvisorCredentialSource, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "env" => Ok(AdvisorCredentialSource::Env),
        "coding_plan" | "coding-plan" => Ok(AdvisorCredentialSource::CodingPlan),
        _ => Err("credential source must be env or coding-plan".into()),
    }
}

fn parse_config_import_local_args(
    args: impl IntoIterator<Item = String>,
) -> Result<CliCommand, String> {
    let mut workspace = PathBuf::from(".");
    let mut home = default_home_dir();
    let mut codex = true;
    let mut claude_code = true;
    let mut advisor_credential_source = None;
    let mut advisor_credential_file = None;

    for arg in args {
        if let Some(value) = arg.strip_prefix("--workspace=") {
            workspace = PathBuf::from(value);
        } else if let Some(value) = arg.strip_prefix("--home=") {
            home = PathBuf::from(value);
        } else if let Some(value) = arg.strip_prefix("--codex=") {
            codex = parse_bool("--codex", value)?;
        } else if let Some(value) = arg.strip_prefix("--claude-code=") {
            claude_code = parse_bool("--claude-code", value)?;
        } else if let Some(value) = arg.strip_prefix("--copy-credentials=") {
            if parse_bool("--copy-credentials", value)? {
                return Err(
                    "--copy-credentials has been removed; use dedicated advisor credentials with --advisor-credential-source and --advisor-credential-file".into(),
                );
            }
        } else if let Some(value) = arg.strip_prefix("--advisor-credential-source=") {
            advisor_credential_source = Some(parse_advisor_credential_source(value)?);
        } else if let Some(value) = arg.strip_prefix("--advisor-credential-file=") {
            let value = value.trim();
            if value.is_empty() {
                return Err("--advisor-credential-file must not be empty".into());
            }
            advisor_credential_file = Some(value.to_string());
        } else {
            return Err(format!("unknown config import-local argument: {arg}"));
        }
    }
    if advisor_credential_source == Some(AdvisorCredentialSource::Env)
        && advisor_credential_file.is_some()
    {
        return Err(
            "--advisor-credential-file requires a non-env --advisor-credential-source".into(),
        );
    }
    if advisor_credential_source.is_some() && advisor_credential_file.is_none() {
        return Err("--advisor-credential-source requires --advisor-credential-file=<path>".into());
    }

    Ok(CliCommand::ConfigImportLocal {
        workspace,
        home,
        options: LocalAgentConfigImportOptions {
            codex,
            claude_code,
            copy_credentials: false,
            advisor_credential_source,
            advisor_credential_file,
        },
    })
}

fn parse_config_import_coding_plan_credentials_args(
    args: impl IntoIterator<Item = String>,
) -> Result<CliCommand, String> {
    let mut workspace = PathBuf::from(".");
    let mut source_file = default_coding_plan_credential_source();
    let mut endpoint = None;
    let mut model = None;

    for arg in args {
        if let Some(value) = arg.strip_prefix("--workspace=") {
            workspace = PathBuf::from(value);
        } else if let Some(value) = arg.strip_prefix("--source-file=") {
            let value = value.trim();
            if value.is_empty() {
                return Err("--source-file must not be empty".into());
            }
            source_file = PathBuf::from(value);
            reject_local_cli_auth_source(&source_file, "--source-file")?;
        } else if let Some(value) = arg.strip_prefix("--endpoint=") {
            let value = value.trim();
            if value.is_empty() {
                return Err("--endpoint must not be empty".into());
            }
            endpoint = Some(value.to_string());
        } else if let Some(value) = arg.strip_prefix("--model=") {
            let value = value.trim();
            if value.is_empty() {
                return Err("--model must not be empty".into());
            }
            model = Some(value.to_string());
        } else {
            return Err(format!(
                "unknown config import-coding-plan-credentials argument: {arg}"
            ));
        }
    }

    Ok(CliCommand::ConfigImportCodingPlanCredentials {
        workspace,
        source_file,
        endpoint,
        model,
    })
}

fn parse_advise_args(args: impl IntoIterator<Item = String>) -> Result<CliCommand, String> {
    let mut workspace = PathBuf::from(".");

    for arg in args {
        if let Some(value) = arg.strip_prefix("--workspace=") {
            workspace = PathBuf::from(value);
        } else {
            return Err(format!("unknown advise argument: {arg}"));
        }
    }

    Ok(CliCommand::Advise { workspace })
}

fn parse_trail_args(args: impl IntoIterator<Item = String>) -> Result<CliCommand, String> {
    let mut workspace = PathBuf::from(".");

    for arg in args {
        if let Some(value) = arg.strip_prefix("--workspace=") {
            workspace = PathBuf::from(value);
        } else {
            return Err(format!("unknown trail argument: {arg}"));
        }
    }

    Ok(CliCommand::Trail { workspace })
}

fn parse_dev_history_args(args: impl IntoIterator<Item = String>) -> Result<CliCommand, String> {
    let mut workspace = PathBuf::from(".");
    let mut home = default_home_dir();
    let mut codex_sessions_root = None;
    let mut claude_projects_root = None;
    let mut top_limit = 20;
    let mut write = false;
    let mut export_raw = false;
    let mut raw_output_root = None;
    let mut raw_package_name = None;

    for arg in args {
        if let Some(value) = arg.strip_prefix("--workspace=") {
            workspace = PathBuf::from(value);
        } else if let Some(value) = arg.strip_prefix("--home=") {
            home = PathBuf::from(value);
        } else if let Some(value) = arg.strip_prefix("--codex-sessions=") {
            codex_sessions_root = Some(PathBuf::from(value));
        } else if let Some(value) = arg.strip_prefix("--claude-projects=") {
            claude_projects_root = Some(PathBuf::from(value));
        } else if let Some(value) = arg
            .strip_prefix("--top=")
            .or_else(|| arg.strip_prefix("--limit="))
        {
            top_limit = value
                .parse()
                .map_err(|_| "--top must be a positive integer".to_string())?;
            if top_limit == 0 {
                return Err("--top must be a positive integer".into());
            }
        } else if arg == "--write" {
            write = true;
        } else if let Some(value) = arg.strip_prefix("--write=") {
            write = parse_bool("--write", value)?;
        } else if arg == "--export-raw" {
            export_raw = true;
        } else if let Some(value) = arg.strip_prefix("--export-raw=") {
            export_raw = parse_bool("--export-raw", value)?;
        } else if let Some(value) = arg.strip_prefix("--output=") {
            raw_output_root = Some(PathBuf::from(value));
        } else if let Some(value) = arg.strip_prefix("--package-name=") {
            let value = value.trim();
            if value.is_empty() {
                return Err("--package-name must not be empty".into());
            }
            raw_package_name = Some(value.to_string());
        } else {
            return Err(format!("unknown dev-history argument: {arg}"));
        }
    }

    Ok(CliCommand::DevHistory {
        workspace,
        codex_sessions_root: codex_sessions_root
            .unwrap_or_else(|| home.join(".codex").join("sessions")),
        claude_projects_root: claude_projects_root
            .unwrap_or_else(|| home.join(".claude").join("projects")),
        top_limit,
        write,
        export_raw,
        raw_output_root,
        raw_package_name,
    })
}

fn parse_calibration_args(args: impl IntoIterator<Item = String>) -> Result<CliCommand, String> {
    let mut workspace = PathBuf::from(".");
    let mut query = CalibrationQuery::default();

    for arg in args {
        if let Some(value) = arg.strip_prefix("--workspace=") {
            workspace = PathBuf::from(value);
        } else if let Some(value) = arg.strip_prefix("--action=") {
            query.action = Some(parse_control_action_kind(value)?);
        } else if let Some(value) = arg.strip_prefix("--limit=") {
            query.limit = value
                .parse()
                .map_err(|_| "--limit must be a non-negative integer".to_string())?;
        } else {
            return Err(format!("unknown calibration argument: {arg}"));
        }
    }

    Ok(CliCommand::Calibration { workspace, query })
}

fn parse_control_action_kind(value: &str) -> Result<ControlActionKind, String> {
    match value.trim().to_ascii_lowercase().replace('-', "_").as_str() {
        "continue_working" | "continue" => Ok(ControlActionKind::ContinueWorking),
        "retry_agent" | "retry" => Ok(ControlActionKind::RetryAgent),
        "force_verification" | "verify" => Ok(ControlActionKind::ForceVerification),
        "run_probe" | "probe" => Ok(ControlActionKind::RunProbe),
        "send_follow_up" | "follow_up" => Ok(ControlActionKind::SendFollowUp),
        "spawn_fresh_agent" | "spawn_fresh" | "spawn" => {
            Ok(ControlActionKind::SpawnFreshAgent)
        }
        "switch_agent" | "switch" => Ok(ControlActionKind::SwitchAgent),
        "ask_user" | "ask" => Ok(ControlActionKind::AskUser),
        "pause" => Ok(ControlActionKind::Pause),
        _ => Err("action must be continue_working, retry_agent, force_verification, run_probe, send_follow_up, spawn_fresh_agent, switch_agent, ask_user, or pause".into()),
    }
}

fn parse_blame_args(args: impl IntoIterator<Item = String>) -> Result<CliCommand, String> {
    let mut workspace = PathBuf::from(".");
    let mut file = None;
    let mut line = None;
    let mut limit = 10;

    for arg in args {
        if let Some(value) = arg.strip_prefix("--workspace=") {
            workspace = PathBuf::from(value);
        } else if let Some(value) = arg.strip_prefix("--file=") {
            file = Some(value.to_string());
        } else if let Some(value) = arg.strip_prefix("--line=") {
            let parsed = value
                .parse()
                .map_err(|_| "--line must be a positive integer".to_string())?;
            if parsed == 0 {
                return Err("--line must be a positive integer".to_string());
            }
            line = Some(parsed);
        } else if let Some(value) = arg.strip_prefix("--limit=") {
            limit = value
                .parse()
                .map_err(|_| "--limit must be a non-negative integer".to_string())?;
        } else {
            return Err(format!("unknown blame argument: {arg}"));
        }
    }

    let file = file.ok_or_else(|| "blame requires --file=<path>".to_string())?;
    Ok(CliCommand::Blame {
        workspace,
        file,
        line,
        limit,
    })
}

fn parse_repo_hunks_args(args: impl IntoIterator<Item = String>) -> Result<CliCommand, String> {
    let mut workspace = PathBuf::from(".");
    let mut file = None;
    let mut line = None;
    let mut limit = 25;

    for arg in args {
        if let Some(value) = arg.strip_prefix("--workspace=") {
            workspace = PathBuf::from(value);
        } else if let Some(value) = arg.strip_prefix("--file=") {
            file = Some(value.to_string());
        } else if let Some(value) = arg.strip_prefix("--line=") {
            let parsed = value
                .parse()
                .map_err(|_| "--line must be a positive integer".to_string())?;
            if parsed == 0 {
                return Err("--line must be a positive integer".to_string());
            }
            line = Some(parsed);
        } else if let Some(value) = arg.strip_prefix("--limit=") {
            limit = value
                .parse()
                .map_err(|_| "--limit must be a non-negative integer".to_string())?;
        } else {
            return Err(format!("unknown repo-hunks argument: {arg}"));
        }
    }

    Ok(CliCommand::RepoHunks {
        workspace,
        file,
        line,
        limit,
    })
}

fn parse_requirements_args(args: impl IntoIterator<Item = String>) -> Result<CliCommand, String> {
    let (workspace, query) = parse_requirement_query_args("requirements", args)?;

    Ok(CliCommand::Requirements { workspace, query })
}

fn parse_completion_certificate_args(
    args: impl IntoIterator<Item = String>,
) -> Result<CliCommand, String> {
    let (workspace, query) = parse_requirement_query_args("completion-certificate", args)?;

    Ok(CliCommand::CompletionCertificate { workspace, query })
}

fn parse_trace_args(args: impl IntoIterator<Item = String>) -> Result<CliCommand, String> {
    let mut workspace = PathBuf::from(".");
    let mut entry = TraceEntry {
        agent: "monitor".into(),
        ..TraceEntry::default()
    };

    for arg in args {
        if let Some(value) = arg.strip_prefix("--workspace=") {
            workspace = PathBuf::from(value);
        } else if let Some(value) = arg.strip_prefix("--time=") {
            entry.time = Some(non_empty_arg("--time", value)?.to_string());
        } else if let Some(value) = arg.strip_prefix("--event-id=") {
            entry.event_id = Some(non_empty_arg("--event-id", value)?.to_string());
        } else if let Some(value) = arg.strip_prefix("--agent=") {
            entry.agent = non_empty_arg("--agent", value)?.to_string();
        } else if let Some(value) = arg.strip_prefix("--provider=") {
            entry.provider = Some(non_empty_arg("--provider", value)?.to_string());
        } else if let Some(value) = arg.strip_prefix("--model=") {
            entry.model = Some(non_empty_arg("--model", value)?.to_string());
        } else if let Some(value) = arg.strip_prefix("--session=") {
            entry.session = Some(non_empty_arg("--session", value)?.to_string());
        } else if let Some(value) = arg.strip_prefix("--file=") {
            entry.file = non_empty_arg("--file", value)?.to_string();
        } else if let Some(value) = arg.strip_prefix("--line=") {
            entry.line = Some(parse_positive_u32("--line", value)?);
        } else if let Some(value) = arg.strip_prefix("--line-end=") {
            entry.line_end = Some(parse_positive_u32("--line-end", value)?);
        } else if let Some(value) = arg.strip_prefix("--rationale=") {
            entry.rationale = Some(non_empty_arg("--rationale", value)?.to_string());
        } else if let Some(value) = arg.strip_prefix("--related-event=") {
            entry
                .related_event_ids
                .push(non_empty_arg("--related-event", value)?.to_string());
        } else if let Some(value) = arg.strip_prefix("--requirement-id=") {
            entry
                .requirement_ids
                .push(non_empty_arg("--requirement-id", value)?.to_string());
        } else if let Some(value) = arg.strip_prefix("--requirement=") {
            entry
                .requirement_ids
                .push(non_empty_arg("--requirement", value)?.to_string());
        } else {
            return Err(format!("unknown trace argument: {arg}"));
        }
    }

    if entry.file.trim().is_empty() {
        return Err("trace requires --file=<path>".into());
    }
    if entry
        .rationale
        .as_deref()
        .is_none_or(|value| value.trim().is_empty())
    {
        return Err("trace requires --rationale=<reason>".into());
    }
    if let (Some(line), Some(line_end)) = (entry.line, entry.line_end)
        && line_end < line
    {
        return Err("--line-end must be greater than or equal to --line".into());
    }

    Ok(CliCommand::Trace { workspace, entry })
}

fn non_empty_arg<'a>(name: &str, value: &'a str) -> Result<&'a str, String> {
    let value = value.trim();
    if value.is_empty() {
        Err(format!("{name} must not be empty"))
    } else {
        Ok(value)
    }
}

fn parse_positive_u32(name: &str, value: &str) -> Result<u32, String> {
    let parsed = value
        .parse()
        .map_err(|_| format!("{name} must be a positive integer"))?;
    if parsed == 0 {
        Err(format!("{name} must be a positive integer"))
    } else {
        Ok(parsed)
    }
}

fn parse_requirement_query_args(
    command_name: &str,
    args: impl IntoIterator<Item = String>,
) -> Result<(PathBuf, RequirementGraphQuery), String> {
    let mut workspace = PathBuf::from(".");
    let mut query = RequirementGraphQuery::default();

    for arg in args {
        if let Some(value) = arg.strip_prefix("--workspace=") {
            workspace = PathBuf::from(value);
        } else if let Some(value) = arg.strip_prefix("--status=") {
            query.status = Some(parse_acceptance_coverage_status(value)?);
        } else if let Some(value) = arg.strip_prefix("--requirement=") {
            let value = value.trim();
            if value.is_empty() {
                return Err("--requirement must not be empty".into());
            }
            query.requirement_id = Some(value.to_string());
        } else if let Some(value) = arg.strip_prefix("--requirement-id=") {
            let value = value.trim();
            if value.is_empty() {
                return Err("--requirement-id must not be empty".into());
            }
            query.requirement_id = Some(value.to_string());
        } else if let Some(value) = arg.strip_prefix("--text=") {
            let value = value.trim();
            if value.is_empty() {
                return Err("--text must not be empty".into());
            }
            query.text = Some(value.to_string());
        } else if let Some(value) = arg.strip_prefix("--max-proof-score=") {
            query.max_proof_score = Some(parse_requirement_proof_score(value)?);
        } else if let Some(value) = arg.strip_prefix("--limit=") {
            query.limit = value
                .parse()
                .map_err(|_| "--limit must be a non-negative integer".to_string())?;
        } else {
            return Err(format!("unknown {command_name} argument: {arg}"));
        }
    }

    Ok((workspace, query))
}

fn parse_requirement_proof_score(value: &str) -> Result<u8, String> {
    let value = value
        .trim()
        .parse::<u8>()
        .map_err(|_| "--max-proof-score must be an integer from 0 to 100".to_string())?;
    if value > 100 {
        return Err("--max-proof-score must be an integer from 0 to 100".into());
    }
    Ok(value)
}

fn parse_acceptance_coverage_status(value: &str) -> Result<AcceptanceCoverageStatus, String> {
    match value.trim().to_ascii_lowercase().replace('_', "-").as_str() {
        "covered" => Ok(AcceptanceCoverageStatus::Covered),
        "stale" => Ok(AcceptanceCoverageStatus::Stale),
        "failed" => Ok(AcceptanceCoverageStatus::Failed),
        "unverified" => Ok(AcceptanceCoverageStatus::Unverified),
        "unmapped" => Ok(AcceptanceCoverageStatus::Unmapped),
        _ => Err("--status must be covered, stale, failed, unverified, or unmapped".into()),
    }
}

fn parse_handoff_args(args: impl IntoIterator<Item = String>) -> Result<CliCommand, String> {
    let mut workspace = PathBuf::from(".");
    let mut agent = None;

    for arg in args {
        if let Some(value) = arg.strip_prefix("--workspace=") {
            workspace = PathBuf::from(value);
        } else if let Some(value) = arg.strip_prefix("--agent=") {
            agent = Some(value.parse().map_err(|error| format!("{error}"))?);
        } else {
            return Err(format!("unknown handoff argument: {arg}"));
        }
    }

    let agent = agent
        .ok_or_else(|| "handoff requires --agent=<codex|claude-code|pi|opencode>".to_string())?;
    Ok(CliCommand::Handoff { workspace, agent })
}

fn parse_memory_args(args: impl IntoIterator<Item = String>) -> Result<CliCommand, String> {
    let args = args.into_iter().collect::<Vec<_>>();
    if args.first().is_none_or(|arg| arg != "promote") {
        return Err("memory requires subcommand: promote".into());
    }
    parse_memory_promote_args(args.into_iter().skip(1))
}

fn parse_memory_promote_args(args: impl IntoIterator<Item = String>) -> Result<CliCommand, String> {
    let mut workspace = PathBuf::from(".");
    let mut memory_id = None;
    let mut source = MemorySource::ManualReview;

    for arg in args {
        if let Some(value) = arg.strip_prefix("--workspace=") {
            workspace = PathBuf::from(value);
        } else if let Some(value) = arg.strip_prefix("--memory-id=") {
            memory_id = Some(value.to_string());
        } else if let Some(value) = arg.strip_prefix("--source=") {
            source = parse_trusted_memory_source(value)?;
        } else {
            return Err(format!("unknown memory promote argument: {arg}"));
        }
    }

    let memory_id =
        memory_id.ok_or_else(|| "memory promote requires --memory-id=<id>".to_string())?;
    Ok(CliCommand::MemoryPromote {
        workspace,
        memory_id,
        source,
    })
}

fn parse_trusted_memory_source(value: &str) -> Result<MemorySource, String> {
    match value {
        "manual_review" => Ok(MemorySource::ManualReview),
        "user" => Ok(MemorySource::User),
        "verified_result" => Ok(MemorySource::VerifiedResult),
        "agent_claim" => Err("memory promote requires a trusted source".into()),
        _ => Err("--source must be one of manual_review, user, verified_result".into()),
    }
}

fn parse_repo_audit_args(args: impl IntoIterator<Item = String>) -> Result<CliCommand, String> {
    let mut workspace = PathBuf::from(".");

    for arg in args {
        if let Some(value) = arg.strip_prefix("--workspace=") {
            workspace = PathBuf::from(value);
        } else {
            return Err(format!("unknown repo-audit argument: {arg}"));
        }
    }

    Ok(CliCommand::RepoAudit { workspace })
}

fn parse_verify_args(args: impl IntoIterator<Item = String>) -> Result<CliCommand, String> {
    let mut workspace = PathBuf::from(".");
    let mut verifier_id = None;

    for arg in args {
        if let Some(value) = arg.strip_prefix("--workspace=") {
            workspace = PathBuf::from(value);
        } else if let Some(value) = arg.strip_prefix("--verifier=") {
            verifier_id = Some(value.to_string());
        } else {
            return Err(format!("unknown verify argument: {arg}"));
        }
    }

    let verifier_id = verifier_id.ok_or_else(|| "verify requires --verifier=<id>".to_string())?;
    Ok(CliCommand::Verify {
        workspace,
        verifier_id,
    })
}

fn parse_probe_args(args: impl IntoIterator<Item = String>) -> Result<CliCommand, String> {
    let mut workspace = PathBuf::from(".");

    for arg in args {
        if let Some(value) = arg.strip_prefix("--workspace=") {
            workspace = PathBuf::from(value);
        } else {
            return Err(format!("unknown probe argument: {arg}"));
        }
    }

    Ok(CliCommand::Probe { workspace })
}

fn parse_demo_args(args: impl IntoIterator<Item = String>) -> Result<CliCommand, String> {
    let mut workspace = PathBuf::from("E:/coding-agent-monitor-demo");

    for arg in args {
        if let Some(value) = arg.strip_prefix("--workspace=") {
            workspace = PathBuf::from(value);
        } else {
            return Err(format!("unknown demo argument: {arg}"));
        }
    }

    Ok(CliCommand::Demo { workspace })
}

fn parse_bool(name: &str, value: &str) -> Result<bool, String> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(format!("{name} must be true or false")),
    }
}

fn parse_api_key_env_name(value: &str) -> Result<String, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err("--api-key-env must not be empty".into());
    }
    if value.contains('=') {
        return Err("--api-key-env must be an environment variable name, not KEY=value".into());
    }
    if value.starts_with("sk-") {
        return Err(
            "--api-key-env must name an environment variable, not contain a key value".into(),
        );
    }
    Ok(value.to_string())
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
mod tests {
    use super::*;
    use coding_agent_monitor::{AgentKind, InstallMode};

    #[test]
    fn parses_monitor_command_with_workspace_store() {
        let command = parse_cli([
            "--workspace=.",
            "--retry-limit=3",
            "--fallbacks=claude-code,pi",
        ])
        .expect("parse monitor command");

        match command {
            CliCommand::Monitor { config, workspace } => {
                assert_eq!(workspace, Some(PathBuf::from(".")));
                assert_eq!(config.retry_limit, 3);
                assert_eq!(config.fallback_agents, vec!["claude-code", "pi"]);
            }
            CliCommand::InstallInjection { .. }
            | CliCommand::InjectRunning { .. }
            | CliCommand::Wrap { .. }
            | CliCommand::Judge { .. }
            | CliCommand::Ingest { .. }
            | CliCommand::HookResponse { .. }
            | CliCommand::ConfigAdvisor { .. }
            | CliCommand::ConfigImportLocal { .. }
            | CliCommand::ConfigImportCodingPlanCredentials { .. }
            | CliCommand::Advise { .. }
            | CliCommand::Trail { .. }
            | CliCommand::DevHistory { .. }
            | CliCommand::Calibration { .. }
            | CliCommand::Blame { .. }
            | CliCommand::RepoHunks { .. }
            | CliCommand::Requirements { .. }
            | CliCommand::CompletionCertificate { .. }
            | CliCommand::Trace { .. }
            | CliCommand::Handoff { .. }
            | CliCommand::MemoryPromote { .. }
            | CliCommand::RepoAudit { .. }
            | CliCommand::Verify { .. }
            | CliCommand::Probe { .. }
            | CliCommand::Demo { .. } => {
                panic!("expected monitor command")
            }
        }
    }

    #[test]
    fn monitor_command_rejects_empty_fallbacks() {
        let error = parse_cli(["--fallbacks=,,"]).expect_err("empty fallbacks should be rejected");

        assert!(error.contains("--fallbacks must include at least one agent"));
    }

    #[test]
    fn parses_inject_command() {
        let command = parse_cli([
            "inject",
            "--agent=claude-code",
            "--workspace=.",
            "--overwrite=true",
        ])
        .expect("parse inject command");

        match command {
            CliCommand::InstallInjection {
                agent,
                workspace,
                mode,
            } => {
                assert_eq!(agent, AgentKind::ClaudeCode);
                assert_eq!(workspace, PathBuf::from("."));
                assert_eq!(mode, InstallMode::CreateOrOverwrite);
            }
            CliCommand::Monitor { .. }
            | CliCommand::InjectRunning { .. }
            | CliCommand::Wrap { .. }
            | CliCommand::Judge { .. }
            | CliCommand::Ingest { .. }
            | CliCommand::HookResponse { .. }
            | CliCommand::ConfigAdvisor { .. }
            | CliCommand::ConfigImportLocal { .. }
            | CliCommand::ConfigImportCodingPlanCredentials { .. }
            | CliCommand::Advise { .. }
            | CliCommand::Trail { .. }
            | CliCommand::DevHistory { .. }
            | CliCommand::Calibration { .. }
            | CliCommand::Blame { .. }
            | CliCommand::RepoHunks { .. }
            | CliCommand::Requirements { .. }
            | CliCommand::CompletionCertificate { .. }
            | CliCommand::Trace { .. }
            | CliCommand::Handoff { .. }
            | CliCommand::MemoryPromote { .. }
            | CliCommand::RepoAudit { .. }
            | CliCommand::Verify { .. }
            | CliCommand::Probe { .. }
            | CliCommand::Demo { .. } => {
                panic!("expected injection command")
            }
        }
    }

    #[test]
    fn inject_command_defaults_to_managed_block_merge() {
        let command =
            parse_cli(["inject", "--agent=codex", "--workspace=."]).expect("parse inject command");

        match command {
            CliCommand::InstallInjection { mode, .. } => {
                assert_eq!(mode, InstallMode::MergeManagedBlock);
            }
            _ => panic!("expected injection command"),
        }
    }

    #[test]
    fn parses_inject_running_command() {
        let command = parse_cli(["inject-running", "--workspace=.", "--overwrite=true"])
            .expect("parse inject-running command");

        match command {
            CliCommand::InjectRunning {
                workspace, mode, ..
            } => {
                assert_eq!(workspace, Some(PathBuf::from(".")));
                assert_eq!(mode, InstallMode::CreateOrOverwrite);
            }
            _ => panic!("expected inject-running command"),
        }
    }

    #[test]
    fn inject_running_defaults_to_managed_block_merge() {
        let command =
            parse_cli(["inject-running", "--workspace=."]).expect("parse inject-running command");

        match command {
            CliCommand::InjectRunning { mode, .. } => {
                assert_eq!(mode, InstallMode::MergeManagedBlock);
            }
            _ => panic!("expected inject-running command"),
        }
    }

    #[test]
    fn inject_running_without_workspace_uses_process_cwd_later() {
        let command = parse_cli(["inject-running"]).expect("parse inject-running command");

        match command {
            CliCommand::InjectRunning {
                workspace, mode, ..
            } => {
                assert_eq!(workspace, None);
                assert_eq!(mode, InstallMode::MergeManagedBlock);
            }
            _ => panic!("expected inject-running command"),
        }
    }

    #[test]
    fn inject_running_defaults_to_dry_run() {
        let command = parse_cli(["inject-running"]).expect("parse inject-running command");

        match command {
            CliCommand::InjectRunning { apply, .. } => {
                assert!(!apply);
            }
            _ => panic!("expected inject-running command"),
        }
    }

    #[test]
    fn inject_running_apply_flag_enables_writes() {
        let command =
            parse_cli(["inject-running", "--apply"]).expect("parse inject-running command");

        match command {
            CliCommand::InjectRunning { apply, .. } => {
                assert!(apply);
            }
            _ => panic!("expected inject-running command"),
        }
    }

    #[test]
    fn injection_workspace_prefers_explicit_workspace_then_process_cwd() {
        let agent = coding_agent_monitor::RunningAgent::new(1, AgentKind::Codex, "codex.exe")
            .with_cwd(Some(PathBuf::from("F:/agent-repo")));

        assert_eq!(
            injection_workspace_for(&agent, Some(&PathBuf::from("F:/explicit"))),
            Some(PathBuf::from("F:/explicit"))
        );
        assert_eq!(
            injection_workspace_for(&agent, None),
            Some(PathBuf::from("F:/agent-repo"))
        );
    }

    #[test]
    fn injection_workspace_skips_agent_support_directories_without_explicit_workspace() {
        let codex_cache = coding_agent_monitor::RunningAgent::new(1, AgentKind::Codex, "codex.exe")
            .with_cwd(Some(PathBuf::from(
                "C:/Users/yys/.codex/plugins/cache/openai-bundled/chrome",
            )));
        let claude_skill =
            coding_agent_monitor::RunningAgent::new(2, AgentKind::ClaudeCode, "node.exe").with_cwd(
                Some(PathBuf::from("C:/Users/yys/.claude/skills/ppt-polish")),
            );

        assert_eq!(injection_workspace_for(&codex_cache, None), None);
        assert_eq!(injection_workspace_for(&claude_skill, None), None);
        assert_eq!(
            injection_workspace_for(&codex_cache, Some(&PathBuf::from("F:/real-project"))),
            Some(PathBuf::from("F:/real-project"))
        );
    }

    #[test]
    fn parses_wrap_command_after_double_dash() {
        let command = parse_cli([
            "wrap",
            "--agent=codex",
            "--workspace=.",
            "--session=s1",
            "--",
            "codex",
            "exec",
        ])
        .expect("parse wrap command");

        match command {
            CliCommand::Wrap {
                agent,
                workspace,
                session,
                command,
            } => {
                assert_eq!(agent, AgentKind::Codex);
                assert_eq!(workspace, PathBuf::from("."));
                assert_eq!(session.as_deref(), Some("s1"));
                assert_eq!(command, vec!["codex", "exec"]);
            }
            _ => panic!("expected wrap command"),
        }
    }

    #[test]
    fn parses_judge_command_with_external_command_after_separator() {
        let command = parse_cli([
            "judge",
            "--workspace=E:/demo",
            "--write-intervention=true",
            "--",
            "claude",
            "-p",
            "review",
        ])
        .expect("parse judge command");

        match command {
            CliCommand::Judge {
                workspace,
                write_intervention,
                external_command,
            } => {
                assert_eq!(workspace, PathBuf::from("E:/demo"));
                assert!(write_intervention);
                assert_eq!(external_command, vec!["claude", "-p", "review"]);
            }
            _ => panic!("expected judge command"),
        }
    }

    #[test]
    fn judge_review_intervention_preserves_spawn_judge_action() {
        let report = AgentReviewReport {
            workspace: "E:/demo".into(),
            status: coding_agent_monitor::AgentReviewStatus::Intervene,
            findings: vec![coding_agent_monitor::AgentReviewFinding {
                severity: coding_agent_monitor::DashboardSeverity::Critical,
                category: "suspicious_untraced_change".into(),
                agent: Some("codex".into()),
                evidence: "src/lib.rs has dirty git hunks without trace evidence".into(),
                recommended_action: AgentReviewAction::SpawnJudgeAgent,
            }],
        };

        let interventions = interventions_from_review(&report);

        assert_eq!(interventions.len(), 1);
        assert_eq!(
            interventions[0].kind,
            coding_agent_monitor::InterventionKind::SuspiciousChange
        );
        assert_eq!(
            interventions[0].action,
            coding_agent_monitor::Action::SpawnJudgeAgent
        );
    }

    #[test]
    fn external_judge_prompt_is_bounded_and_evidence_first() {
        assert!(EXTERNAL_JUDGE_PROMPT.contains("Output exactly one line"));
        assert!(
            EXTERNAL_JUDGE_PROMPT.contains("continue | force_verification | handoff | restart")
        );
        assert!(EXTERNAL_JUDGE_PROMPT.contains("evidence=<ids/files/tests>"));
        assert!(EXTERNAL_JUDGE_PROMPT.contains("Judge the control loop"));
        assert!(EXTERNAL_JUDGE_PROMPT.contains("unverified completion"));
        assert!(EXTERNAL_JUDGE_PROMPT.contains("stale verification"));
        assert!(EXTERNAL_JUDGE_PROMPT.contains("intended-environment validation"));
        assert!(EXTERNAL_JUDGE_PROMPT.contains("mobile/native/system/ML"));
        assert!(EXTERNAL_JUDGE_PROMPT.contains("Prefer force_verification over handoff"));
        assert!(EXTERNAL_JUDGE_PROMPT.contains("Do not propose broad refactors"));
    }

    #[test]
    fn parses_advise_command() {
        let command = parse_cli(["advise", "--workspace=E:/demo"]).expect("parse advise command");

        match command {
            CliCommand::Advise { workspace } => {
                assert_eq!(workspace, PathBuf::from("E:/demo"));
            }
            _ => panic!("expected advise command"),
        }
    }

    #[test]
    fn parses_trace_command_with_requirement_binding() {
        let command = parse_cli([
            "trace",
            "--workspace=E:/demo",
            "--agent=codex",
            "--session=s1",
            "--event-id=evt-trace-proof",
            "--file=src/lib.rs",
            "--line=10",
            "--line-end=18",
            "--rationale=Implement project-contract proof mapping.",
            "--related-event=evt-user-goal",
            "--requirement-id=req-contract-every-meaningful-change",
        ])
        .expect("parse trace command");

        match command {
            CliCommand::Trace { workspace, entry } => {
                assert_eq!(workspace, PathBuf::from("E:/demo"));
                assert_eq!(entry.agent, "codex");
                assert_eq!(entry.session.as_deref(), Some("s1"));
                assert_eq!(entry.event_id.as_deref(), Some("evt-trace-proof"));
                assert_eq!(entry.file, "src/lib.rs");
                assert_eq!(entry.line, Some(10));
                assert_eq!(entry.line_end, Some(18));
                assert_eq!(
                    entry.rationale.as_deref(),
                    Some("Implement project-contract proof mapping.")
                );
                assert_eq!(entry.related_event_ids, vec!["evt-user-goal".to_string()]);
                assert_eq!(
                    entry.requirement_ids,
                    vec!["req-contract-every-meaningful-change".to_string()]
                );
            }
            _ => panic!("expected trace command"),
        }
    }

    #[test]
    fn trace_command_requires_rationale() {
        let error = parse_cli(["trace", "--file=src/lib.rs"])
            .expect_err("trace command should require rationale");

        assert!(error.contains("trace requires --rationale"));
    }

    #[test]
    fn parses_ingest_command() {
        let command = parse_cli([
            "ingest",
            "--adapter=codex",
            "--workspace=E:/demo",
            "--session=codex-live",
            "--retry-limit=0",
            "--fallbacks=pi,opencode",
        ])
        .expect("parse ingest command");

        match command {
            CliCommand::Ingest {
                adapter,
                workspace,
                session,
                config,
            } => {
                assert_eq!(adapter, AgentKind::Codex);
                assert_eq!(workspace, PathBuf::from("E:/demo"));
                assert_eq!(session.as_deref(), Some("codex-live"));
                assert_eq!(config.retry_limit, 0);
                assert_eq!(config.fallback_agents, vec!["pi", "opencode"]);
            }
            _ => panic!("expected ingest command"),
        }
    }

    #[test]
    fn ingest_command_rejects_empty_fallbacks() {
        let error = parse_cli(["ingest", "--adapter=codex", "--fallbacks=,,"])
            .expect_err("empty fallbacks should be rejected");

        assert!(error.contains("--fallbacks must include at least one agent"));
    }

    #[test]
    fn parses_hook_response_command() {
        let command = parse_cli([
            "hook-response",
            "--adapter=opencode",
            "--workspace=E:/demo",
            "--session=open-live",
            "--format=generic",
        ])
        .expect("parse hook-response command");

        match command {
            CliCommand::HookResponse {
                adapter,
                workspace,
                session,
                format,
            } => {
                assert_eq!(adapter, AgentKind::OpenCode);
                assert_eq!(workspace, PathBuf::from("E:/demo"));
                assert_eq!(session.as_deref(), Some("open-live"));
                assert_eq!(format, HookResponseFormat::Generic);
            }
            _ => panic!("expected hook-response command"),
        }
    }

    #[test]
    fn parses_claude_code_hook_response_format() {
        let command = parse_cli([
            "hook-response",
            "--adapter=claude-code",
            "--format=claude-code",
        ])
        .expect("parse hook-response command");

        match command {
            CliCommand::HookResponse { format, .. } => {
                assert_eq!(format, HookResponseFormat::ClaudeCode);
            }
            _ => panic!("expected hook-response command"),
        }
    }

    #[test]
    fn parses_codex_hook_response_format() {
        let command = parse_cli(["hook-response", "--adapter=codex", "--format=codex"])
            .expect("parse hook-response command");

        match command {
            CliCommand::HookResponse { format, .. } => {
                assert_eq!(format, HookResponseFormat::Codex);
            }
            _ => panic!("expected hook-response command"),
        }
    }

    #[test]
    fn parses_opencode_hook_response_format() {
        let command = parse_cli(["hook-response", "--adapter=opencode", "--format=opencode"])
            .expect("parse hook-response command");

        match command {
            CliCommand::HookResponse { format, .. } => {
                assert_eq!(format, HookResponseFormat::OpenCode);
            }
            _ => panic!("expected hook-response command"),
        }
    }

    #[test]
    fn hook_response_blocks_protected_path_from_workspace_security() {
        let workspace = tempfile::tempdir().expect("workspace");
        std::fs::create_dir_all(workspace.path().join(".agent-monitor")).expect("store");
        std::fs::write(
            workspace.path().join(".agent-monitor").join("config.json"),
            r#"{
              "security": {
                "protected_paths": ["protected/**"]
              }
            }"#,
        )
        .expect("config");
        let input = br#"{"event":"tool.execute.before","session":{"id":"open-live"},"tool":"edit","input":{"path":"protected/deploy.yml"}}"#;
        let mut output = Vec::new();

        run_hook_response(
            &input[..],
            &mut output,
            AgentKind::OpenCode,
            workspace.path(),
            None,
            HookResponseFormat::Generic,
        )
        .expect("hook response");

        let response: coding_agent_monitor::AdapterHookResponse =
            serde_json::from_slice(&output).expect("response json");
        assert_eq!(
            response.decision,
            coding_agent_monitor::AdapterHookDecision::Block
        );
        assert_eq!(response.session.as_deref(), Some("open-live"));
        assert!(response.reason.unwrap().contains("protected path"));
    }

    #[test]
    fn hook_response_persists_attempt_and_block_decision() {
        let workspace = tempfile::tempdir().expect("workspace");
        std::fs::create_dir_all(workspace.path().join(".agent-monitor")).expect("store");
        std::fs::write(
            workspace.path().join(".agent-monitor").join("config.json"),
            r#"{
              "security": {
                "protected_paths": ["protected/**"]
              }
            }"#,
        )
        .expect("config");
        let input = br#"{"event":"tool.execute.before","id":"hook-attempt-1","session":{"id":"open-live"},"tool":"edit","input":{"path":"protected/deploy.yml"}}"#;
        let mut output = Vec::new();

        run_hook_response(
            &input[..],
            &mut output,
            AgentKind::OpenCode,
            workspace.path(),
            None,
            HookResponseFormat::Generic,
        )
        .expect("hook response");

        let events =
            std::fs::read_to_string(workspace.path().join(".agent-monitor").join("events.jsonl"))
                .expect("events");
        let events = events
            .lines()
            .map(|line| serde_json::from_str::<coding_agent_monitor::Event>(line).expect("event"))
            .collect::<Vec<_>>();

        assert_eq!(events.len(), 2);
        assert_eq!(events[0].kind, coding_agent_monitor::EventKind::ToolCall);
        assert_eq!(events[0].event_id.as_deref(), Some("hook-attempt-1"));
        assert_eq!(
            events[0].command.as_deref(),
            Some("edit protected/deploy.yml")
        );
        assert_eq!(
            events[1].kind,
            coding_agent_monitor::EventKind::InterventionResult
        );
        assert_eq!(
            events[1].event_id.as_deref(),
            Some("hook-attempt-1:hook-response")
        );
        assert_eq!(events[1].related_event_ids, vec!["hook-attempt-1"]);
        assert!(
            events[1]
                .content
                .as_deref()
                .is_some_and(|content| content.contains("protected path"))
        );
    }

    #[test]
    fn hook_response_persists_attempt_and_ask_decision() {
        let workspace = tempfile::tempdir().expect("workspace");
        let input = br#"{"hook_event_name":"PreToolUse","id":"hook-ask-1","session_id":"claude-live","decision":"ask","reason":"needs approval for protected workflow edit","tool_name":"Edit","tool_input":{"file_path":"protected/deploy.yml"}}"#;
        let mut output = Vec::new();

        run_hook_response(
            &input[..],
            &mut output,
            AgentKind::ClaudeCode,
            workspace.path(),
            None,
            HookResponseFormat::Generic,
        )
        .expect("hook response");

        let response: coding_agent_monitor::AdapterHookResponse =
            serde_json::from_slice(&output).expect("response json");
        assert_eq!(
            response.decision,
            coding_agent_monitor::AdapterHookDecision::Ask
        );

        let events =
            std::fs::read_to_string(workspace.path().join(".agent-monitor").join("events.jsonl"))
                .expect("events");
        let events = events
            .lines()
            .map(|line| serde_json::from_str::<coding_agent_monitor::Event>(line).expect("event"))
            .collect::<Vec<_>>();

        assert_eq!(events.len(), 2);
        assert_eq!(
            events[1].kind,
            coding_agent_monitor::EventKind::InterventionResult
        );
        assert_eq!(
            events[1].event_id.as_deref(),
            Some("hook-ask-1:hook-response")
        );
        assert!(
            events[1].content.as_deref().is_some_and(
                |content| content.contains("hook response requested user authorization")
            )
        );
    }

    #[test]
    fn claude_code_hook_response_blocks_write_under_read_only_judge_packet() {
        let workspace = tempfile::tempdir().expect("workspace");
        let mut store = ProjectStore::open(workspace.path()).expect("store");
        store
            .append_event(&Event {
                event_id: Some("session-started".into()),
                agent: agent_kind_label(AgentKind::ClaudeCode).into(),
                session: Some("claude-live".into()),
                agent_session_id: Some("claude-live".into()),
                kind: EventKind::AgentHealth,
                content: Some("session started".into()),
                ..Event::default()
            })
            .expect("append precondition event");
        store
            .append_event(&Event {
                event_id: Some("evidence-1".into()),
                agent: agent_kind_label(AgentKind::ClaudeCode).into(),
                session: Some("claude-live".into()),
                agent_session_id: Some("claude-live".into()),
                kind: EventKind::ModelMessage,
                content: Some("read-only judge evidence".into()),
                ..Event::default()
            })
            .expect("append packet evidence");
        store
            .write_control_packet(&coding_agent_monitor::ControlPacket {
                packet_id: "packet-read-only-judge".into(),
                target_agent: agent_kind_label(AgentKind::ClaudeCode).into(),
                urgency: coding_agent_monitor::PacketUrgency::Context,
                title: "Read-only judge review required".into(),
                summary: "The monitor selected a read-only judge review.".into(),
                instructions: vec![coding_agent_monitor::PacketInstruction {
                    priority: coding_agent_monitor::PacketInstructionPriority::Must,
                    text: "Act as a read-only judge and inspect the evidence without editing."
                        .into(),
                }],
                evidence_refs: vec!["evidence-1".into()],
                forbidden: vec![
                    "Do not edit files or mutate the worktree during judge review.".into(),
                    "Do not run destructive commands or apply patches.".into(),
                ],
                success_criteria: vec!["Judge report returned without file changes.".into()],
                preconditions: coding_agent_monitor::PacketPreconditions {
                    adapter: Some(agent_kind_label(AgentKind::ClaudeCode).into()),
                    agent_session_id: Some("claude-live".into()),
                    ..Default::default()
                },
            })
            .expect("write control packet");
        let input = br#"{"hook_event_name":"PreToolUse","session_id":"claude-live","tool_name":"Write","tool_input":{"file_path":"src/lib.rs"}}"#;
        let mut output = Vec::new();

        run_hook_response(
            &input[..],
            &mut output,
            AgentKind::ClaudeCode,
            workspace.path(),
            None,
            HookResponseFormat::ClaudeCode,
        )
        .expect("hook response");

        let value: serde_json::Value = serde_json::from_slice(&output).expect("response json");
        assert_eq!(
            value.pointer("/hookSpecificOutput/permissionDecision"),
            Some(&serde_json::Value::String("deny".into()))
        );
        assert!(
            value
                .pointer("/hookSpecificOutput/permissionDecisionReason")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|reason| reason.contains("read-only judge"))
        );

        let events =
            std::fs::read_to_string(workspace.path().join(".agent-monitor").join("events.jsonl"))
                .expect("events");
        assert!(
            events
                .lines()
                .filter_map(|line| serde_json::from_str::<Event>(line).ok())
                .any(|event| event.kind == EventKind::InterventionResult
                    && event
                        .content
                        .as_deref()
                        .is_some_and(|content| content.contains("read-only judge")))
        );
    }

    #[test]
    fn claude_code_hook_response_ignores_read_only_judge_packet_after_git_head_moves() {
        let workspace = tempfile::tempdir().expect("workspace");
        git(workspace.path(), ["init"]);
        git(
            workspace.path(),
            ["config", "user.email", "monitor@example.test"],
        );
        git(workspace.path(), ["config", "user.name", "Monitor Test"]);
        std::fs::create_dir_all(workspace.path().join("src")).expect("src dir");
        std::fs::write(
            workspace.path().join("src").join("lib.rs"),
            "pub fn one() {}\n",
        )
        .expect("initial source");
        git(workspace.path(), ["add", "src/lib.rs"]);
        git(workspace.path(), ["commit", "-m", "initial"]);
        let initial_head = git_stdout(workspace.path(), ["rev-parse", "HEAD"]);

        let mut store = ProjectStore::open(workspace.path()).expect("store");
        store
            .append_event(&Event {
                event_id: Some("session-started".into()),
                agent: agent_kind_label(AgentKind::ClaudeCode).into(),
                session: Some("claude-live".into()),
                agent_session_id: Some("claude-live".into()),
                kind: EventKind::AgentHealth,
                content: Some("session started".into()),
                ..Event::default()
            })
            .expect("append precondition event");
        store
            .append_event(&Event {
                event_id: Some("evidence-1".into()),
                agent: agent_kind_label(AgentKind::ClaudeCode).into(),
                session: Some("claude-live".into()),
                agent_session_id: Some("claude-live".into()),
                kind: EventKind::ModelMessage,
                content: Some("read-only judge evidence".into()),
                ..Event::default()
            })
            .expect("append packet evidence");
        store
            .write_control_packet(&coding_agent_monitor::ControlPacket {
                packet_id: "packet-stale-read-only-judge".into(),
                target_agent: agent_kind_label(AgentKind::ClaudeCode).into(),
                urgency: coding_agent_monitor::PacketUrgency::Context,
                title: "Read-only judge review required".into(),
                summary: "The monitor selected a read-only judge review.".into(),
                instructions: vec![coding_agent_monitor::PacketInstruction {
                    priority: coding_agent_monitor::PacketInstructionPriority::Must,
                    text: "Act as a read-only judge and inspect the evidence without editing."
                        .into(),
                }],
                evidence_refs: vec!["evidence-1".into()],
                forbidden: vec![
                    "Do not edit files or mutate the worktree during judge review.".into(),
                    "Do not run destructive commands or apply patches.".into(),
                ],
                success_criteria: vec!["Judge report returned without file changes.".into()],
                preconditions: coding_agent_monitor::PacketPreconditions {
                    git_head: Some(initial_head),
                    adapter: Some(agent_kind_label(AgentKind::ClaudeCode).into()),
                    agent_session_id: Some("claude-live".into()),
                    ..Default::default()
                },
            })
            .expect("write control packet at initial head");

        std::fs::write(
            workspace.path().join("src").join("lib.rs"),
            "pub fn two() {}\n",
        )
        .expect("advance source");
        git(workspace.path(), ["add", "src/lib.rs"]);
        git(workspace.path(), ["commit", "-m", "advance head"]);
        let input = br#"{"hook_event_name":"PreToolUse","session_id":"claude-live","tool_name":"Write","tool_input":{"file_path":"src/lib.rs"}}"#;
        let mut output = Vec::new();

        run_hook_response(
            &input[..],
            &mut output,
            AgentKind::ClaudeCode,
            workspace.path(),
            None,
            HookResponseFormat::ClaudeCode,
        )
        .expect("hook response");

        assert!(
            output.is_empty(),
            "Claude Code allow responses should not emit a blocking payload: {}",
            String::from_utf8_lossy(&output)
        );
        let events =
            std::fs::read_to_string(workspace.path().join(".agent-monitor").join("events.jsonl"))
                .expect("events");
        assert!(
            !events
                .lines()
                .filter_map(|line| serde_json::from_str::<Event>(line).ok())
                .any(|event| event.kind == EventKind::InterventionResult
                    && event
                        .content
                        .as_deref()
                        .is_some_and(|content| content.contains("read-only judge"))),
            "stale packet should not produce read-only judge intervention: {events}"
        );
    }

    #[test]
    fn claude_code_hook_response_format_renders_native_denial() {
        let workspace = tempfile::tempdir().expect("workspace");
        let input = br#"{"hook_event_name":"PreToolUse","session_id":"claude-live","tool_name":"Bash","tool_input":{"command":"git clean -fdx"}}"#;
        let mut output = Vec::new();

        run_hook_response(
            &input[..],
            &mut output,
            AgentKind::ClaudeCode,
            workspace.path(),
            None,
            HookResponseFormat::ClaudeCode,
        )
        .expect("hook response");

        let value: serde_json::Value = serde_json::from_slice(&output).expect("response json");
        assert_eq!(
            value.pointer("/hookSpecificOutput/hookEventName"),
            Some(&serde_json::Value::String("PreToolUse".into()))
        );
        assert_eq!(
            value.pointer("/hookSpecificOutput/permissionDecision"),
            Some(&serde_json::Value::String("deny".into()))
        );
        assert!(
            value
                .pointer("/hookSpecificOutput/permissionDecisionReason")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|reason| reason.contains("git clean -fdx"))
        );
    }

    #[test]
    fn claude_code_hook_response_format_renders_native_ask() {
        let workspace = tempfile::tempdir().expect("workspace");
        let input = br#"{"hook_event_name":"PreToolUse","session_id":"claude-live","decision":"ask","reason":"needs approval for destructive command","tool_name":"Bash","tool_input":{"command":"git clean -fdx"}}"#;
        let mut output = Vec::new();

        run_hook_response(
            &input[..],
            &mut output,
            AgentKind::ClaudeCode,
            workspace.path(),
            None,
            HookResponseFormat::ClaudeCode,
        )
        .expect("hook response");

        let value: serde_json::Value = serde_json::from_slice(&output).expect("response json");
        assert_eq!(
            value.pointer("/hookSpecificOutput/hookEventName"),
            Some(&serde_json::Value::String("PreToolUse".into()))
        );
        assert_eq!(
            value.pointer("/hookSpecificOutput/permissionDecision"),
            Some(&serde_json::Value::String("ask".into()))
        );
        assert!(
            value
                .pointer("/hookSpecificOutput/permissionDecisionReason")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|reason| reason.contains("needs approval"))
        );
    }

    #[test]
    fn claude_code_hook_response_format_emits_no_output_on_allow() {
        let workspace = tempfile::tempdir().expect("workspace");
        let input = br#"{"hook_event_name":"PreToolUse","session_id":"claude-live","tool_name":"Read","tool_input":{"file_path":"src/lib.rs"}}"#;
        let mut output = Vec::new();

        run_hook_response(
            &input[..],
            &mut output,
            AgentKind::ClaudeCode,
            workspace.path(),
            None,
            HookResponseFormat::ClaudeCode,
        )
        .expect("hook response");

        assert!(output.is_empty());
    }

    #[test]
    fn codex_hook_response_format_renders_native_denial() {
        let workspace = tempfile::tempdir().expect("workspace");
        let input = br#"{"hook_event_name":"PreToolUse","session_id":"codex-live","tool_name":"Shell","tool_input":{"command":"git clean -fdx"}}"#;
        let mut output = Vec::new();

        run_hook_response(
            &input[..],
            &mut output,
            AgentKind::Codex,
            workspace.path(),
            None,
            HookResponseFormat::Codex,
        )
        .expect("hook response");

        let value: serde_json::Value = serde_json::from_slice(&output).expect("response json");
        assert_eq!(
            value.pointer("/hookSpecificOutput/hookEventName"),
            Some(&serde_json::Value::String("PreToolUse".into()))
        );
        assert_eq!(
            value.pointer("/hookSpecificOutput/permissionDecision"),
            Some(&serde_json::Value::String("deny".into()))
        );
        assert!(
            value
                .pointer("/hookSpecificOutput/permissionDecisionReason")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|reason| reason.contains("git clean -fdx"))
        );
    }

    #[test]
    fn codex_hook_response_format_emits_no_output_on_allow() {
        let workspace = tempfile::tempdir().expect("workspace");
        let input = br#"{"hook_event_name":"PreToolUse","session_id":"codex-live","tool_name":"Read","tool_input":{"file_path":"src/lib.rs"}}"#;
        let mut output = Vec::new();

        run_hook_response(
            &input[..],
            &mut output,
            AgentKind::Codex,
            workspace.path(),
            None,
            HookResponseFormat::Codex,
        )
        .expect("hook response");

        assert!(output.is_empty());
    }

    #[test]
    fn opencode_hook_response_format_renders_native_denial() {
        let workspace = tempfile::tempdir().expect("workspace");
        let input = br#"{"event":"tool.execute.before","session":{"id":"open-live"},"tool":"bash","input":{"command":"git clean -fdx"}}"#;
        let mut output = Vec::new();

        run_hook_response(
            &input[..],
            &mut output,
            AgentKind::OpenCode,
            workspace.path(),
            None,
            HookResponseFormat::OpenCode,
        )
        .expect("hook response");

        let value: serde_json::Value = serde_json::from_slice(&output).expect("response json");
        assert_eq!(
            value.pointer("/action"),
            Some(&serde_json::Value::String("block".into()))
        );
        assert_eq!(
            value.pointer("/decision"),
            Some(&serde_json::Value::String("block".into()))
        );
        assert!(!value.pointer("/hookSpecificOutput").is_some());
        assert!(
            value
                .pointer("/message")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|reason| reason.contains("git clean -fdx"))
        );
    }

    #[test]
    fn opencode_hook_response_format_emits_no_output_on_allow() {
        let workspace = tempfile::tempdir().expect("workspace");
        let input = br#"{"event":"tool.execute.before","session":{"id":"open-live"},"tool":"read","input":{"path":"src/lib.rs"}}"#;
        let mut output = Vec::new();

        run_hook_response(
            &input[..],
            &mut output,
            AgentKind::OpenCode,
            workspace.path(),
            None,
            HookResponseFormat::OpenCode,
        )
        .expect("hook response");

        assert!(output.is_empty());
    }

    #[test]
    fn hook_response_rejects_adapter_without_blocking_capability() {
        let workspace = tempfile::tempdir().expect("workspace");
        let input = br#"{"event":"pi.wrapper.command_started","command":"git clean -fdx"}"#;
        let mut output = Vec::new();

        let error = run_hook_response(
            &input[..],
            &mut output,
            AgentKind::Pi,
            workspace.path(),
            None,
            HookResponseFormat::Generic,
        )
        .expect_err("Pi cannot consume hook block responses by default");

        assert!(error.contains("does not support pre-tool blocking"));
        assert!(output.is_empty());
    }

    #[test]
    fn hook_response_rejects_disabled_adapter() {
        let workspace = tempfile::tempdir().expect("workspace");
        std::fs::create_dir_all(workspace.path().join(".agent-monitor")).expect("store");
        std::fs::write(
            workspace.path().join(".agent-monitor").join("config.json"),
            r#"{
              "adapters": {
                "claude_code": { "enabled": false }
              }
            }"#,
        )
        .expect("config");
        let input = br#"{"hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{"command":"git clean -fdx"}}"#;
        let mut output = Vec::new();

        let error = run_hook_response(
            &input[..],
            &mut output,
            AgentKind::ClaudeCode,
            workspace.path(),
            None,
            HookResponseFormat::ClaudeCode,
        )
        .expect_err("disabled adapter should not produce hook responses");

        assert!(error.contains("disabled"));
        assert!(output.is_empty());
    }

    #[test]
    fn parses_config_advisor_command() {
        let command = parse_cli([
            "config",
            "advisor",
            "--workspace=E:/demo",
            "--endpoint=https://api.example.test/v1/chat/completions",
            "--model=gpt-5.5",
            "--api-key-env=CAM_ADVISOR_KEY",
        ])
        .expect("parse config advisor command");

        match command {
            CliCommand::ConfigAdvisor { workspace, update } => {
                assert_eq!(workspace, PathBuf::from("E:/demo"));
                assert_eq!(
                    update.endpoint,
                    "https://api.example.test/v1/chat/completions"
                );
                assert_eq!(update.model, "gpt-5.5");
                assert_eq!(update.api_key_env, "CAM_ADVISOR_KEY");
                assert_eq!(update.credential_source, None);
                assert_eq!(update.credential_file, None);
                assert!(update.enabled);
            }
            _ => panic!("expected config advisor command"),
        }
    }

    #[test]
    fn parses_config_advisor_command_with_coding_plan_credentials() {
        let command = parse_cli([
            "config",
            "advisor",
            "--workspace=E:/demo",
            "--endpoint=https://api.openai.com/v1/chat/completions",
            "--model=gpt-5.5",
            "--credential-source=coding-plan",
            "--credential-file=credentials/coding-plan/auth.json",
        ])
        .expect("parse config advisor command");

        match command {
            CliCommand::ConfigAdvisor { workspace, update } => {
                assert_eq!(workspace, PathBuf::from("E:/demo"));
                assert_eq!(
                    update.endpoint,
                    "https://api.openai.com/v1/chat/completions"
                );
                assert_eq!(update.model, "gpt-5.5");
                assert_eq!(update.api_key_env, "OPENAI_API_KEY");
                assert_eq!(
                    update.credential_source,
                    Some(AdvisorCredentialSource::CodingPlan)
                );
                assert_eq!(
                    update.credential_file.as_deref(),
                    Some("credentials/coding-plan/auth.json")
                );
            }
            _ => panic!("expected config advisor command"),
        }
    }

    #[test]
    fn config_advisor_command_requires_api_key_env_name() {
        let error = parse_cli([
            "config",
            "advisor",
            "--endpoint=https://api.example.test/v1/chat/completions",
            "--model=gpt-5.5",
        ])
        .expect_err("missing api key env should fail");

        assert!(error.contains("--api-key-env"));
    }

    #[test]
    fn config_advisor_command_requires_credential_file_for_coding_plan_source() {
        let error = parse_cli([
            "config",
            "advisor",
            "--endpoint=https://api.openai.com/v1/chat/completions",
            "--model=gpt-5.5",
            "--credential-source=coding-plan",
        ])
        .expect_err("missing credential file should fail");

        assert!(error.contains("--credential-file"));
    }

    #[test]
    fn config_advisor_command_rejects_claude_plan_credential_source() {
        let error = parse_cli([
            "config",
            "advisor",
            "--endpoint=https://api.example.test/v1/messages",
            "--model=claude-opus-4",
            "--credential-source=claude-plan",
            "--credential-file=credentials/advisor.json",
        ])
        .expect_err("claude-plan should not be accepted as an advisor credential source");

        assert!(error.contains("coding-plan"));
        assert!(!error.contains("claude-plan"));
    }

    #[test]
    fn config_advisor_command_rejects_codex_plan_credential_source_alias() {
        let error = parse_cli([
            "config",
            "advisor",
            "--endpoint=https://api.openai.com/v1/chat/completions",
            "--model=gpt-5.5",
            "--credential-source=codex-plan",
            "--credential-file=credentials/coding-plan/auth.json",
        ])
        .expect_err("codex-plan should not be accepted as a credential source");

        assert!(error.contains("coding-plan"));
        assert!(!error.contains("codex-plan"));
    }

    #[test]
    fn config_import_local_rejects_copy_credentials_flag() {
        let error = parse_cli([
            "config",
            "import-local",
            "--workspace=E:/demo",
            "--home=C:/Users/yys",
            "--codex=true",
            "--claude-code=false",
            "--copy-credentials=true",
        ])
        .expect_err("copying local cli credentials should be rejected");

        assert!(error.contains("--advisor-credential-file"));
        assert!(error.contains("dedicated advisor credentials"));
    }

    #[test]
    fn parses_config_import_local_command_with_dedicated_advisor_credentials() {
        let command = parse_cli([
            "config",
            "import-local",
            "--workspace=E:/demo",
            "--home=C:/Users/yys",
            "--advisor-credential-source=coding-plan",
            "--advisor-credential-file=C:/Users/yys/coding-plan/auth.json",
        ])
        .expect("parse config import-local command");

        match command {
            CliCommand::ConfigImportLocal {
                workspace,
                home,
                options,
            } => {
                assert_eq!(workspace, PathBuf::from("E:/demo"));
                assert_eq!(home, PathBuf::from("C:/Users/yys"));
                assert!(options.codex);
                assert!(options.claude_code);
                assert!(!options.copy_credentials);
                assert_eq!(
                    options.advisor_credential_source,
                    Some(AdvisorCredentialSource::CodingPlan)
                );
                assert_eq!(
                    options.advisor_credential_file.as_deref(),
                    Some("C:/Users/yys/coding-plan/auth.json")
                );
            }
            _ => panic!("expected config import-local command"),
        }
    }

    #[test]
    fn parses_config_import_coding_plan_credentials_command() {
        let command = parse_cli([
            "config",
            "import-coding-plan-credentials",
            "--workspace=E:/demo",
            "--source-file=C:/Users/yys/coding-plan/auth.json",
            "--endpoint=https://api.openai.com/v1/chat/completions",
            "--model=gpt-5.5",
        ])
        .expect("parse coding-plan credential import command");

        match command {
            CliCommand::ConfigImportCodingPlanCredentials {
                workspace,
                source_file,
                endpoint,
                model,
            } => {
                assert_eq!(workspace, PathBuf::from("E:/demo"));
                assert_eq!(
                    source_file,
                    PathBuf::from("C:/Users/yys/coding-plan/auth.json")
                );
                assert_eq!(
                    endpoint.as_deref(),
                    Some("https://api.openai.com/v1/chat/completions")
                );
                assert_eq!(model.as_deref(), Some("gpt-5.5"));
            }
            _ => panic!("expected config import-coding-plan-credentials command"),
        }
    }

    #[test]
    fn config_import_coding_plan_credentials_defaults_to_dedicated_profile() {
        let command = parse_cli(["config", "import-coding-plan-credentials"])
            .expect("parse coding-plan credential import command");

        match command {
            CliCommand::ConfigImportCodingPlanCredentials { source_file, .. } => {
                assert!(source_file.ends_with(PathBuf::from(".coding-plan").join("auth.json")));
                assert!(!source_file.to_string_lossy().contains(".codex"));
                assert!(!source_file.to_string_lossy().contains(".claude"));
            }
            _ => panic!("expected config import-coding-plan-credentials command"),
        }
    }

    #[test]
    fn config_import_coding_plan_credentials_rejects_cli_auth_source_path() {
        let error = parse_cli([
            "config",
            "import-coding-plan-credentials",
            "--source-file=C:/Users/yys/.codex/auth.json",
        ])
        .expect_err("local CLI auth must not parse as coding-plan source");

        assert!(error.contains("local CLI auth"));
        assert!(error.contains("dedicated coding-plan"));
    }

    #[test]
    fn parses_trail_command() {
        let command = parse_cli(["trail", "--workspace=E:/demo"]).expect("parse trail command");

        match command {
            CliCommand::Trail { workspace } => {
                assert_eq!(workspace, PathBuf::from("E:/demo"));
            }
            _ => panic!("expected trail command"),
        }
    }

    #[test]
    fn parses_dev_history_command() {
        let command = parse_cli([
            "dev-history",
            "--workspace=F:/rag_sys",
            "--home=C:/Users/yys",
            "--codex-sessions=C:/tmp/codex",
            "--claude-projects=C:/tmp/claude",
            "--top=7",
            "--write",
        ])
        .expect("parse dev-history command");

        match command {
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
                assert_eq!(workspace, PathBuf::from("F:/rag_sys"));
                assert_eq!(codex_sessions_root, PathBuf::from("C:/tmp/codex"));
                assert_eq!(claude_projects_root, PathBuf::from("C:/tmp/claude"));
                assert_eq!(top_limit, 7);
                assert!(write);
                assert!(!export_raw);
                assert_eq!(raw_output_root, None);
                assert_eq!(raw_package_name, None);
            }
            _ => panic!("expected dev-history command"),
        }
    }

    #[test]
    fn parses_dev_history_raw_export_command() {
        let command = parse_cli([
            "dev-history",
            "--workspace=F:/rag_sys",
            "--home=C:/Users/yys",
            "--export-raw",
            "--output=F:/rag_sys/.agent-monitor/exports",
            "--package-name=rag-sys-raw",
        ])
        .expect("parse raw dev-history command");

        match command {
            CliCommand::DevHistory {
                workspace,
                export_raw,
                raw_output_root,
                raw_package_name,
                ..
            } => {
                assert_eq!(workspace, PathBuf::from("F:/rag_sys"));
                assert!(export_raw);
                assert_eq!(
                    raw_output_root,
                    Some(PathBuf::from("F:/rag_sys/.agent-monitor/exports"))
                );
                assert_eq!(raw_package_name.as_deref(), Some("rag-sys-raw"));
            }
            _ => panic!("expected dev-history command"),
        }
    }

    #[test]
    fn dev_history_command_defaults_to_local_cli_history_roots() {
        let command =
            parse_cli(["dev-history", "--home=C:/Users/yys"]).expect("parse dev-history command");

        match command {
            CliCommand::DevHistory {
                codex_sessions_root,
                claude_projects_root,
                ..
            } => {
                assert_eq!(
                    codex_sessions_root,
                    PathBuf::from("C:/Users/yys")
                        .join(".codex")
                        .join("sessions")
                );
                assert_eq!(
                    claude_projects_root,
                    PathBuf::from("C:/Users/yys")
                        .join(".claude")
                        .join("projects")
                );
            }
            _ => panic!("expected dev-history command"),
        }
    }

    #[test]
    fn parses_memory_promote_command() {
        let command = parse_cli([
            "memory",
            "promote",
            "--workspace=E:/demo",
            "--memory-id=mem-evt-design",
            "--source=manual_review",
        ])
        .expect("parse memory promote command");

        match command {
            CliCommand::MemoryPromote {
                workspace,
                memory_id,
                source,
            } => {
                assert_eq!(workspace, PathBuf::from("E:/demo"));
                assert_eq!(memory_id, "mem-evt-design");
                assert_eq!(source, MemorySource::ManualReview);
            }
            _ => panic!("expected memory promote command"),
        }
    }

    #[test]
    fn memory_promote_command_rejects_agent_claim_source() {
        let error = parse_cli([
            "memory",
            "promote",
            "--memory-id=mem-evt-design",
            "--source=agent_claim",
        ])
        .expect_err("agent_claim source should not parse for promotion");

        assert!(error.contains("trusted source"));
    }

    #[test]
    fn memory_promote_command_rejects_unknown_source() {
        let error = parse_cli([
            "memory",
            "promote",
            "--memory-id=mem-evt-design",
            "--source=unknown",
        ])
        .expect_err("unknown source should not parse for promotion");

        assert!(error.contains("--source must be one of"));
    }

    #[test]
    fn parses_blame_command() {
        let command = parse_cli([
            "blame",
            "--workspace=E:/demo",
            "--file=src/lib.rs",
            "--line=42",
            "--limit=3",
        ])
        .expect("parse blame command");

        match command {
            CliCommand::Blame {
                workspace,
                file,
                line,
                limit,
            } => {
                assert_eq!(workspace, PathBuf::from("E:/demo"));
                assert_eq!(file, "src/lib.rs");
                assert_eq!(line, Some(42));
                assert_eq!(limit, 3);
            }
            _ => panic!("expected blame command"),
        }
    }

    #[test]
    fn blame_command_rejects_zero_line() {
        let error = parse_cli(["blame", "--file=src/lib.rs", "--line=0"])
            .expect_err("zero line should be rejected");

        assert!(error.contains("--line must be a positive integer"));
    }

    #[test]
    fn parses_handoff_command() {
        let command = parse_cli(["handoff", "--workspace=E:/demo", "--agent=claude-code"])
            .expect("parse handoff command");

        match command {
            CliCommand::Handoff { workspace, agent } => {
                assert_eq!(workspace, PathBuf::from("E:/demo"));
                assert_eq!(agent, AgentKind::ClaudeCode);
            }
            _ => panic!("expected handoff command"),
        }
    }

    #[test]
    fn parses_repo_audit_command() {
        let command =
            parse_cli(["repo-audit", "--workspace=E:/demo"]).expect("parse repo-audit command");

        match command {
            CliCommand::RepoAudit { workspace } => {
                assert_eq!(workspace, PathBuf::from("E:/demo"));
            }
            _ => panic!("expected repo-audit command"),
        }
    }

    #[test]
    fn parses_repo_hunks_command() {
        let command = parse_cli([
            "repo-hunks",
            "--workspace=E:/demo",
            "--file=src/lib.rs",
            "--line=42",
            "--limit=5",
        ])
        .expect("parse repo-hunks command");

        match command {
            CliCommand::RepoHunks {
                workspace,
                file,
                line,
                limit,
            } => {
                assert_eq!(workspace, PathBuf::from("E:/demo"));
                assert_eq!(file.as_deref(), Some("src/lib.rs"));
                assert_eq!(line, Some(42));
                assert_eq!(limit, 5);
            }
            _ => panic!("expected repo-hunks command"),
        }
    }

    #[test]
    fn parses_requirements_command() {
        let command = parse_cli([
            "requirements",
            "--workspace=E:/demo",
            "--status=unmapped",
            "--requirement=req-docs",
            "--text=coding-plan",
            "--max-proof-score=49",
            "--limit=5",
        ])
        .expect("parse requirements command");

        match command {
            CliCommand::Requirements { workspace, query } => {
                assert_eq!(workspace, PathBuf::from("E:/demo"));
                assert_eq!(
                    query.status,
                    Some(coding_agent_monitor::AcceptanceCoverageStatus::Unmapped)
                );
                assert_eq!(query.requirement_id.as_deref(), Some("req-docs"));
                assert_eq!(query.text.as_deref(), Some("coding-plan"));
                assert_eq!(query.max_proof_score, Some(49));
                assert_eq!(query.limit, 5);
            }
            _ => panic!("expected requirements command"),
        }
    }

    #[test]
    fn parses_completion_certificate_command() {
        let command = parse_cli([
            "completion-certificate",
            "--workspace=E:/demo",
            "--status=covered",
            "--requirement=req-api",
            "--text=advisor",
            "--max-proof-score=60",
            "--limit=3",
        ])
        .expect("parse completion-certificate command");

        match command {
            CliCommand::CompletionCertificate { workspace, query } => {
                assert_eq!(workspace, PathBuf::from("E:/demo"));
                assert_eq!(
                    query.status,
                    Some(coding_agent_monitor::AcceptanceCoverageStatus::Covered)
                );
                assert_eq!(query.requirement_id.as_deref(), Some("req-api"));
                assert_eq!(query.text.as_deref(), Some("advisor"));
                assert_eq!(query.max_proof_score, Some(60));
                assert_eq!(query.limit, 3);
            }
            _ => panic!("expected completion-certificate command"),
        }
    }

    #[test]
    fn requirements_command_rejects_invalid_max_proof_score() {
        let error = parse_cli(["requirements", "--max-proof-score=101"])
            .expect_err("proof score must be bounded");

        assert!(error.contains("--max-proof-score"));
    }

    #[test]
    fn parses_calibration_command() {
        let command = parse_cli([
            "calibration",
            "--workspace=E:/demo",
            "--action=force_verification",
            "--limit=7",
        ])
        .expect("parse calibration command");

        match command {
            CliCommand::Calibration { workspace, query } => {
                assert_eq!(workspace, PathBuf::from("E:/demo"));
                assert_eq!(
                    query.action,
                    Some(coding_agent_monitor::ControlActionKind::ForceVerification)
                );
                assert_eq!(query.limit, 7);
            }
            _ => panic!("expected calibration command"),
        }
    }

    #[test]
    fn parses_run_probe_action_kind() {
        let command = parse_cli([
            "calibration",
            "--workspace=E:/demo",
            "--action=run_probe",
            "--limit=7",
        ])
        .expect("parse calibration command");

        match command {
            CliCommand::Calibration { query, .. } => {
                assert_eq!(
                    query.action,
                    Some(coding_agent_monitor::ControlActionKind::RunProbe)
                );
            }
            _ => panic!("expected calibration command"),
        }
    }

    #[test]
    fn repo_hunks_command_rejects_zero_line() {
        let error =
            parse_cli(["repo-hunks", "--line=0"]).expect_err("zero line should be rejected");

        assert!(error.contains("--line must be a positive integer"));
    }

    #[test]
    fn parses_verify_command() {
        let command = parse_cli(["verify", "--workspace=E:/demo", "--verifier=smoke"])
            .expect("parse verify command");

        match command {
            CliCommand::Verify {
                workspace,
                verifier_id,
            } => {
                assert_eq!(workspace, PathBuf::from("E:/demo"));
                assert_eq!(verifier_id, "smoke");
            }
            _ => panic!("expected verify command"),
        }
    }

    #[test]
    fn parses_probe_command() {
        let command = parse_cli(["probe", "--workspace=E:/demo"]).expect("parse probe command");

        match command {
            CliCommand::Probe { workspace } => {
                assert_eq!(workspace, PathBuf::from("E:/demo"));
            }
            _ => panic!("expected probe command"),
        }
    }

    #[test]
    fn parses_demo_command() {
        let command = parse_cli(["demo", "--workspace=E:/coding-agent-monitor-demo"])
            .expect("parse demo command");

        match command {
            CliCommand::Demo { workspace } => {
                assert_eq!(workspace, PathBuf::from("E:/coding-agent-monitor-demo"));
            }
            _ => panic!("expected demo command"),
        }
    }

    fn git<const N: usize>(workspace: &std::path::Path, args: [&str; N]) {
        let output = std::process::Command::new("git")
            .current_dir(workspace)
            .args(args)
            .output()
            .expect("run git");
        assert!(
            output.status.success(),
            "git command failed: {}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn git_stdout<const N: usize>(workspace: &std::path::Path, args: [&str; N]) -> String {
        let output = std::process::Command::new("git")
            .current_dir(workspace)
            .args(args)
            .output()
            .expect("run git");
        assert!(
            output.status.success(),
            "git command failed: {}\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout)
            .expect("git stdout utf8")
            .trim()
            .to_string()
    }
}
