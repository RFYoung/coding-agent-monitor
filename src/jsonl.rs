//! Streaming JSONL control entrypoint (`run_jsonl`) and its per-line summary types.

use crate::*;

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

pub(crate) fn ensure_adapter_ingest_enabled(
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

pub(crate) fn filter_disabled_fallback_agents(
    mut config: Config,
    project_config: &ProjectConfig,
) -> Config {
    config.fallback_agents.retain(|agent| {
        AgentKind::from_str(agent).is_ok_and(|kind| {
            let capabilities = adapter_capabilities_for_config(kind, &project_config.adapters);
            adapter_capability_allows_writable_handoff(&capabilities)
        })
    });
    config
}

pub(crate) fn run_adapter_jsonl_inner(
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

pub(crate) fn stamp_adapter_line_provenance(
    events: &mut [Event],
    line_number: usize,
    raw_line: &str,
) {
    let source_hash = fnv1a64_digest(raw_line.as_bytes());
    for event in events {
        fill_empty_string(&mut event.source_type, "adapter_jsonl".into());
        if event.source_offset.is_none() {
            event.source_offset = Some(line_number as u64);
        }
        fill_empty_string(&mut event.source_hash, source_hash.clone());
    }
}

pub(crate) fn run_jsonl_inner(
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

pub(crate) fn process_monitor_event(
    event: Event,
    line_number: usize,
    monitor: &mut Monitor,
    store: Option<&mut ProjectStore>,
    output: &mut impl Write,
) -> Result<(), JsonlError> {
    process_monitor_events(vec![event], line_number, monitor, store, output)
}

pub(crate) fn process_monitor_events(
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

pub(crate) fn event_is_low_signal_message_delta(event: &Event) -> bool {
    event.kind == EventKind::CommandOutput
        && event
            .content
            .as_deref()
            .is_some_and(|content| content.trim_start().starts_with("message delta:"))
}

pub(crate) fn event_triggers_streaming_control_evaluation(
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

pub(crate) fn event_is_content_control_trigger(event: &Event) -> bool {
    let Some(content) = event.content.as_deref() else {
        return false;
    };
    looks_like_premature_stop(content)
        || looks_like_completion_claim(content)
        || looks_like_unverified_completion(content)
}

pub(crate) fn repeated_command_failure_crossed_threshold(
    events: &[Event],
    current: &Event,
) -> bool {
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

pub(crate) fn repeated_command_failure_signature(event: &Event) -> Option<String> {
    if event.kind != EventKind::CommandResult || event.exit_code.is_none_or(|code| code == 0) {
        return None;
    }
    let command = event.command.as_deref().map(normalize_command_signature)?;
    if is_verification_command(&command) {
        return None;
    }
    Some(command)
}

pub(crate) fn repeated_service_failure_crossed_threshold(
    events: &[Event],
    current: &Event,
) -> bool {
    repeated_content_failure_count(
        events,
        current,
        looks_like_service_failure,
        event_can_clear_service_failure,
    )
    .is_some_and(|count| count == 3)
}

pub(crate) fn repeated_permission_lifecycle_crossed_threshold(
    events: &[Event],
    current: &Event,
) -> bool {
    repeated_content_failure_count(
        events,
        current,
        permission_lifecycle_is_blocked,
        event_can_clear_service_failure,
    )
    .is_some_and(|count| count == 2)
}

pub(crate) fn repeated_content_failure_count(
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

pub(crate) fn event_is_lifecycle_control_trigger(event: &Event) -> bool {
    let content = event.content.as_deref().unwrap_or_default();
    matches!(event.kind, EventKind::AgentHealth)
        && (looks_like_session_idle_or_stop(content)
            || looks_like_session_error(content)
            || looks_like_context_compaction(content)
            || looks_like_forgetting_design_memory(content))
        || matches!(event.kind, EventKind::InterventionResult)
            && permission_lifecycle_is_blocked(content)
}

pub(crate) fn design_entry_from_event(event: &Event) -> Option<DesignEntry> {
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

pub(crate) fn trace_entry_from_event(event: &Event) -> Option<TraceEntry> {
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

pub(crate) struct JsonlSummary<T> {
    pub(crate) count: usize,
    pub(crate) recent: Vec<T>,
}

pub(crate) struct JsonlLine {
    line_number: usize,
    line: String,
    terminated: bool,
}

pub(crate) fn read_jsonl_summary<T, F>(
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

pub(crate) fn read_non_empty_jsonl_lines(path: &Path) -> Result<Vec<JsonlLine>, StoreError> {
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

pub(crate) fn count_jsonl_lines(path: &Path) -> Result<usize, StoreError> {
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

pub(crate) fn dashboard_advisor_status(store_root: &Path) -> DashboardAdvisorStatus {
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

pub(crate) fn dashboard_advisor_status_from_config(
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

pub(crate) fn dashboard_coding_plan_credential_status(
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

pub(crate) fn advisor_dashboard_credential_path(
    credential_file: &str,
    store_root: &Path,
) -> PathBuf {
    let path = Path::new(credential_file);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        store_root.join(path)
    }
}

pub(crate) fn coding_plan_dashboard_token(value: &serde_json::Value) -> Option<String> {
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

pub(crate) fn credential_string_at_any_json_pointer(
    value: &serde_json::Value,
    pointers: &[&str],
) -> Option<String> {
    pointers
        .iter()
        .find_map(|pointer| value.pointer(pointer).and_then(serde_json::Value::as_str))
        .map(|token| token.trim().to_string())
        .filter(|token| !token.is_empty())
}

pub(crate) fn looks_like_jwt_bearer_token(token: &str) -> bool {
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

pub(crate) fn is_public_openai_endpoint(endpoint: &str) -> bool {
    advisor_endpoint_host(endpoint)
        .as_deref()
        .is_some_and(|host| host.eq_ignore_ascii_case("api.openai.com"))
}

pub(crate) fn advisor_endpoint_host(endpoint: &str) -> Option<String> {
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

pub(crate) fn local_cli_auth_profile_dir_for_dashboard(path: &Path) -> Option<&'static str> {
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

pub(crate) fn dashboard_severity(
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

pub(crate) fn max_dashboard_severity(
    left: DashboardSeverity,
    right: DashboardSeverity,
) -> DashboardSeverity {
    if dashboard_severity_rank(left) >= dashboard_severity_rank(right) {
        left
    } else {
        right
    }
}

pub(crate) fn dashboard_severity_rank(severity: DashboardSeverity) -> u8 {
    match severity {
        DashboardSeverity::Healthy => 0,
        DashboardSeverity::Warning => 1,
        DashboardSeverity::Critical => 2,
    }
}

pub(crate) fn agent_sessions(
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

pub(crate) fn status_rank(status: AgentActivityStatus) -> u8 {
    match status {
        AgentActivityStatus::Degraded => 0,
        AgentActivityStatus::Stale => 1,
        AgentActivityStatus::Active => 2,
    }
}

pub(crate) fn parse_utc_seconds(value: &str) -> Option<i64> {
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

pub(crate) fn current_utc_timestamp() -> Option<String> {
    current_utc_seconds().map(format_utc_seconds)
}

pub(crate) fn current_utc_seconds() -> Option<i64> {
    Some(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()?
            .as_secs() as i64,
    )
}

pub(crate) fn format_utc_seconds(seconds: i64) -> String {
    let days = seconds.div_euclid(86_400);
    let seconds_of_day = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    let hour = seconds_of_day / 3_600;
    let minute = seconds_of_day % 3_600 / 60;
    let second = seconds_of_day % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

pub(crate) fn civil_from_days(days: i64) -> (i32, u32, u32) {
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

pub(crate) fn days_from_civil(year: i32, month: u32, day: u32) -> i64 {
    let year = year - if month <= 2 { 1 } else { 0 };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let month = month as i32;
    let day = day as i32;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    i64::from(era * 146_097 + doe - 719_468)
}
