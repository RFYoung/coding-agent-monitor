//! CLI entrypoint for `agent-monitor`.
//!
//! This file owns command dispatch. Argument parsing, hook-response policy,
//! external judge invocation, and small path helpers live in sibling modules so
//! the command arms stay close to their user-visible behavior.

use coding_agent_monitor::{
    AcceptanceCoverageStatus, AdapterIngestOptions, AdvisorCredentialSource,
    AdvisorEndpointConfigUpdate, AgentKind, BlameQuery, CalibrationQuery, Config,
    ControlActionKind, DashboardSnapshot, DevHistoryAnalysisOptions, DevHistoryRawExportOptions,
    InstallMode, LocalAgentConfigImportOptions, MemorySource, ProjectStore, RepoHunkHistoryQuery,
    RequirementGraphQuery, RuntimeAuthConfig, RuntimeAuthStyle, TraceEntry, VerificationScope,
    VerifierConfig, WrappedCommand, advise_workspace, analyze_local_dev_history,
    create_demo_workspace, detect_running_agents_from_system, export_raw_dev_history,
    handoff_workspace, import_coding_plan_advisor_credentials, import_local_agent_configs,
    injection_plan_for_workspace, install_agent_injection, judge_snapshot, load_blame_report,
    load_calibration_report, load_completion_certificate_report, load_decision_trails,
    load_repo_hunk_history, load_requirement_graph, promote_memory_candidate,
    record_repo_audit_history, record_trace_entry, run_adapter_jsonl_with_store, run_jsonl,
    run_jsonl_with_store, run_probe, run_verifier, run_wrapped_command,
    write_adapter_runtime_auth_config, write_advisor_endpoint_config, write_verifier_config,
};
use std::env;
use std::path::PathBuf;
use std::process::ExitCode;

// Keep parsing pure and testable; command arms below should only perform I/O
// and call library entrypoints. Command-specific machinery lives in focused
// sibling modules: argument parsing, hook responses, external judge, and the
// small cross-command helpers shared by parsing and dispatch.
mod cli_judge;
mod cli_parse;
mod cli_support;
mod hook_response;

use cli_judge::{interventions_from_review, run_external_judge};
use cli_parse::*;
use cli_support::injection_workspace_for;
use hook_response::{HookResponseFormat, run_hook_response};

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
