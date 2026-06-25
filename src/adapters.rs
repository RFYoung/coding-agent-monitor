use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::str::FromStr;

use crate::{
    AdapterCapabilities, AdapterConfig, AdapterOverride, RuntimeAuthConfig,
    config::runtime_auth_config_is_safe_for_capabilities,
};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum AgentKind {
    Codex,
    ClaudeCode,
    Pi,
    OpenCode,
}

pub fn agent_kind_label(agent: AgentKind) -> &'static str {
    match agent {
        AgentKind::Codex => "codex",
        AgentKind::ClaudeCode => "claude-code",
        AgentKind::Pi => "pi",
        AgentKind::OpenCode => "opencode",
    }
}

pub fn adapter_capabilities_for(agent: AgentKind) -> AdapterCapabilities {
    match agent {
        AgentKind::Codex => AdapterCapabilities {
            enabled: true,
            ingest_transcript: true,
            ingest_jsonl: true,
            hook_pre_tool: true,
            hook_post_tool: true,
            hook_stop: true,
            can_block_tool: true,
            can_rewrite_tool_input: false,
            can_inject_context: true,
            can_run_headless: true,
            can_resume_session: true,
            can_export_session: false,
            can_start_subagent: false,
            can_switch_mode: true,
            supports_readonly_mode: true,
            supports_workspace_write_mode: true,
            requires_external_sandbox: false,
            runtime_auth: Some(RuntimeAuthConfig::native_cli_auth()),
        },
        AgentKind::ClaudeCode => AdapterCapabilities {
            enabled: true,
            ingest_transcript: true,
            ingest_jsonl: true,
            hook_pre_tool: true,
            hook_post_tool: true,
            hook_stop: true,
            can_block_tool: true,
            can_rewrite_tool_input: true,
            can_inject_context: true,
            can_run_headless: true,
            can_resume_session: true,
            can_export_session: true,
            can_start_subagent: true,
            can_switch_mode: true,
            supports_readonly_mode: true,
            supports_workspace_write_mode: true,
            requires_external_sandbox: false,
            runtime_auth: Some(RuntimeAuthConfig::native_cli_auth()),
        },
        AgentKind::Pi => AdapterCapabilities {
            enabled: true,
            ingest_transcript: true,
            ingest_jsonl: false,
            hook_pre_tool: false,
            hook_post_tool: false,
            hook_stop: false,
            can_block_tool: false,
            can_rewrite_tool_input: false,
            can_inject_context: true,
            can_run_headless: true,
            can_resume_session: false,
            can_export_session: false,
            can_start_subagent: false,
            can_switch_mode: false,
            supports_readonly_mode: false,
            supports_workspace_write_mode: false,
            requires_external_sandbox: true,
            runtime_auth: Some(RuntimeAuthConfig::native_cli_auth()),
        },
        AgentKind::OpenCode => AdapterCapabilities {
            enabled: true,
            ingest_transcript: true,
            ingest_jsonl: true,
            hook_pre_tool: true,
            hook_post_tool: true,
            hook_stop: true,
            can_block_tool: true,
            can_rewrite_tool_input: false,
            can_inject_context: true,
            can_run_headless: true,
            can_resume_session: true,
            can_export_session: true,
            can_start_subagent: false,
            can_switch_mode: true,
            supports_readonly_mode: true,
            supports_workspace_write_mode: true,
            requires_external_sandbox: false,
            runtime_auth: Some(RuntimeAuthConfig::native_cli_auth()),
        },
    }
}

pub fn adapter_capabilities_for_config(
    agent: AgentKind,
    config: &AdapterConfig,
) -> AdapterCapabilities {
    let mut capabilities = adapter_capabilities_for(agent);
    let overrides = match agent {
        AgentKind::Codex => &config.codex,
        AgentKind::ClaudeCode => &config.claude_code,
        AgentKind::Pi => &config.pi,
        AgentKind::OpenCode => &config.opencode,
    };
    apply_adapter_override(agent, &mut capabilities, overrides);
    capabilities
}

pub(crate) fn adapter_capabilities_from_config(
    config: &AdapterConfig,
) -> BTreeMap<String, AdapterCapabilities> {
    [
        AgentKind::Codex,
        AgentKind::ClaudeCode,
        AgentKind::OpenCode,
        AgentKind::Pi,
    ]
    .into_iter()
    .map(|agent| {
        (
            agent_kind_label(agent).to_string(),
            adapter_capabilities_for_config(agent, config),
        )
    })
    .collect()
}

fn apply_adapter_override(
    agent: AgentKind,
    capabilities: &mut AdapterCapabilities,
    overrides: &AdapterOverride,
) {
    if let Some(value) = overrides.enabled {
        capabilities.enabled = value;
    }
    if let Some(value) = overrides.ingest_jsonl {
        capabilities.ingest_jsonl = value;
    }
    if let Some(value) = overrides.hook_pre_tool {
        capabilities.hook_pre_tool = value;
    }
    if let Some(value) = overrides.hook_post_tool {
        capabilities.hook_post_tool = value;
    }
    if let Some(value) = overrides.can_block_tool {
        capabilities.can_block_tool = value;
    }
    if let Some(value) = overrides.can_inject_context {
        capabilities.can_inject_context = value;
    }
    if let Some(value) = overrides.can_run_headless {
        capabilities.can_run_headless = value;
    }
    if let Some(value) = overrides.can_resume_session {
        capabilities.can_resume_session = value;
    }
    if let Some(value) = overrides.can_export_session {
        capabilities.can_export_session = value;
    }
    if let Some(value) = overrides.supports_readonly_mode {
        capabilities.supports_readonly_mode = value;
    }
    if let Some(value) = overrides.supports_workspace_write_mode {
        capabilities.supports_workspace_write_mode = value;
    }
    if let Some(value) = overrides.requires_external_sandbox {
        capabilities.requires_external_sandbox = value;
    }
    if let Some(value) = overrides.runtime_auth.clone() {
        capabilities.runtime_auth = if runtime_auth_config_is_safe_for_capabilities(agent, &value) {
            Some(value)
        } else {
            None
        };
    }
}

impl FromStr for AgentKind {
    type Err = ParseAgentKindError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "codex" => Ok(Self::Codex),
            "claude-code" | "claude" => Ok(Self::ClaudeCode),
            "pi" => Ok(Self::Pi),
            "opencode" | "open-code" => Ok(Self::OpenCode),
            _ => Err(ParseAgentKindError {
                value: value.into(),
            }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("unsupported agent kind: {value}")]
pub struct ParseAgentKindError {
    value: String,
}
