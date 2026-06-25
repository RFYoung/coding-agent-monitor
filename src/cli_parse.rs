//! Command-line argument parsing for the `agent-monitor` binary: turns raw
//! `argv` into a validated `CliCommand`. Pure parsing and validation, no I/O.

use crate::cli_support::{
    default_coding_plan_credential_source, default_home_dir, reject_local_cli_auth_source,
};
use crate::hook_response::HookResponseFormat;
use crate::*;

pub(crate) fn parse_cli(
    args: impl IntoIterator<Item = impl Into<String>>,
) -> Result<CliCommand, String> {
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

pub(crate) fn parse_monitor_args(
    args: impl IntoIterator<Item = String>,
) -> Result<CliCommand, String> {
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

pub(crate) fn parse_inject_args(
    args: impl IntoIterator<Item = String>,
) -> Result<CliCommand, String> {
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

pub(crate) fn parse_inject_running_args(
    args: impl IntoIterator<Item = String>,
) -> Result<CliCommand, String> {
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

pub(crate) fn parse_wrap_args(
    args: impl IntoIterator<Item = String>,
) -> Result<CliCommand, String> {
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

pub(crate) fn parse_judge_args(
    args: impl IntoIterator<Item = String>,
) -> Result<CliCommand, String> {
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

pub(crate) fn parse_ingest_args(
    args: impl IntoIterator<Item = String>,
) -> Result<CliCommand, String> {
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

pub(crate) fn parse_hook_response_args(
    args: impl IntoIterator<Item = String>,
) -> Result<CliCommand, String> {
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

pub(crate) fn parse_hook_response_format(value: &str) -> Result<HookResponseFormat, String> {
    match value.trim() {
        "generic" => Ok(HookResponseFormat::Generic),
        "codex" => Ok(HookResponseFormat::Codex),
        "claude-code" | "claude" => Ok(HookResponseFormat::ClaudeCode),
        "opencode" | "open-code" => Ok(HookResponseFormat::OpenCode),
        "" => Err("--format must not be empty".into()),
        _ => Err("--format must be generic, codex, claude-code, or opencode".into()),
    }
}

pub(crate) fn parse_config_args(
    args: impl IntoIterator<Item = String>,
) -> Result<CliCommand, String> {
    let args = args.into_iter().collect::<Vec<_>>();
    match args.first().map(String::as_str) {
        Some("advisor") => parse_config_advisor_args(args.into_iter().skip(1)),
        Some("verifier") => parse_config_verifier_args(args.into_iter().skip(1)),
        Some("runtime-auth") => parse_config_runtime_auth_args(args.into_iter().skip(1)),
        Some("import-local") => parse_config_import_local_args(args.into_iter().skip(1)),
        Some("import-coding-plan-credentials") => {
            parse_config_import_coding_plan_credentials_args(args.into_iter().skip(1))
        }
        _ => Err(
            "config requires subcommand: advisor|verifier|runtime-auth|import-local|import-coding-plan-credentials"
                .into(),
        ),
    }
}

pub(crate) fn parse_config_advisor_args(
    args: impl IntoIterator<Item = String>,
) -> Result<CliCommand, String> {
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

pub(crate) fn parse_advisor_credential_source(
    value: &str,
) -> Result<AdvisorCredentialSource, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "env" => Ok(AdvisorCredentialSource::Env),
        "coding_plan" | "coding-plan" => Ok(AdvisorCredentialSource::CodingPlan),
        _ => Err("credential source must be env or coding-plan".into()),
    }
}

pub(crate) fn parse_config_verifier_args(
    args: impl IntoIterator<Item = String>,
) -> Result<CliCommand, String> {
    let mut workspace = PathBuf::from(".");
    let mut id = None;
    let mut command = None;
    let mut scope = VerificationScope::Full;
    let mut timeout_secs = 300;
    let mut paths = Vec::new();
    let mut acceptance_patterns = Vec::new();

    for arg in args {
        if let Some(value) = arg.strip_prefix("--workspace=") {
            workspace = PathBuf::from(value);
        } else if let Some(value) = arg.strip_prefix("--id=") {
            let value = value.trim();
            if value.is_empty() {
                return Err("--id must not be empty".into());
            }
            id = Some(value.to_string());
        } else if let Some(value) = arg.strip_prefix("--command=") {
            let value = value.trim();
            if value.is_empty() {
                return Err("--command must not be empty".into());
            }
            command = Some(value.to_string());
        } else if let Some(value) = arg.strip_prefix("--scope=") {
            scope = parse_verification_scope(value)?;
        } else if let Some(value) = arg.strip_prefix("--timeout-secs=") {
            timeout_secs = parse_positive_u64("--timeout-secs", value)?;
        } else if let Some(value) = arg.strip_prefix("--path=") {
            let value = value.trim();
            if value.is_empty() {
                return Err("--path must not be empty".into());
            }
            paths.push(value.to_string());
        } else if let Some(value) = arg.strip_prefix("--acceptance-pattern=") {
            let value = value.trim();
            if value.is_empty() {
                return Err("--acceptance-pattern must not be empty".into());
            }
            acceptance_patterns.push(value.to_string());
        } else {
            return Err(format!("unknown config verifier argument: {arg}"));
        }
    }

    let id = id.ok_or_else(|| "config verifier requires --id=<id>".to_string())?;
    let command =
        command.ok_or_else(|| "config verifier requires --command=<command>".to_string())?;
    Ok(CliCommand::ConfigVerifier {
        workspace,
        verifier: VerifierConfig {
            id,
            command,
            scope,
            timeout_secs,
            paths,
            acceptance_patterns,
        },
    })
}

pub(crate) fn parse_config_runtime_auth_args(
    args: impl IntoIterator<Item = String>,
) -> Result<CliCommand, String> {
    let mut workspace = PathBuf::from(".");
    let mut agent = None;
    let mut style = None;
    let mut endpoint = None;
    let mut profile_id = None;
    let mut account_id = None;
    let mut model = None;
    let mut api_format = None;
    let mut health_status = None;

    for arg in args {
        if let Some(value) = arg.strip_prefix("--workspace=") {
            workspace = PathBuf::from(value);
        } else if let Some(value) = arg.strip_prefix("--agent=") {
            agent = Some(value.parse().map_err(|error| format!("{error}"))?);
        } else if let Some(value) = arg.strip_prefix("--style=") {
            style = Some(parse_runtime_auth_style(value)?);
        } else if let Some(value) = arg.strip_prefix("--endpoint=") {
            endpoint = Some(non_empty_arg("--endpoint", value)?.to_string());
        } else if let Some(value) = arg.strip_prefix("--profile-id=") {
            profile_id = Some(non_empty_arg("--profile-id", value)?.to_string());
        } else if let Some(value) = arg.strip_prefix("--account-id=") {
            account_id = Some(non_empty_arg("--account-id", value)?.to_string());
        } else if let Some(value) = arg.strip_prefix("--model=") {
            model = Some(non_empty_arg("--model", value)?.to_string());
        } else if let Some(value) = arg.strip_prefix("--api-format=") {
            api_format = Some(non_empty_arg("--api-format", value)?.to_string());
        } else if let Some(value) = arg.strip_prefix("--health-status=") {
            health_status = Some(non_empty_arg("--health-status", value)?.to_string());
        } else {
            return Err(format!("unknown config runtime-auth argument: {arg}"));
        }
    }

    let agent = agent.ok_or_else(|| {
        "config runtime-auth requires --agent=<codex|claude-code|pi|opencode>".to_string()
    })?;
    let style = style.ok_or_else(|| "config runtime-auth requires --style=<style>".to_string())?;
    Ok(CliCommand::ConfigRuntimeAuth {
        workspace,
        agent,
        runtime_auth: RuntimeAuthConfig {
            style,
            endpoint,
            profile_id,
            account_id,
            model,
            api_format,
            health_status,
        },
    })
}

pub(crate) fn parse_runtime_auth_style(value: &str) -> Result<RuntimeAuthStyle, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "native_cli_auth" | "native-cli-auth" => Ok(RuntimeAuthStyle::NativeCliAuth),
        "local_auth_broker" | "local-auth-broker" => Ok(RuntimeAuthStyle::LocalAuthBroker),
        "" => Err("--style must not be empty".into()),
        _ => Err("--style must be native-cli-auth or local-auth-broker".into()),
    }
}

pub(crate) fn parse_verification_scope(value: &str) -> Result<VerificationScope, String> {
    match value.trim().to_ascii_lowercase().as_str() {
        "full" => Ok(VerificationScope::Full),
        "targeted" => Ok(VerificationScope::Targeted),
        "style" => Ok(VerificationScope::Style),
        "" => Err("--scope must not be empty".into()),
        _ => Err("--scope must be full, targeted, or style".into()),
    }
}

pub(crate) fn parse_config_import_local_args(
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

pub(crate) fn parse_config_import_coding_plan_credentials_args(
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

pub(crate) fn parse_advise_args(
    args: impl IntoIterator<Item = String>,
) -> Result<CliCommand, String> {
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

pub(crate) fn parse_trail_args(
    args: impl IntoIterator<Item = String>,
) -> Result<CliCommand, String> {
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

pub(crate) fn parse_dev_history_args(
    args: impl IntoIterator<Item = String>,
) -> Result<CliCommand, String> {
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

pub(crate) fn parse_calibration_args(
    args: impl IntoIterator<Item = String>,
) -> Result<CliCommand, String> {
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

pub(crate) fn parse_control_action_kind(value: &str) -> Result<ControlActionKind, String> {
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

pub(crate) fn parse_blame_args(
    args: impl IntoIterator<Item = String>,
) -> Result<CliCommand, String> {
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

pub(crate) fn parse_repo_hunks_args(
    args: impl IntoIterator<Item = String>,
) -> Result<CliCommand, String> {
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

pub(crate) fn parse_requirements_args(
    args: impl IntoIterator<Item = String>,
) -> Result<CliCommand, String> {
    let (workspace, query) = parse_requirement_query_args("requirements", args)?;

    Ok(CliCommand::Requirements { workspace, query })
}

pub(crate) fn parse_completion_certificate_args(
    args: impl IntoIterator<Item = String>,
) -> Result<CliCommand, String> {
    let (workspace, query) = parse_requirement_query_args("completion-certificate", args)?;

    Ok(CliCommand::CompletionCertificate { workspace, query })
}

pub(crate) fn parse_trace_args(
    args: impl IntoIterator<Item = String>,
) -> Result<CliCommand, String> {
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

pub(crate) fn non_empty_arg<'a>(name: &str, value: &'a str) -> Result<&'a str, String> {
    let value = value.trim();
    if value.is_empty() {
        Err(format!("{name} must not be empty"))
    } else {
        Ok(value)
    }
}

pub(crate) fn parse_positive_u32(name: &str, value: &str) -> Result<u32, String> {
    let parsed = value
        .parse()
        .map_err(|_| format!("{name} must be a positive integer"))?;
    if parsed == 0 {
        Err(format!("{name} must be a positive integer"))
    } else {
        Ok(parsed)
    }
}

pub(crate) fn parse_positive_u64(name: &str, value: &str) -> Result<u64, String> {
    let parsed = value
        .parse()
        .map_err(|_| format!("{name} must be a positive integer"))?;
    if parsed == 0 {
        Err(format!("{name} must be a positive integer"))
    } else {
        Ok(parsed)
    }
}

pub(crate) fn parse_requirement_query_args(
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

pub(crate) fn parse_requirement_proof_score(value: &str) -> Result<u8, String> {
    let value = value
        .trim()
        .parse::<u8>()
        .map_err(|_| "--max-proof-score must be an integer from 0 to 100".to_string())?;
    if value > 100 {
        return Err("--max-proof-score must be an integer from 0 to 100".into());
    }
    Ok(value)
}

pub(crate) fn parse_acceptance_coverage_status(
    value: &str,
) -> Result<AcceptanceCoverageStatus, String> {
    match value.trim().to_ascii_lowercase().replace('_', "-").as_str() {
        "covered" => Ok(AcceptanceCoverageStatus::Covered),
        "stale" => Ok(AcceptanceCoverageStatus::Stale),
        "failed" => Ok(AcceptanceCoverageStatus::Failed),
        "unverified" => Ok(AcceptanceCoverageStatus::Unverified),
        "unmapped" => Ok(AcceptanceCoverageStatus::Unmapped),
        _ => Err("--status must be covered, stale, failed, unverified, or unmapped".into()),
    }
}

pub(crate) fn parse_handoff_args(
    args: impl IntoIterator<Item = String>,
) -> Result<CliCommand, String> {
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

pub(crate) fn parse_memory_args(
    args: impl IntoIterator<Item = String>,
) -> Result<CliCommand, String> {
    let args = args.into_iter().collect::<Vec<_>>();
    if args.first().is_none_or(|arg| arg != "promote") {
        return Err("memory requires subcommand: promote".into());
    }
    parse_memory_promote_args(args.into_iter().skip(1))
}

pub(crate) fn parse_memory_promote_args(
    args: impl IntoIterator<Item = String>,
) -> Result<CliCommand, String> {
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

pub(crate) fn parse_trusted_memory_source(value: &str) -> Result<MemorySource, String> {
    match value {
        "manual_review" => Ok(MemorySource::ManualReview),
        "user" => Ok(MemorySource::User),
        "verified_result" => Ok(MemorySource::VerifiedResult),
        "agent_claim" => Err("memory promote requires a trusted source".into()),
        _ => Err("--source must be one of manual_review, user, verified_result".into()),
    }
}

pub(crate) fn parse_repo_audit_args(
    args: impl IntoIterator<Item = String>,
) -> Result<CliCommand, String> {
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

pub(crate) fn parse_verify_args(
    args: impl IntoIterator<Item = String>,
) -> Result<CliCommand, String> {
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

pub(crate) fn parse_probe_args(
    args: impl IntoIterator<Item = String>,
) -> Result<CliCommand, String> {
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

pub(crate) fn parse_demo_args(
    args: impl IntoIterator<Item = String>,
) -> Result<CliCommand, String> {
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

pub(crate) fn parse_bool(name: &str, value: &str) -> Result<bool, String> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(format!("{name} must be true or false")),
    }
}

pub(crate) fn parse_api_key_env_name(value: &str) -> Result<String, String> {
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

#[cfg(test)]
#[path = "cli_parse_tests.rs"]
mod tests;
