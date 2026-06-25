//! Pre-tool hook response handling for the `agent-monitor` binary.
//!
//! Decodes an adapter hook payload, applies workspace security policy and any
//! active read-only judge control packet, persists the attempt/decision events,
//! and renders the decision in the adapter's native hook format.

use coding_agent_monitor::{
    AdapterHookDecision, AdapterHookResponse, AgentKind, ControlPacket, Event, EventKind,
    ProjectConfig, ProjectStore, adapter_capabilities_for_config, adapter_hook_response,
    agent_kind_label, normalize_adapter_event,
};
use std::io::Write;
use std::path::Path;
use std::process::Command;

/// Native hook-response wire format requested on the `hook-response` command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HookResponseFormat {
    Generic,
    Codex,
    ClaudeCode,
    OpenCode,
}

pub(crate) fn run_hook_response(
    input: impl std::io::Read,
    mut output: impl Write,
    adapter: AgentKind,
    workspace: &Path,
    session: Option<&str>,
    format: HookResponseFormat,
) -> Result<(), String> {
    // Hook handlers are synchronous policy gates. Keep the sequence explicit:
    // parse payload, validate adapter capability, apply deterministic security,
    // overlay active monitor packets, persist evidence, then render the native
    // adapter response.
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
    // Static security policy wins first. Control packets only tighten behavior,
    // and only for tool calls that would mutate the worktree.
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
) -> Result<Option<ControlPacket>, String> {
    // Packet preconditions are replay protection: a stale packet for another
    // session, worktree, or git head must not affect this hook call.
    let path = workspace.join(".agent-monitor").join("packets.jsonl");
    let contents = match std::fs::read_to_string(&path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("read control packet log: {error}")),
    };

    let mut latest = None;
    for line in contents.lines().filter(|line| !line.trim().is_empty()) {
        let packet: ControlPacket = serde_json::from_str(line)
            .map_err(|error| format!("decode control packet from {}: {error}", path.display()))?;
        if control_packet_matches_hook_target(&packet, adapter, workspace, session) {
            latest = Some(packet);
        }
    }
    Ok(latest)
}

fn control_packet_matches_hook_target(
    packet: &ControlPacket,
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

fn control_packet_requires_read_only_judge(packet: &ControlPacket) -> bool {
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

fn read_only_judge_hook_block_reason(packet: &ControlPacket, raw: &serde_json::Value) -> String {
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
    // Persist both the normalized adapter attempt and any monitor decision so
    // later case files can explain why a tool call was blocked or escalated.
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

#[cfg(test)]
#[path = "hook_response_tests.rs"]
mod tests;
