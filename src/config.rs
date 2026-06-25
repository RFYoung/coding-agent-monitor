//! Project configuration persistence and local agent config import.
//!
//! Keep two credential boundaries separate here:
//! - advisor credentials belong to the monitor's optional diagnostic LLM;
//! - adapter runtime auth describes how the monitor launches or talks to Codex,
//!   Claude Code, OpenCode, or Pi without copying their CLI auth material.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

use crate::AgentKind;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectConfig {
    #[serde(default)]
    pub advisor: AdvisorConfig,
    #[serde(default)]
    pub policy: PolicyConfig,
    #[serde(default)]
    pub security: SecurityConfig,
    #[serde(default)]
    pub verifiers: Vec<VerifierConfig>,
    #[serde(default)]
    pub adapters: AdapterConfig,
    #[serde(default)]
    pub local_agents: LocalAgentConfig,
}

impl ProjectConfig {
    pub fn load(store_root: impl AsRef<Path>) -> Result<Self, ProjectConfigError> {
        let path = store_root.as_ref().join("config.json");
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = fs::read_to_string(&path).map_err(|source| ProjectConfigError::Read {
            path: path.clone(),
            source,
        })?;
        serde_json::from_str(&content).map_err(|source| ProjectConfigError::Decode { path, source })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdvisorEndpointConfigUpdate {
    pub endpoint: String,
    pub model: String,
    pub api_key_env: String,
    pub credential_source: Option<AdvisorCredentialSource>,
    pub credential_file: Option<String>,
    pub enabled: bool,
}

/// Non-secret runtime-auth metadata for an adapter.
///
/// This records the control surface the monitor may use at runtime. It must not
/// contain bearer tokens, refresh tokens, copied CLI auth files, or env values.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeAuthConfig {
    pub style: RuntimeAuthStyle,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_format: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health_status: Option<String>,
}

impl RuntimeAuthConfig {
    pub fn native_cli_auth() -> Self {
        Self {
            style: RuntimeAuthStyle::NativeCliAuth,
            endpoint: None,
            profile_id: None,
            account_id: None,
            model: None,
            api_format: None,
            health_status: None,
        }
    }
}

impl Default for RuntimeAuthConfig {
    fn default() -> Self {
        Self::native_cli_auth()
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeAuthStyle {
    /// Launch the official local CLI and let it read its own auth store.
    NativeCliAuth,
    /// Talk to a loopback broker/proxy that owns provider accounts and tokens.
    LocalAuthBroker,
}

#[derive(Debug, thiserror::Error)]
pub enum ProjectConfigWriteError {
    #[error("create directory {path}: {source}")]
    CreateDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("project config: {0}")]
    Config(#[from] ProjectConfigError),
    #[error("encode project config {path}: {source}")]
    Encode {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("write project config {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("read local config or credential {path}: {source}")]
    ReadLocal {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("invalid local config import options: {0}")]
    InvalidOptions(String),
}

pub fn write_advisor_endpoint_config(
    workspace_root: impl AsRef<Path>,
    update: AdvisorEndpointConfigUpdate,
) -> Result<ProjectConfig, ProjectConfigWriteError> {
    let root = workspace_root.as_ref().join(".agent-monitor");
    validate_advisor_endpoint_update(&update, &root)?;

    fs::create_dir_all(&root).map_err(|source| ProjectConfigWriteError::CreateDir {
        path: root.clone(),
        source,
    })?;
    let mut config = ProjectConfig::load(&root)?;
    config.advisor.enabled = update.enabled;
    config.advisor.provider.kind = AdvisorProviderKind::OpenAiCompatible;
    config.advisor.provider.endpoint = update.endpoint.trim().to_string();
    config.advisor.provider.model = update.model.trim().to_string();
    config.advisor.provider.api_key_env = update.api_key_env.trim().to_string();
    if let Some(credential_source) = update.credential_source {
        config.advisor.provider.credential_source = credential_source;
        config.advisor.provider.credential_file =
            if credential_source == AdvisorCredentialSource::Env {
                None
            } else {
                update
                    .credential_file
                    .map(|file| file.trim().to_string())
                    .filter(|file| !file.is_empty())
            };
    } else if config.advisor.provider.credential_source == AdvisorCredentialSource::Env {
        config.advisor.provider.credential_file = None;
    }
    write_project_config(&root, &config)?;
    Ok(config)
}

pub fn write_adapter_runtime_auth_config(
    workspace_root: impl AsRef<Path>,
    agent: AgentKind,
    runtime_auth: RuntimeAuthConfig,
) -> Result<ProjectConfig, ProjectConfigWriteError> {
    validate_runtime_auth_config(agent, &runtime_auth)?;
    let root = workspace_root.as_ref().join(".agent-monitor");
    fs::create_dir_all(&root).map_err(|source| ProjectConfigWriteError::CreateDir {
        path: root.clone(),
        source,
    })?;
    let mut config = ProjectConfig::load(&root)?;
    adapter_override_mut(&mut config, agent).runtime_auth = Some(runtime_auth);
    // Runtime-auth updates do not modify advisor credentials. Existing advisor
    // config may be intentionally offline or incompatible while the user fixes
    // adapter launch metadata, so only validate the field being changed.
    write_project_config_without_advisor_validation(&root, &config)?;
    Ok(config)
}

fn adapter_override_mut(config: &mut ProjectConfig, agent: AgentKind) -> &mut AdapterOverride {
    match agent {
        AgentKind::Codex => &mut config.adapters.codex,
        AgentKind::ClaudeCode => &mut config.adapters.claude_code,
        AgentKind::Pi => &mut config.adapters.pi,
        AgentKind::OpenCode => &mut config.adapters.opencode,
    }
}

fn validate_runtime_auth_config(
    agent: AgentKind,
    runtime_auth: &RuntimeAuthConfig,
) -> Result<(), ProjectConfigWriteError> {
    // Treat runtime-auth metadata as untrusted: config.json can be edited by
    // hand, and capability data can flow into case files and packets.
    match runtime_auth.style {
        RuntimeAuthStyle::NativeCliAuth => {
            if runtime_auth
                .endpoint
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
            {
                return Err(ProjectConfigWriteError::InvalidOptions(
                    "native_cli_auth must not include a broker endpoint".into(),
                ));
            }
        }
        RuntimeAuthStyle::LocalAuthBroker => {
            let endpoint = runtime_auth
                .endpoint
                .as_deref()
                .filter(|endpoint| !endpoint.trim().is_empty())
                .ok_or_else(|| {
                    ProjectConfigWriteError::InvalidOptions(
                        "local_auth_broker requires endpoint".into(),
                    )
                })?;
            if !is_loopback_http_endpoint(endpoint) {
                return Err(ProjectConfigWriteError::InvalidOptions(
                    "local_auth_broker endpoint must be http(s) loopback or localhost".into(),
                ));
            }
        }
    }

    for (field, value) in runtime_auth_string_fields(runtime_auth) {
        if let Some(value) = value
            && runtime_auth_field_is_secret_like(value)
        {
            return Err(ProjectConfigWriteError::InvalidOptions(format!(
                "runtime auth {field} contains secret-like or local CLI auth material"
            )));
        }
    }

    if matches!(agent, AgentKind::Codex | AgentKind::ClaudeCode)
        && runtime_auth.style == RuntimeAuthStyle::LocalAuthBroker
        && runtime_auth
            .profile_id
            .as_deref()
            .is_none_or(|value| value.trim().is_empty())
    {
        return Err(ProjectConfigWriteError::InvalidOptions(
            "local_auth_broker requires profile_id for Codex and Claude Code".into(),
        ));
    }

    Ok(())
}

pub(crate) fn runtime_auth_config_is_safe_for_capabilities(
    agent: AgentKind,
    runtime_auth: &RuntimeAuthConfig,
) -> bool {
    // Adapter capabilities are advisor-visible. Invalid metadata is safer to
    // omit than to expose as a degraded-but-present auth surface.
    validate_runtime_auth_config(agent, runtime_auth).is_ok()
}

fn runtime_auth_string_fields(
    runtime_auth: &RuntimeAuthConfig,
) -> [(&'static str, Option<&str>); 6] {
    [
        ("endpoint", runtime_auth.endpoint.as_deref()),
        ("profile_id", runtime_auth.profile_id.as_deref()),
        ("account_id", runtime_auth.account_id.as_deref()),
        ("model", runtime_auth.model.as_deref()),
        ("api_format", runtime_auth.api_format.as_deref()),
        ("health_status", runtime_auth.health_status.as_deref()),
    ]
}

fn runtime_auth_field_is_secret_like(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    lower.contains("access_token")
        || lower.contains("refresh_token")
        || lower.contains("authorization")
        || lower.contains("x-api-key")
        || lower.contains("api_key")
        || lower.contains("bearer ")
        || lower.contains("token=")
        || lower.contains("secret=")
        || lower.contains("password=")
        || lower.contains("sk-")
        || lower.contains(".codex")
        || lower.contains(".claude")
        || lower.contains("auth.json")
        || lower.contains(".credentials")
        || looks_like_jwt_bearer_token(value)
}

fn is_loopback_http_endpoint(endpoint: &str) -> bool {
    let endpoint = endpoint.trim();
    let Some(rest) = endpoint
        .strip_prefix("http://")
        .or_else(|| endpoint.strip_prefix("https://"))
    else {
        return false;
    };
    let authority = rest
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default()
        .trim();
    if authority.is_empty() || authority.contains('@') {
        return false;
    }
    let host = if let Some(rest) = authority.strip_prefix('[') {
        let Some((host, _)) = rest.split_once(']') else {
            return false;
        };
        host
    } else {
        authority.split(':').next().unwrap_or_default()
    }
    .trim()
    .to_ascii_lowercase();

    host == "localhost" || host == "127.0.0.1" || host.starts_with("127.") || host == "::1"
}

pub fn write_verifier_config(
    workspace_root: impl AsRef<Path>,
    mut verifier: VerifierConfig,
) -> Result<ProjectConfig, ProjectConfigWriteError> {
    sanitize_verifier_config(&mut verifier)?;
    let root = workspace_root.as_ref().join(".agent-monitor");
    fs::create_dir_all(&root).map_err(|source| ProjectConfigWriteError::CreateDir {
        path: root.clone(),
        source,
    })?;
    let mut config = ProjectConfig::load(&root)?;
    if let Some(existing) = config
        .verifiers
        .iter_mut()
        .find(|existing| existing.id == verifier.id)
    {
        *existing = verifier;
    } else {
        config.verifiers.push(verifier);
    }
    write_project_config_without_advisor_validation(&root, &config)?;
    Ok(config)
}

fn sanitize_verifier_config(verifier: &mut VerifierConfig) -> Result<(), ProjectConfigWriteError> {
    verifier.id = verifier.id.trim().to_string();
    verifier.command = verifier.command.trim().to_string();
    verifier.paths = verifier
        .paths
        .iter()
        .map(|path| path.trim().to_string())
        .filter(|path| !path.is_empty())
        .collect();
    verifier.acceptance_patterns = verifier
        .acceptance_patterns
        .iter()
        .map(|pattern| pattern.trim().to_string())
        .filter(|pattern| !pattern.is_empty())
        .collect();
    if verifier.id.is_empty() {
        return Err(ProjectConfigWriteError::InvalidOptions(
            "verifier id must not be empty".into(),
        ));
    }
    if verifier.command.is_empty() {
        return Err(ProjectConfigWriteError::InvalidOptions(
            "verifier command must not be empty".into(),
        ));
    }
    if verifier.timeout_secs == 0 {
        return Err(ProjectConfigWriteError::InvalidOptions(
            "verifier timeout_secs must be greater than zero".into(),
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LocalAgentConfigImportOptions {
    pub codex: bool,
    pub claude_code: bool,
    pub copy_credentials: bool,
    pub advisor_credential_source: Option<AdvisorCredentialSource>,
    pub advisor_credential_file: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct LocalAgentConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codex: Option<LocalCodexConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub claude_code: Option<LocalClaudeCodeConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct LocalCodexConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_reasoning_effort: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_policy: Option<String>,
    #[serde(default)]
    pub command: Vec<String>,
    #[serde(default)]
    pub uses_native_auth: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_auth: Option<RuntimeAuthConfig>,
    #[serde(
        default,
        alias = "credential_file",
        skip_serializing_if = "Option::is_none"
    )]
    pub advisor_credential_file: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct LocalClaudeCodeConfig {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effort_level: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tui: Option<String>,
    #[serde(default)]
    pub enabled_plugins: Vec<String>,
    #[serde(default)]
    pub env_keys: Vec<String>,
    #[serde(default)]
    pub command: Vec<String>,
    #[serde(default)]
    pub uses_native_auth: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_auth: Option<RuntimeAuthConfig>,
    #[serde(
        default,
        alias = "credential_file",
        skip_serializing_if = "Option::is_none"
    )]
    pub advisor_credential_file: Option<String>,
}

pub fn import_local_agent_configs(
    workspace_root: impl AsRef<Path>,
    home_dir: impl AsRef<Path>,
    options: LocalAgentConfigImportOptions,
) -> Result<ProjectConfig, ProjectConfigWriteError> {
    if options.copy_credentials {
        return Err(ProjectConfigWriteError::InvalidOptions(
            "local Codex/Claude CLI credentials cannot be copied into the project; link dedicated advisor credentials with --advisor-credential-file instead".into(),
        ));
    }
    let root = workspace_root.as_ref().join(".agent-monitor");
    validate_local_import_advisor_credentials(&options, &root)?;
    let advisor_reference_updated = options.advisor_credential_file.is_some();

    fs::create_dir_all(&root).map_err(|source| ProjectConfigWriteError::CreateDir {
        path: root.clone(),
        source,
    })?;
    let mut config = ProjectConfig::load(&root)?;
    let home_dir = home_dir.as_ref();
    let mut codex_model_hint = None;

    if options.codex
        && let Some(codex) = read_codex_config(home_dir)?
    {
        let codex_model = codex.model.clone();
        codex_model_hint = codex_model.clone();
        config.local_agents.codex = Some(codex);
        config.adapters.codex.enabled = Some(true);
        config.adapters.codex.ingest_jsonl = Some(true);
        config.adapters.codex.can_run_headless = Some(true);
        config.adapters.codex.can_inject_context = Some(true);
        config.adapters.codex.supports_workspace_write_mode = Some(true);
        config.adapters.codex.runtime_auth = Some(RuntimeAuthConfig::native_cli_auth());
    }

    if options.claude_code
        && let Some(claude_code) = read_claude_code_config(home_dir)?
    {
        config.local_agents.claude_code = Some(claude_code);
        config.adapters.claude_code.enabled = Some(true);
        config.adapters.claude_code.hook_pre_tool = Some(true);
        config.adapters.claude_code.hook_post_tool = Some(true);
        config.adapters.claude_code.can_block_tool = Some(true);
        config.adapters.claude_code.can_inject_context = Some(true);
        config.adapters.claude_code.can_run_headless = Some(true);
        config.adapters.claude_code.supports_workspace_write_mode = Some(true);
        config.adapters.claude_code.runtime_auth = Some(RuntimeAuthConfig::native_cli_auth());
    }

    if let Some(credential_file) = options.advisor_credential_file.as_deref() {
        configure_advisor_credential_reference(
            &mut config,
            options
                .advisor_credential_source
                .unwrap_or(AdvisorCredentialSource::CodingPlan),
            credential_file,
            codex_model_hint,
        );
    }

    if advisor_reference_updated {
        write_project_config(&root, &config)?;
    } else {
        // Plain local imports only record non-secret CLI metadata such as model
        // hints, command shape, and native-auth style. Do not let an unrelated
        // stale advisor profile block that adapter setup path.
        write_project_config_without_advisor_validation(&root, &config)?;
    }
    Ok(config)
}

pub fn import_coding_plan_advisor_credentials(
    workspace_root: impl AsRef<Path>,
    source_file: impl AsRef<Path>,
    endpoint: Option<&str>,
    model: Option<&str>,
) -> Result<ProjectConfig, ProjectConfigWriteError> {
    let source_file = source_file.as_ref();
    if let Some(cli_dir) = local_cli_auth_profile_dir(source_file) {
        return Err(ProjectConfigWriteError::InvalidOptions(format!(
            "coding_plan credential source {} points at local CLI auth directory {cli_dir}; provide a dedicated coding-plan credential profile outside Codex/Claude CLI config",
            source_file.display()
        )));
    }
    let source_content =
        fs::read_to_string(source_file).map_err(|source| ProjectConfigWriteError::ReadLocal {
            path: source_file.to_path_buf(),
            source,
        })?;
    let source_json: serde_json::Value =
        serde_json::from_str(&source_content).map_err(|error| {
            ProjectConfigWriteError::InvalidOptions(format!(
                "coding_plan credential source {} must be valid JSON: {error}",
                source_file.display()
            ))
        })?;
    let token = coding_plan_advisor_token(&source_json).ok_or_else(|| {
        ProjectConfigWriteError::InvalidOptions(format!(
            "coding_plan credential source {} must contain a supported advisor token field",
            source_file.display()
        ))
    })?;
    let root = workspace_root.as_ref().join(".agent-monitor");
    let mut config = ProjectConfig::load(&root)?;
    let endpoint = optional_non_empty_config_value("endpoint", endpoint)?
        .or_else(|| coding_plan_profile_endpoint(&source_json));
    let model = optional_non_empty_config_value("model", model)?
        .or_else(|| coding_plan_profile_model(&source_json));
    let candidate_endpoint = endpoint
        .as_deref()
        .filter(|endpoint| !endpoint.trim().is_empty())
        .or_else(|| {
            let configured = config.advisor.provider.endpoint.trim();
            (!configured.is_empty()).then_some(configured)
        })
        .map(str::to_string)
        .unwrap_or_else(default_openai_chat_completions_endpoint);
    validate_coding_plan_endpoint_compatibility(&token, &candidate_endpoint)?;

    let credential_dir = root.join("credentials").join("coding-plan");
    fs::create_dir_all(&credential_dir).map_err(|source| ProjectConfigWriteError::CreateDir {
        path: credential_dir.clone(),
        source,
    })?;
    let credential_path = credential_dir.join("auth.json");
    let credential_profile = serde_json::json!({
        "OPENAI_API_KEY": token,
    });
    let credential_content =
        serde_json::to_string_pretty(&credential_profile).map_err(|source| {
            ProjectConfigWriteError::Encode {
                path: credential_path.clone(),
                source,
            }
        })?;
    fs::write(&credential_path, format!("{credential_content}\n")).map_err(|source| {
        ProjectConfigWriteError::Write {
            path: credential_path.clone(),
            source,
        }
    })?;

    config.advisor.enabled = true;
    config.advisor.provider.kind = AdvisorProviderKind::OpenAiCompatible;
    if let Some(endpoint) = endpoint {
        config.advisor.provider.endpoint = endpoint;
    } else if config.advisor.provider.endpoint.trim().is_empty() {
        config.advisor.provider.endpoint = default_openai_chat_completions_endpoint();
    }
    if let Some(model) = model {
        config.advisor.provider.model = model;
    }
    config.advisor.provider.api_key_env = default_api_key_env();
    config.advisor.provider.credential_source = AdvisorCredentialSource::CodingPlan;
    config.advisor.provider.credential_file = Some("credentials/coding-plan/auth.json".into());
    write_project_config(&root, &config)?;
    Ok(config)
}

fn optional_non_empty_config_value(
    label: &str,
    value: Option<&str>,
) -> Result<Option<String>, ProjectConfigWriteError> {
    value
        .map(|value| {
            let value = value.trim();
            if value.is_empty() {
                Err(ProjectConfigWriteError::InvalidOptions(format!(
                    "{label} must not be empty"
                )))
            } else {
                Ok(value.to_string())
            }
        })
        .transpose()
}

fn validate_advisor_endpoint_update(
    update: &AdvisorEndpointConfigUpdate,
    store_root: &Path,
) -> Result<(), ProjectConfigWriteError> {
    match update.credential_source {
        Some(AdvisorCredentialSource::Env) if has_non_empty_value(&update.credential_file) => {
            Err(ProjectConfigWriteError::InvalidOptions(
                "credential_file requires a non-env credential_source".into(),
            ))
        }
        Some(source)
            if source != AdvisorCredentialSource::Env
                && !has_non_empty_value(&update.credential_file) =>
        {
            Err(ProjectConfigWriteError::InvalidOptions(format!(
                "credential_source {} requires credential_file",
                advisor_credential_source_name(source)
            )))
        }
        None if has_non_empty_value(&update.credential_file) => {
            Err(ProjectConfigWriteError::InvalidOptions(
                "credential_file requires credential_source".into(),
            ))
        }
        Some(source) if source != AdvisorCredentialSource::Env => {
            validate_advisor_credential_profile_for_endpoint(
                source,
                update.credential_file.as_deref(),
                store_root,
                "credential_file",
                update.endpoint.as_str(),
            )
        }
        _ => Ok(()),
    }
}

fn validate_local_import_advisor_credentials(
    options: &LocalAgentConfigImportOptions,
    store_root: &Path,
) -> Result<(), ProjectConfigWriteError> {
    if options
        .advisor_credential_file
        .as_deref()
        .is_some_and(|file| file.trim().is_empty())
    {
        return Err(ProjectConfigWriteError::InvalidOptions(
            "advisor credential_file must not be empty".into(),
        ));
    }
    match options.advisor_credential_source {
        Some(AdvisorCredentialSource::Env)
            if has_non_empty_value(&options.advisor_credential_file) =>
        {
            Err(ProjectConfigWriteError::InvalidOptions(
                "advisor credential_file requires a non-env advisor credential_source".into(),
            ))
        }
        Some(source)
            if source != AdvisorCredentialSource::Env
                && !has_non_empty_value(&options.advisor_credential_file) =>
        {
            Err(ProjectConfigWriteError::InvalidOptions(format!(
                "advisor credential_source {} requires credential_file",
                advisor_credential_source_name(source)
            )))
        }
        Some(source) if source != AdvisorCredentialSource::Env => {
            validate_advisor_credential_profile(
                source,
                options.advisor_credential_file.as_deref(),
                store_root,
                "advisor credential_file",
            )
        }
        None if has_non_empty_value(&options.advisor_credential_file) => {
            validate_advisor_credential_profile(
                AdvisorCredentialSource::CodingPlan,
                options.advisor_credential_file.as_deref(),
                store_root,
                "advisor credential_file",
            )
        }
        _ => Ok(()),
    }
}

fn validate_advisor_credential_profile(
    source: AdvisorCredentialSource,
    credential_file: Option<&str>,
    store_root: &Path,
    label: &str,
) -> Result<(), ProjectConfigWriteError> {
    if source == AdvisorCredentialSource::ClaudePlan {
        return Err(ProjectConfigWriteError::InvalidOptions(
            "advisor credential_source claude_plan is no longer supported; use coding_plan with a dedicated provider credential profile".into(),
        ));
    }
    let credential_file = credential_file
        .map(str::trim)
        .filter(|file| !file.is_empty())
        .ok_or_else(|| {
            ProjectConfigWriteError::InvalidOptions(format!(
                "{label} is required for {}",
                advisor_credential_source_name(source)
            ))
        })?;
    let path = advisor_credential_profile_path(credential_file, store_root);
    if let Some(cli_dir) = local_cli_auth_profile_dir(&path) {
        return Err(ProjectConfigWriteError::InvalidOptions(format!(
            "{label} {} points at local CLI auth directory {cli_dir}; configure a dedicated coding-plan credential profile outside Codex/Claude CLI config",
            path.display()
        )));
    }
    let content =
        fs::read_to_string(&path).map_err(|source| ProjectConfigWriteError::ReadLocal {
            path: path.clone(),
            source,
        })?;
    let value: serde_json::Value = serde_json::from_str(&content).map_err(|error| {
        ProjectConfigWriteError::InvalidOptions(format!(
            "{} credential profile {} must be valid JSON: {error}",
            advisor_credential_source_name(source),
            path.display()
        ))
    })?;
    let Some(token) = advisor_credential_token(source, &value) else {
        return Err(ProjectConfigWriteError::InvalidOptions(format!(
            "{} credential profile {} must contain a supported advisor token field",
            advisor_credential_source_name(source),
            path.display()
        )));
    };
    validate_coding_plan_endpoint_compatibility(&token, "").or_else(|error| {
        if source == AdvisorCredentialSource::CodingPlan {
            Err(error)
        } else {
            Ok(())
        }
    })?;
    Ok(())
}

fn validate_advisor_credential_profile_for_endpoint(
    source: AdvisorCredentialSource,
    credential_file: Option<&str>,
    store_root: &Path,
    label: &str,
    endpoint: &str,
) -> Result<(), ProjectConfigWriteError> {
    if source == AdvisorCredentialSource::ClaudePlan {
        return Err(ProjectConfigWriteError::InvalidOptions(
            "advisor credential_source claude_plan is no longer supported; use coding_plan with a dedicated provider credential profile".into(),
        ));
    }
    let credential_file = credential_file
        .map(str::trim)
        .filter(|file| !file.is_empty())
        .ok_or_else(|| {
            ProjectConfigWriteError::InvalidOptions(format!(
                "{label} is required for {}",
                advisor_credential_source_name(source)
            ))
        })?;
    let path = advisor_credential_profile_path(credential_file, store_root);
    if let Some(cli_dir) = local_cli_auth_profile_dir(&path) {
        return Err(ProjectConfigWriteError::InvalidOptions(format!(
            "{label} {} points at local CLI auth directory {cli_dir}; configure a dedicated coding-plan credential profile outside Codex/Claude CLI config",
            path.display()
        )));
    }
    let content =
        fs::read_to_string(&path).map_err(|source| ProjectConfigWriteError::ReadLocal {
            path: path.clone(),
            source,
        })?;
    let value: serde_json::Value = serde_json::from_str(&content).map_err(|error| {
        ProjectConfigWriteError::InvalidOptions(format!(
            "{} credential profile {} must be valid JSON: {error}",
            advisor_credential_source_name(source),
            path.display()
        ))
    })?;
    let Some(token) = advisor_credential_token(source, &value) else {
        return Err(ProjectConfigWriteError::InvalidOptions(format!(
            "{} credential profile {} must contain a supported advisor token field",
            advisor_credential_source_name(source),
            path.display()
        )));
    };
    validate_coding_plan_endpoint_compatibility(&token, endpoint)?;
    Ok(())
}

fn advisor_credential_profile_path(credential_file: &str, store_root: &Path) -> PathBuf {
    let path = Path::new(credential_file);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        store_root.join(path)
    }
}

fn advisor_credential_token(
    source: AdvisorCredentialSource,
    value: &serde_json::Value,
) -> Option<String> {
    match source {
        AdvisorCredentialSource::Env => Some(String::new()),
        AdvisorCredentialSource::CodingPlan => coding_plan_advisor_token(value),
        AdvisorCredentialSource::ClaudePlan => None,
    }
}

fn coding_plan_advisor_token(value: &serde_json::Value) -> Option<String> {
    credential_string_at_any(
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

fn coding_plan_profile_endpoint(value: &serde_json::Value) -> Option<String> {
    credential_string_at_any(
        value,
        &[
            "/endpoint",
            "/base_url",
            "/baseUrl",
            "/provider/endpoint",
            "/provider/base_url",
            "/advisor/endpoint",
            "/advisor/provider/endpoint",
        ],
    )
}

fn coding_plan_profile_model(value: &serde_json::Value) -> Option<String> {
    credential_string_at_any(
        value,
        &[
            "/model",
            "/provider/model",
            "/advisor/model",
            "/advisor/provider/model",
        ],
    )
}

fn validate_coding_plan_endpoint_compatibility(
    token: &str,
    endpoint: &str,
) -> Result<(), ProjectConfigWriteError> {
    if !looks_like_jwt_bearer_token(token) || !is_public_openai_endpoint(endpoint) {
        return Ok(());
    }
    Err(ProjectConfigWriteError::InvalidOptions(format!(
        "JWT/OAuth-style coding-plan credential is not compatible with advisor endpoint {endpoint}; configure a dedicated provider/proxy endpoint that accepts coding-plan bearer tokens"
    )))
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
    endpoint_host(endpoint)
        .as_deref()
        .is_some_and(|host| host.eq_ignore_ascii_case("api.openai.com"))
}

fn endpoint_host(endpoint: &str) -> Option<String> {
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

fn local_cli_auth_profile_dir(path: &Path) -> Option<&'static str> {
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

fn credential_string_at_any(value: &serde_json::Value, pointers: &[&str]) -> Option<String> {
    pointers
        .iter()
        .find_map(|pointer| value.pointer(pointer).and_then(serde_json::Value::as_str))
        .map(|token| token.trim().to_string())
        .filter(|token| !token.is_empty())
}

fn has_non_empty_value(value: &Option<String>) -> bool {
    value
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
}

fn advisor_credential_source_name(source: AdvisorCredentialSource) -> &'static str {
    match source {
        AdvisorCredentialSource::Env => "env",
        AdvisorCredentialSource::CodingPlan => "coding_plan",
        AdvisorCredentialSource::ClaudePlan => "claude_plan",
    }
}

fn configure_advisor_credential_reference(
    config: &mut ProjectConfig,
    credential_source: AdvisorCredentialSource,
    credential_file: &str,
    model_hint: Option<String>,
) {
    config.advisor.enabled = true;
    if config.advisor.provider.endpoint.trim().is_empty() {
        config.advisor.provider.endpoint = default_openai_chat_completions_endpoint();
    }
    if config.advisor.provider.model.trim().is_empty()
        && let Some(model) = model_hint
    {
        config.advisor.provider.model = model;
    }
    config.advisor.provider.credential_source = credential_source;
    config.advisor.provider.credential_file = if credential_source == AdvisorCredentialSource::Env {
        None
    } else {
        Some(credential_file.trim().to_string())
    };
}

fn write_project_config(
    store_root: &Path,
    config: &ProjectConfig,
) -> Result<(), ProjectConfigWriteError> {
    // Use the strict path whenever a write can create or alter advisor
    // credential references. This keeps bad token/endpoint combinations out at
    // the boundary where they are introduced.
    validate_project_config_for_write(config, store_root)?;
    write_project_config_content(store_root, config)
}

fn write_project_config_without_advisor_validation(
    store_root: &Path,
    config: &ProjectConfig,
) -> Result<(), ProjectConfigWriteError> {
    // Narrow config writes use this path when they are not changing advisor
    // fields. It preserves an existing broken advisor profile so users can fix
    // adapter/verifier config independently.
    write_project_config_content(store_root, config)
}

fn write_project_config_content(
    store_root: &Path,
    config: &ProjectConfig,
) -> Result<(), ProjectConfigWriteError> {
    let path = store_root.join("config.json");
    let content = serde_json::to_string_pretty(&config).map_err(|source| {
        ProjectConfigWriteError::Encode {
            path: path.clone(),
            source,
        }
    })?;
    fs::write(&path, format!("{content}\n")).map_err(|source| ProjectConfigWriteError::Write {
        path: path.clone(),
        source,
    })?;
    Ok(())
}

fn validate_project_config_for_write(
    config: &ProjectConfig,
    store_root: &Path,
) -> Result<(), ProjectConfigWriteError> {
    if !config.advisor.enabled {
        return Ok(());
    }

    match config.advisor.provider.credential_source {
        AdvisorCredentialSource::Env => {
            if has_non_empty_value(&config.advisor.provider.credential_file) {
                return Err(ProjectConfigWriteError::InvalidOptions(
                    "advisor credential_file requires a non-env credential_source".into(),
                ));
            }
            Ok(())
        }
        source => validate_advisor_credential_profile_for_endpoint(
            source,
            config.advisor.provider.credential_file.as_deref(),
            store_root,
            "advisor credential_file",
            config.advisor.provider.endpoint.as_str(),
        ),
    }
}

fn read_codex_config(home_dir: &Path) -> Result<Option<LocalCodexConfig>, ProjectConfigWriteError> {
    let path = home_dir.join(".codex").join("config.toml");
    let Some(content) = read_optional_text(&path)? else {
        return Ok(None);
    };
    Ok(Some(LocalCodexConfig {
        model: toml_string_value(&content, "model"),
        model_reasoning_effort: toml_string_value(&content, "model_reasoning_effort"),
        sandbox_mode: toml_string_value(&content, "sandbox_mode"),
        approval_policy: toml_string_value(&content, "approval_policy"),
        command: vec!["codex".into(), "exec".into(), "--json".into()],
        uses_native_auth: true,
        runtime_auth: Some(RuntimeAuthConfig::native_cli_auth()),
        advisor_credential_file: None,
    }))
}

fn read_claude_code_config(
    home_dir: &Path,
) -> Result<Option<LocalClaudeCodeConfig>, ProjectConfigWriteError> {
    let path = home_dir.join(".claude").join("settings.json");
    let Some(content) = read_optional_text(&path)? else {
        return Ok(None);
    };
    let value: serde_json::Value =
        serde_json::from_str(&content).map_err(|source| ProjectConfigWriteError::Encode {
            path: path.clone(),
            source,
        })?;
    let mut enabled_plugins = object_keys_with_bool(&value, "enabledPlugins", true);
    enabled_plugins.sort();
    let mut env_keys = value
        .get("env")
        .and_then(serde_json::Value::as_object)
        .map(|env| env.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    env_keys.sort();

    Ok(Some(LocalClaudeCodeConfig {
        effort_level: json_string_value(&value, "effortLevel"),
        tui: json_string_value(&value, "tui"),
        enabled_plugins,
        env_keys,
        command: vec!["claude".into()],
        uses_native_auth: true,
        runtime_auth: Some(RuntimeAuthConfig::native_cli_auth()),
        advisor_credential_file: None,
    }))
}

fn read_optional_text(path: &Path) -> Result<Option<String>, ProjectConfigWriteError> {
    match fs::read_to_string(path) {
        Ok(content) => Ok(Some(content)),
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(ProjectConfigWriteError::ReadLocal {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn toml_string_value(content: &str, key: &str) -> Option<String> {
    content.lines().find_map(|line| {
        let line = line.trim();
        if line.starts_with('#') {
            return None;
        }
        let (found_key, raw_value) = line.split_once('=')?;
        if found_key.trim() != key {
            return None;
        }
        parse_quoted_value(raw_value.trim())
    })
}

fn parse_quoted_value(raw: &str) -> Option<String> {
    let value = raw.strip_prefix('"')?.split('"').next()?;
    Some(value.to_string())
}

fn json_string_value(value: &serde_json::Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

fn object_keys_with_bool(value: &serde_json::Value, key: &str, expected: bool) -> Vec<String> {
    value
        .get(key)
        .and_then(serde_json::Value::as_object)
        .map(|object| {
            object
                .iter()
                .filter(|(_, value)| value.as_bool() == Some(expected))
                .map(|(key, _)| key.clone())
                .collect()
        })
        .unwrap_or_default()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdvisorConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub provider: AdvisorProviderConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdvisorProviderConfig {
    #[serde(default)]
    pub kind: AdvisorProviderKind,
    #[serde(default)]
    pub endpoint: String,
    #[serde(default)]
    pub model: String,
    #[serde(default = "default_api_key_env")]
    pub api_key_env: String,
    #[serde(default)]
    pub credential_source: AdvisorCredentialSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_file: Option<String>,
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
    #[serde(default = "default_max_input_tokens")]
    pub max_input_tokens: u32,
    #[serde(default = "default_max_output_tokens")]
    pub max_output_tokens: u32,
}

impl Default for AdvisorProviderConfig {
    fn default() -> Self {
        Self {
            kind: AdvisorProviderKind::OpenAiCompatible,
            endpoint: String::new(),
            model: String::new(),
            api_key_env: default_api_key_env(),
            credential_source: AdvisorCredentialSource::Env,
            credential_file: None,
            timeout_secs: default_timeout_secs(),
            max_input_tokens: default_max_input_tokens(),
            max_output_tokens: default_max_output_tokens(),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AdvisorCredentialSource {
    #[default]
    Env,
    CodingPlan,
    ClaudePlan,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum AdvisorProviderKind {
    #[serde(rename = "openai_compatible")]
    #[default]
    OpenAiCompatible,
}

#[derive(Debug, thiserror::Error)]
pub enum ProjectConfigError {
    #[error("read project config {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("decode project config {path}: {source}")]
    Decode {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PolicyConfig {
    #[serde(default = "default_max_user_questions_per_hour")]
    pub max_user_questions_per_hour: u32,
    #[serde(default = "default_switch_agent_cooldown_min")]
    pub switch_agent_cooldown_min: u32,
    #[serde(default = "default_spawn_fresh_cooldown_min")]
    pub spawn_fresh_cooldown_min: u32,
    #[serde(default = "default_max_parallel_writable_agents")]
    pub max_parallel_writable_agents: u32,
    #[serde(default = "default_require_verification_after_source_change")]
    pub require_verification_after_source_change: bool,
    #[serde(default = "default_allow_docs_only_continue_without_tests")]
    pub allow_docs_only_continue_without_tests: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree_lock_stale_after_secs: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handoff_outcome_timeout_secs: Option<i64>,
}

impl Default for PolicyConfig {
    fn default() -> Self {
        Self {
            max_user_questions_per_hour: default_max_user_questions_per_hour(),
            switch_agent_cooldown_min: default_switch_agent_cooldown_min(),
            spawn_fresh_cooldown_min: default_spawn_fresh_cooldown_min(),
            max_parallel_writable_agents: default_max_parallel_writable_agents(),
            require_verification_after_source_change:
                default_require_verification_after_source_change(),
            allow_docs_only_continue_without_tests: default_allow_docs_only_continue_without_tests(
            ),
            worktree_lock_stale_after_secs: None,
            handoff_outcome_timeout_secs: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SecurityConfig {
    #[serde(default = "default_security_redact_env")]
    pub redact_env: bool,
    #[serde(default = "default_security_redact_auth_files")]
    pub redact_auth_files: bool,
    #[serde(default = "default_security_deny_paths")]
    pub deny_paths: Vec<String>,
    #[serde(default = "default_security_protected_paths")]
    pub protected_paths: Vec<String>,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            redact_env: default_security_redact_env(),
            redact_auth_files: default_security_redact_auth_files(),
            deny_paths: default_security_deny_paths(),
            protected_paths: default_security_protected_paths(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VerifierConfig {
    pub id: String,
    pub command: String,
    #[serde(default)]
    pub scope: VerificationScope,
    #[serde(default = "default_verifier_timeout_secs")]
    pub timeout_secs: u64,
    #[serde(default)]
    pub paths: Vec<String>,
    #[serde(default)]
    pub acceptance_patterns: Vec<String>,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VerificationScope {
    #[default]
    Full,
    Targeted,
    Style,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct AdapterConfig {
    #[serde(default)]
    pub codex: AdapterOverride,
    #[serde(default)]
    pub claude_code: AdapterOverride,
    #[serde(default)]
    pub opencode: AdapterOverride,
    #[serde(default)]
    pub pi: AdapterOverride,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct AdapterOverride {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ingest_jsonl: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hook_pre_tool: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hook_post_tool: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub can_block_tool: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub can_inject_context: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub can_run_headless: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub can_resume_session: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub can_export_session: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_readonly_mode: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_workspace_write_mode: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires_external_sandbox: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_auth: Option<RuntimeAuthConfig>,
}

fn default_api_key_env() -> String {
    "OPENAI_API_KEY".into()
}

fn default_openai_chat_completions_endpoint() -> String {
    "https://api.openai.com/v1/chat/completions".into()
}

fn default_timeout_secs() -> u64 {
    45
}

fn default_max_input_tokens() -> u32 {
    18_000
}

fn default_max_output_tokens() -> u32 {
    2_500
}

fn default_max_user_questions_per_hour() -> u32 {
    2
}

fn default_switch_agent_cooldown_min() -> u32 {
    20
}

fn default_spawn_fresh_cooldown_min() -> u32 {
    10
}

fn default_max_parallel_writable_agents() -> u32 {
    1
}

fn default_require_verification_after_source_change() -> bool {
    true
}

fn default_allow_docs_only_continue_without_tests() -> bool {
    true
}

fn default_security_redact_env() -> bool {
    true
}

fn default_security_redact_auth_files() -> bool {
    true
}

fn default_security_deny_paths() -> Vec<String> {
    Vec::new()
}

fn default_security_protected_paths() -> Vec<String> {
    vec![
        "migrations/**".into(),
        "infra/**".into(),
        ".github/workflows/**".into(),
    ]
}

fn default_verifier_timeout_secs() -> u64 {
    300
}
