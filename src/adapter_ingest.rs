use crate::{
    AgentKind, Event, EventKind, SecurityConfig, agent_kind_label,
    destructive_command_user_decision_cause, fnv1a64_digest, safe_slug,
    security_path_user_decision_cause,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AdapterHookDecision {
    Allow,
    Ask,
    Block,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdapterHookResponse {
    pub decision: AdapterHookDecision,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
}

pub fn adapter_hook_response(
    _adapter: AgentKind,
    default_session: Option<&str>,
    raw: &serde_json::Value,
    security: &SecurityConfig,
) -> Option<AdapterHookResponse> {
    let event_name = adapter_event_name(raw)?;
    if !adapter_pre_tool_event_name(&event_name) {
        return None;
    }

    let (decision, reason) = adapter_hook_policy_decision(raw, security);
    Some(AdapterHookResponse {
        decision,
        reason,
        session: adapter_session(raw).or_else(|| default_session.map(str::to_string)),
    })
}

pub fn normalize_adapter_event(
    adapter: AgentKind,
    default_session: Option<&str>,
    raw: &serde_json::Value,
) -> Option<Event> {
    if raw.get("kind").is_some()
        && let Ok(mut event) = serde_json::from_value::<Event>(raw.clone())
    {
        if event.agent.trim().is_empty() {
            event.agent = agent_kind_label(adapter).into();
        }
        if event.session.as_deref().is_none_or(str::is_empty) {
            event.session = default_session.map(str::to_string);
        }
        apply_adapter_provenance(&mut event, adapter, raw);
        return Some(event);
    }

    let event_name = adapter_event_name(raw);
    let mut event = adapter_base_event(adapter, default_session, raw);

    match event_name.as_deref() {
        Some("session.started")
        | Some("session_start")
        | Some("SessionStart")
        | Some("pi.wrapper.session_started")
        | Some("pi.wrapper.session.started") => {
            event.kind = EventKind::AgentHealth;
            event.content = Some("session started".into());
            Some(event)
        }
        Some("thread.started") | Some("thread_start") => {
            event.kind = EventKind::AgentHealth;
            event.content = Some("thread started".into());
            Some(event)
        }
        Some("turn.started") | Some("turn_start") => {
            event.kind = EventKind::AgentHealth;
            event.content = Some("turn started".into());
            Some(event)
        }
        Some("turn.completed") | Some("turn_completed") => {
            event.kind = EventKind::AgentHealth;
            event.content = Some("turn completed".into());
            Some(event)
        }
        Some("turn.failed") | Some("turn_failure") => {
            event.kind = EventKind::AgentHealth;
            event.content = Some(adapter_lifecycle_error_content(raw));
            Some(event)
        }
        Some("item.started") | Some("item_started") => adapter_item_event(event, raw, true),
        Some("item.completed") | Some("item_completed") => adapter_item_event(event, raw, false),
        Some("session.idle")
        | Some("session_idle")
        | Some("SessionIdle")
        | Some("pi.wrapper.session_idle")
        | Some("pi.wrapper.session.idle") => {
            event.kind = EventKind::AgentHealth;
            event.content = Some("session idle".into());
            Some(event)
        }
        Some("session.stopped")
        | Some("session.stop")
        | Some("session_end")
        | Some("session.finished")
        | Some("Stop")
        | Some("pi.wrapper.session_stopped")
        | Some("pi.wrapper.session.stopped") => {
            event.kind = EventKind::AgentHealth;
            event.content = Some("session stopped".into());
            Some(event)
        }
        Some("SubagentStop") | Some("subagent_stop") => {
            event.kind = EventKind::AgentHealth;
            event.content = Some("subagent stopped".into());
            Some(event)
        }
        Some("session.error")
        | Some("session.failed")
        | Some("session_error")
        | Some("pi.wrapper.session_error")
        | Some("pi.wrapper.session.error") => {
            event.kind = EventKind::AgentHealth;
            event.content = Some(adapter_lifecycle_error_content(raw));
            Some(event)
        }
        Some("permission.denied") | Some("permission_denied") => {
            event.kind = EventKind::InterventionResult;
            event.command = adapter_command(raw);
            event.content = Some(adapter_permission_denial_content(raw));
            Some(event)
        }
        Some("permission.requested") | Some("permission_requested") => {
            event.kind = EventKind::InterventionResult;
            event.command = adapter_command(raw);
            event.content = Some(adapter_permission_request_content(raw));
            Some(event)
        }
        Some("PreToolUse") | Some("pre_tool_use") => {
            event.command = adapter_command(raw);
            if adapter_hook_decision_denies(raw) {
                event.kind = EventKind::InterventionResult;
                event.content = Some(adapter_permission_denial_content(raw));
            } else if adapter_hook_decision_requests_permission(raw)
                || adapter_command_requires_user_authorization(&event)
            {
                event.kind = EventKind::InterventionResult;
                event.content = Some(adapter_permission_request_content(raw));
            } else {
                event.kind = EventKind::ToolCall;
                event.content = event
                    .command
                    .as_ref()
                    .map(|command| format!("tool command: {command}"))
                    .or_else(|| adapter_text(raw));
            }
            Some(event)
        }
        Some("PostToolUse") | Some("post_tool_use") => {
            if let Some(event) = adapter_successful_file_tool_change_event(event.clone(), raw) {
                return Some(event);
            }
            event.command = adapter_command(raw);
            event.kind = if event.command.is_some() {
                EventKind::CommandResult
            } else {
                EventKind::ToolResult
            };
            event.exit_code = adapter_exit_code(raw);
            event.content = adapter_output(raw)
                .or_else(|| adapter_error_text(raw))
                .or_else(|| adapter_text(raw));
            Some(event)
        }
        Some("UserPromptSubmit") | Some("user_prompt_submit") => {
            event.kind = EventKind::UserInstruction;
            event.content = Some(adapter_user_prompt_text(raw)?);
            Some(event)
        }
        Some("PreCompact") | Some("pre_compact") => {
            event.kind = EventKind::AgentHealth;
            event.content = Some(adapter_context_compaction_content(raw));
            Some(event)
        }
        Some("Notification") | Some("notification")
            if adapter_notification_requests_permission(raw) =>
        {
            event.kind = EventKind::InterventionResult;
            event.command = adapter_command(raw);
            event.content = Some(adapter_permission_request_content(raw));
            Some(event)
        }
        Some("Notification") | Some("notification") => {
            event.kind = EventKind::AgentHealth;
            event.content = Some(adapter_notification_content(raw));
            Some(event)
        }
        Some("session.compacted") => {
            event.kind = EventKind::AgentHealth;
            event.content = Some("context compaction completed".into());
            Some(event)
        }
        Some("experimental.session.compacting") => {
            event.kind = EventKind::AgentHealth;
            event.content = Some(adapter_context_compaction_content(raw));
            Some(event)
        }
        Some("StopFailure") | Some("stop_failure") => {
            event.kind = EventKind::AgentHealth;
            event.content = Some(adapter_lifecycle_error_content(raw));
            Some(event)
        }
        Some("PostToolUseFailure") | Some("post_tool_use_failure") => {
            event.command = adapter_command(raw);
            event.exit_code = adapter_exit_code(raw).or(Some(1));
            event.content = adapter_error_text(raw)
                .or_else(|| adapter_output(raw))
                .or_else(|| adapter_text(raw));
            event.kind = if event.command.is_some() {
                EventKind::CommandResult
            } else {
                EventKind::ToolResult
            };
            Some(event)
        }
        Some("test_result") | Some("verification.completed") | Some("verification.failed") => {
            event.kind = EventKind::TestResult;
            event.command = adapter_command(raw);
            event.exit_code = adapter_exit_code(raw);
            event.content = adapter_output(raw).or_else(|| adapter_text(raw));
            Some(event)
        }
        Some("tool.execute.after")
        | Some("tool_result")
        | Some("tool_call.completed")
        | Some("command_result")
        | Some("pi.wrapper.command_result")
        | Some("pi.wrapper.command.completed") => {
            if let Some(event) = adapter_successful_file_tool_change_event(event.clone(), raw) {
                return Some(event);
            }
            event.kind = EventKind::CommandResult;
            event.command = adapter_command(raw);
            event.exit_code = adapter_exit_code(raw);
            event.content = adapter_output(raw)
                .or_else(|| adapter_error_text(raw))
                .or_else(|| adapter_text(raw));
            if event.command.is_none() && !matches!(event_name.as_deref(), Some("command_result")) {
                event.kind = EventKind::ToolResult;
            }
            Some(event)
        }
        Some("tool.execute.before")
        | Some("tool_call")
        | Some("tool_use")
        | Some("pi.wrapper.command_started")
        | Some("pi.wrapper.command.started") => {
            event.kind = EventKind::ToolCall;
            event.command = adapter_command(raw);
            event.content = event
                .command
                .as_ref()
                .map(|command| format!("tool command: {command}"))
                .or_else(|| adapter_text(raw));
            Some(event)
        }
        Some("file_change") | Some("file.written") => {
            event.kind = EventKind::FileChange;
            event.file = adapter_path_field(raw);
            event.line = adapter_line_field(raw, "line");
            event.line_end = adapter_line_field(raw, "line_end")
                .or_else(|| adapter_line_field(raw, "end_line"))
                .or_else(|| adapter_line_field(raw, "lineEnd"));
            event.rationale = adapter_string_field(raw, "rationale");
            event.content = adapter_text(raw);
            Some(event)
        }
        Some("diff.changed") => {
            event.kind = EventKind::RepoDiff;
            event.file = adapter_path_field(raw);
            event.line = adapter_line_field(raw, "line");
            event.line_end = adapter_line_field(raw, "line_end")
                .or_else(|| adapter_line_field(raw, "end_line"))
                .or_else(|| adapter_line_field(raw, "lineEnd"));
            event.rationale = adapter_string_field(raw, "rationale");
            event.content = adapter_text(raw);
            Some(event)
        }
        Some("design_thought") | Some("memory.candidate") => {
            event.kind = EventKind::DesignThought;
            event.content = Some(adapter_text(raw)?);
            Some(event)
        }
        Some("content_block_delta")
        | Some("message.delta")
        | Some("message_delta")
        | Some("assistant.delta")
        | Some("assistant_delta")
        | Some("agent_message_delta")
        | Some("response.output_text.delta")
        | Some("output_text.delta")
        | Some("item.delta") => adapter_message_delta_event(event, raw),
        Some("assistant")
        | Some("assistant_message")
        | Some("agent_message")
        | Some("message")
        | Some("model_message") => {
            event.kind = EventKind::ModelMessage;
            event.content = Some(adapter_text(raw)?);
            Some(event)
        }
        Some(_) => None,
        None => {
            event.content = Some(adapter_text(raw)?);
            event.kind = EventKind::ModelMessage;
            Some(event)
        }
    }
}

pub(crate) fn normalize_adapter_events(
    adapter: AgentKind,
    default_session: Option<&str>,
    raw: &serde_json::Value,
) -> Vec<Event> {
    if raw.get("kind").is_some()
        && let Ok(mut event) = serde_json::from_value::<Event>(raw.clone())
    {
        if event.agent.trim().is_empty() {
            event.agent = agent_kind_label(adapter).into();
        }
        if event.session.as_deref().is_none_or(str::is_empty) {
            event.session = default_session.map(str::to_string);
        }
        apply_adapter_provenance(&mut event, adapter, raw);
        return vec![event];
    }

    let event_name = adapter_event_name(raw);
    if adapter_successful_file_tool_result_event_name(event_name.as_deref()) {
        let event = adapter_base_event(adapter, default_session, raw);
        let events = adapter_successful_file_tool_change_events(event, raw);
        if !events.is_empty() {
            return events;
        }
    }

    normalize_adapter_event(adapter, default_session, raw)
        .into_iter()
        .collect()
}

fn adapter_successful_file_tool_result_event_name(event_name: Option<&str>) -> bool {
    matches!(
        event_name,
        Some(
            "PostToolUse"
                | "post_tool_use"
                | "tool.execute.after"
                | "tool_result"
                | "tool_call.completed"
                | "command_result"
                | "pi.wrapper.command_result"
                | "pi.wrapper.command.completed"
        )
    )
}

fn adapter_pre_tool_event_name(event_name: &str) -> bool {
    matches!(
        event_name,
        "PreToolUse"
            | "pre_tool_use"
            | "tool.execute.before"
            | "tool_call"
            | "tool_use"
            | "pi.wrapper.command_started"
            | "pi.wrapper.command.started"
    )
}

fn adapter_hook_policy_decision(
    raw: &serde_json::Value,
    security: &SecurityConfig,
) -> (AdapterHookDecision, Option<String>) {
    if adapter_hook_decision_denies(raw) {
        return (
            AdapterHookDecision::Block,
            Some(adapter_permission_denial_content(raw)),
        );
    }

    if adapter_hook_decision_requests_permission(raw) {
        return (
            AdapterHookDecision::Ask,
            Some(adapter_permission_request_content(raw)),
        );
    }

    if let Some(command) = adapter_command(raw)
        && let Some(cause) = destructive_command_user_decision_cause(&command)
    {
        return (
            AdapterHookDecision::Block,
            Some(format!(
                "blocked destructive command before tool execution: {command} ({cause})"
            )),
        );
    }

    for path in adapter_mutating_file_tool_paths(raw) {
        if let Some(cause) = security_path_user_decision_cause(&path, security) {
            let class = if cause.contains("protected path") {
                "protected path"
            } else {
                "security path"
            };
            return (
                AdapterHookDecision::Block,
                Some(format!(
                    "blocked {class} before tool execution: {path} ({cause})"
                )),
            );
        }
    }

    (AdapterHookDecision::Allow, None)
}

pub(crate) fn adapter_event_name(raw: &serde_json::Value) -> Option<String> {
    string_field(raw, "event")
        .or_else(|| string_field(raw, "type"))
        .or_else(|| string_field(raw, "kind"))
        .or_else(|| string_field(raw, "hook_event_name"))
        .or_else(|| raw.pointer("/event/type").and_then(json_string))
}

fn adapter_lifecycle_error_content(raw: &serde_json::Value) -> String {
    let detail = adapter_lifecycle_error_detail(raw);
    let class = adapter_lifecycle_error_class(raw, detail.as_deref());
    detail
        .map(|detail| format!("session error [{class}]: {detail}"))
        .unwrap_or_else(|| format!("session error [{class}]"))
}

fn adapter_lifecycle_error_detail(raw: &serde_json::Value) -> Option<String> {
    adapter_output(raw)
        .or_else(|| adapter_text(raw))
        .or_else(|| string_field(raw, "error"))
        .or_else(|| string_field(raw, "reason"))
        .or_else(|| raw.pointer("/error/message").and_then(json_string))
        .or_else(|| raw.pointer("/error/code").and_then(json_string))
        .map(|detail| detail.trim().to_string())
        .filter(|detail| !detail.is_empty())
}

fn adapter_lifecycle_error_class(raw: &serde_json::Value, detail: Option<&str>) -> &'static str {
    let mut signals = Vec::new();
    if let Some(detail) = detail {
        signals.push(detail.to_string());
    }
    signals.extend(
        [
            "error_type",
            "errorType",
            "code",
            "category",
            "status",
            "reason",
        ]
        .into_iter()
        .filter_map(|field| string_field(raw, field)),
    );
    signals.extend(
        [
            "/error/type",
            "/error/code",
            "/error/name",
            "/error/kind",
            "/error/category",
            "/error/status",
        ]
        .into_iter()
        .filter_map(|pointer| raw.pointer(pointer).and_then(json_string)),
    );
    let signal = signals.join(" ").to_lowercase();

    if [
        "context length",
        "context_length",
        "context limit",
        "context_limit",
        "context window",
        "max tokens",
        "maximum context",
        "prompt too long",
        "token limit",
    ]
    .iter()
    .any(|needle| signal.contains(needle))
    {
        return "context_limit";
    }

    if [
        "rate limit",
        "rate_limit",
        "429",
        "too many requests",
        "quota exceeded",
    ]
    .iter()
    .any(|needle| signal.contains(needle))
    {
        return "rate_limit";
    }

    if [
        "service unavailable",
        "provider unavailable",
        "unavailable",
        "upstream",
        "overloaded",
        "502",
        "503",
        "504",
    ]
    .iter()
    .any(|needle| signal.contains(needle))
    {
        return "provider_unavailable";
    }

    if [
        "tool failed",
        "tool_failure",
        "tool error",
        "tool_error",
        "hook failed",
        "malformed tool",
    ]
    .iter()
    .any(|needle| signal.contains(needle))
    {
        return "tool_failure";
    }

    if [
        "process crash",
        "process exited",
        "crashed",
        "panic",
        "signal",
        "killed",
    ]
    .iter()
    .any(|needle| signal.contains(needle))
    {
        return "process_crash";
    }

    "unknown"
}

fn adapter_permission_denial_content(raw: &serde_json::Value) -> String {
    string_field(raw, "reason")
        .or_else(|| adapter_text(raw))
        .or_else(|| adapter_output(raw))
        .map(|reason| format!("permission denied: {reason}"))
        .unwrap_or_else(|| "permission denied".into())
}

fn adapter_permission_request_content(raw: &serde_json::Value) -> String {
    string_field(raw, "reason")
        .or_else(|| adapter_text(raw))
        .or_else(|| adapter_output(raw))
        .map(|reason| format!("permission requested: {reason}"))
        .or_else(|| {
            adapter_command(raw)
                .map(|command| format!("permission requested for command: {command}"))
        })
        .unwrap_or_else(|| "permission requested".into())
}

fn adapter_command_requires_user_authorization(event: &Event) -> bool {
    event
        .command
        .as_deref()
        .and_then(destructive_command_user_decision_cause)
        .is_some()
}

fn adapter_hook_decision_denies(raw: &serde_json::Value) -> bool {
    adapter_hook_decision_text(raw)
        .map(|decision| {
            let decision = decision.to_lowercase();
            decision.contains("deny")
                || decision.contains("denied")
                || decision.contains("block")
                || decision.contains("reject")
        })
        .unwrap_or(false)
}

fn adapter_hook_decision_requests_permission(raw: &serde_json::Value) -> bool {
    adapter_hook_decision_text(raw)
        .map(|decision| {
            let decision = decision.to_lowercase();
            decision.contains("ask")
                || decision.contains("request")
                || decision.contains("pending")
                || decision.contains("defer")
        })
        .unwrap_or(false)
}

fn adapter_hook_decision_text(raw: &serde_json::Value) -> Option<String> {
    string_field(raw, "decision")
        .or_else(|| string_field(raw, "status"))
        .or_else(|| string_field(raw, "action"))
        .or_else(|| string_field(raw, "permission"))
        .or_else(|| string_field(raw, "permission_decision"))
        .or_else(|| string_field(raw, "permissionDecision"))
        .or_else(|| string_field(raw, "permission_status"))
        .or_else(|| string_field(raw, "permissionStatus"))
        .or_else(|| raw.pointer("/permission/decision").and_then(json_string))
        .or_else(|| raw.pointer("/permission/status").and_then(json_string))
        .or_else(|| raw.pointer("/decision/action").and_then(json_string))
}

fn adapter_notification_requests_permission(raw: &serde_json::Value) -> bool {
    adapter_text(raw)
        .map(|text| {
            let text = text.to_lowercase();
            text.contains("permission")
                || text.contains("approval")
                || text.contains("approve")
                || text.contains("allow")
        })
        .unwrap_or(false)
}

fn adapter_notification_content(raw: &serde_json::Value) -> String {
    adapter_text(raw)
        .map(|message| format!("notification: {}", message.trim()))
        .unwrap_or_else(|| "notification".into())
}

pub(crate) fn adapter_ignored_event(
    adapter: AgentKind,
    session: Option<&str>,
    line_number: usize,
    event_name: &str,
) -> Event {
    let id = if line_number > 0 {
        format!("adapter-ingest-ignored-line-{line_number}")
    } else {
        format!("adapter-ingest-ignored-{}", safe_slug(event_name))
    };
    Event {
        event_id: Some(id),
        agent: agent_kind_label(adapter).into(),
        session: session.map(str::to_string),
        kind: EventKind::CommandOutput,
        content: Some(format!(
            "adapter ingest ignored unsupported event type `{}`",
            safe_adapter_event_label(event_name)
        )),
        ..Event::default()
    }
}

fn safe_adapter_event_label(event_name: &str) -> String {
    let trimmed = event_name.trim();
    if !trimmed.is_empty()
        && trimmed.len() <= 80
        && trimmed
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-' | ':' | '/'))
    {
        trimmed.to_string()
    } else {
        format!(
            "hash-{}",
            fnv1a64_digest(trimmed.as_bytes()).replace(':', "-")
        )
    }
}

pub(crate) fn adapter_ingest_warning_event(
    adapter: AgentKind,
    session: Option<&str>,
    line_number: usize,
) -> Event {
    Event {
        event_id: Some(format!("adapter-ingest-warning-line-{line_number}")),
        agent: agent_kind_label(adapter).into(),
        session: session.map(str::to_string),
        kind: EventKind::CommandOutput,
        content: Some(format!(
            "adapter ingest skipped malformed JSONL line {line_number}"
        )),
        ..Event::default()
    }
}

fn adapter_base_event(
    adapter: AgentKind,
    default_session: Option<&str>,
    raw: &serde_json::Value,
) -> Event {
    let session = adapter_session(raw).or_else(|| default_session.map(str::to_string));
    let time = adapter_string_field(raw, "time")
        .or_else(|| adapter_string_field(raw, "timestamp"))
        .or_else(|| adapter_string_field(raw, "created_at"))
        .or_else(|| raw.pointer("/message/created_at").and_then(json_string));
    Event {
        time: time.clone(),
        event_id: adapter_string_field(raw, "event_id")
            .or_else(|| adapter_string_field(raw, "id"))
            .or_else(|| raw.pointer("/item/id").and_then(json_string))
            .or_else(|| raw.pointer("/event/item/id").and_then(json_string)),
        project_id: adapter_string_field(raw, "project_id")
            .or_else(|| raw.pointer("/project/id").and_then(json_string)),
        run_id: adapter_string_field(raw, "run_id")
            .or_else(|| raw.pointer("/run/id").and_then(json_string)),
        workspace: adapter_string_field(raw, "workspace")
            .or_else(|| raw.pointer("/project/root").and_then(json_string)),
        adapter: Some(agent_kind_label(adapter).into()),
        adapter_version: adapter_string_field(raw, "adapter_version")
            .or_else(|| raw.pointer("/adapter/version").and_then(json_string)),
        seq: adapter_u64_field(raw, "seq").or_else(|| adapter_u64_field(raw, "sequence")),
        observed_at: adapter_string_field(raw, "observed_at"),
        occurred_at: adapter_string_field(raw, "occurred_at").or(time),
        cwd: adapter_string_field(raw, "cwd"),
        worktree: adapter_string_field(raw, "worktree"),
        git_head: adapter_nested_string_field(raw, "git", "head"),
        git_branch: adapter_nested_string_field(raw, "git", "branch"),
        git_dirty: adapter_nested_bool_field(raw, "git", "dirty"),
        agent: adapter_string_field(raw, "agent")
            .or_else(|| adapter_string_field(raw, "actor"))
            .or_else(|| raw.pointer("/actor/name").and_then(json_string))
            .unwrap_or_else(|| agent_kind_label(adapter).into()),
        actor: adapter_string_field(raw, "actor").or_else(|| Some("agent".into())),
        provider: adapter_string_field(raw, "provider"),
        model: adapter_string_field(raw, "model")
            .or_else(|| raw.pointer("/message/model").and_then(json_string)),
        agent_session_id: string_field(raw, "agent_session_id").or_else(|| session.clone()),
        session,
        kind: EventKind::ModelMessage,
        content: None,
        file: None,
        line: None,
        line_end: None,
        rationale: None,
        stream: adapter_string_field(raw, "stream"),
        command: None,
        exit_code: None,
        source_type: adapter_nested_string_field(raw, "source", "type"),
        source_path: adapter_nested_string_field(raw, "source", "path"),
        source_offset: adapter_nested_u64_field(raw, "source", "offset"),
        source_hash: adapter_nested_string_field(raw, "source", "hash"),
        redaction_status: adapter_nested_string_field(raw, "redaction", "status"),
        redaction_rules: adapter_nested_string_array_field(raw, "redaction", "rules"),
        related_event_ids: Vec::new(),
        requirement_ids: Vec::new(),
    }
}

fn apply_adapter_provenance(event: &mut Event, adapter: AgentKind, raw: &serde_json::Value) {
    if event.adapter.as_deref().is_none_or(str::is_empty) {
        event.adapter = Some(agent_kind_label(adapter).into());
    }
    if event.project_id.as_deref().is_none_or(str::is_empty) {
        event.project_id = adapter_string_field(raw, "project_id")
            .or_else(|| raw.pointer("/project/id").and_then(json_string));
    }
    if event.run_id.as_deref().is_none_or(str::is_empty) {
        event.run_id = adapter_string_field(raw, "run_id")
            .or_else(|| raw.pointer("/run/id").and_then(json_string));
    }
    if event.agent_session_id.as_deref().is_none_or(str::is_empty) {
        event.agent_session_id = event.session.clone();
    }
    if event.adapter_version.as_deref().is_none_or(str::is_empty) {
        event.adapter_version = adapter_string_field(raw, "adapter_version")
            .or_else(|| raw.pointer("/adapter/version").and_then(json_string));
    }
    if event.seq.is_none() {
        event.seq = adapter_u64_field(raw, "seq").or_else(|| adapter_u64_field(raw, "sequence"));
    }
    if event.observed_at.as_deref().is_none_or(str::is_empty) {
        event.observed_at = adapter_string_field(raw, "observed_at");
    }
    if event.occurred_at.as_deref().is_none_or(str::is_empty) {
        event.occurred_at = adapter_string_field(raw, "occurred_at")
            .or_else(|| event.time.clone())
            .or_else(|| adapter_string_field(raw, "timestamp"));
    }
    if event.cwd.as_deref().is_none_or(str::is_empty) {
        event.cwd = adapter_string_field(raw, "cwd");
    }
    if event.worktree.as_deref().is_none_or(str::is_empty) {
        event.worktree = adapter_string_field(raw, "worktree");
    }
    if event.git_head.as_deref().is_none_or(str::is_empty) {
        event.git_head = adapter_nested_string_field(raw, "git", "head");
    }
    if event.git_branch.as_deref().is_none_or(str::is_empty) {
        event.git_branch = adapter_nested_string_field(raw, "git", "branch");
    }
    if event.git_dirty.is_none() {
        event.git_dirty = adapter_nested_bool_field(raw, "git", "dirty");
    }
    if event.actor.as_deref().is_none_or(str::is_empty) {
        event.actor = adapter_string_field(raw, "actor").or_else(|| Some("agent".into()));
    }
    if event.source_type.as_deref().is_none_or(str::is_empty) {
        event.source_type = adapter_nested_string_field(raw, "source", "type");
    }
    if event.source_path.as_deref().is_none_or(str::is_empty) {
        event.source_path = adapter_nested_string_field(raw, "source", "path");
    }
    if event.source_offset.is_none() {
        event.source_offset = adapter_nested_u64_field(raw, "source", "offset");
    }
    if event.source_hash.as_deref().is_none_or(str::is_empty) {
        event.source_hash = adapter_nested_string_field(raw, "source", "hash");
    }
    if event.redaction_status.as_deref().is_none_or(str::is_empty) {
        event.redaction_status = adapter_nested_string_field(raw, "redaction", "status");
    }
    if event.redaction_rules.is_empty() {
        event.redaction_rules = adapter_nested_string_array_field(raw, "redaction", "rules");
    }
}

fn adapter_session(raw: &serde_json::Value) -> Option<String> {
    string_field(raw, "session")
        .or_else(|| string_field(raw, "session_id"))
        .or_else(|| string_field(raw, "thread_id"))
        .or_else(|| string_field(raw, "conversation_id"))
        .or_else(|| raw.pointer("/session/id").and_then(json_string))
        .or_else(|| raw.pointer("/session/session_id").and_then(json_string))
        .or_else(|| raw.pointer("/thread/id").and_then(json_string))
        .or_else(|| raw.pointer("/event/session/id").and_then(json_string))
        .or_else(|| raw.pointer("/event/session_id").and_then(json_string))
        .or_else(|| raw.pointer("/event/sessionID").and_then(json_string))
        .or_else(|| raw.pointer("/event/thread_id").and_then(json_string))
        .or_else(|| {
            raw.pointer("/event/properties/session/id")
                .and_then(json_string)
        })
        .or_else(|| {
            raw.pointer("/event/properties/session_id")
                .and_then(json_string)
        })
        .or_else(|| {
            raw.pointer("/event/properties/sessionID")
                .and_then(json_string)
        })
        .or_else(|| {
            raw.pointer("/event/properties/thread_id")
                .and_then(json_string)
        })
}

fn adapter_text(raw: &serde_json::Value) -> Option<String> {
    string_field(raw, "content")
        .or_else(|| string_field(raw, "message"))
        .or_else(|| string_field(raw, "text"))
        .or_else(|| string_field(raw, "summary"))
        .or_else(|| string_field(raw, "delta"))
        .or_else(|| raw.pointer("/item/content").and_then(json_text))
        .or_else(|| raw.pointer("/item/message").and_then(json_text))
        .or_else(|| raw.pointer("/item/text").and_then(json_string))
        .or_else(|| raw.pointer("/event/item/content").and_then(json_text))
        .or_else(|| raw.pointer("/event/item/message").and_then(json_text))
        .or_else(|| raw.pointer("/event/item/text").and_then(json_string))
        .or_else(|| raw.pointer("/message/content").and_then(json_text))
        .or_else(|| raw.pointer("/delta/content").and_then(json_text))
        .or_else(|| raw.pointer("/delta/message").and_then(json_text))
        .or_else(|| raw.pointer("/delta/text").and_then(json_string))
        .or_else(|| raw.pointer("/event/delta/content").and_then(json_text))
        .or_else(|| raw.pointer("/event/delta/message").and_then(json_text))
        .or_else(|| raw.pointer("/event/delta/text").and_then(json_string))
        .or_else(|| {
            raw.pointer("/event/properties/delta/content")
                .and_then(json_text)
        })
        .or_else(|| {
            raw.pointer("/event/properties/delta/message")
                .and_then(json_text)
        })
        .or_else(|| {
            raw.pointer("/event/properties/delta/text")
                .and_then(json_string)
        })
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

fn adapter_message_delta_event(mut event: Event, raw: &serde_json::Value) -> Option<Event> {
    let text = adapter_text(raw)?;
    event.kind = EventKind::CommandOutput;
    event.content = Some(format!("message delta: {text}"));
    Some(event)
}

fn adapter_user_prompt_text(raw: &serde_json::Value) -> Option<String> {
    string_field(raw, "prompt")
        .or_else(|| raw.pointer("/payload/prompt").and_then(json_string))
        .or_else(|| adapter_text(raw))
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

fn adapter_context_compaction_content(raw: &serde_json::Value) -> String {
    let trigger = string_field(raw, "trigger")
        .or_else(|| raw.pointer("/payload/trigger").and_then(json_string))
        .or_else(|| string_field(raw, "reason"));
    trigger
        .map(|trigger| format!("context compaction requested: {}", trigger.trim()))
        .unwrap_or_else(|| "context compaction requested".into())
}

fn adapter_output(raw: &serde_json::Value) -> Option<String> {
    string_field(raw, "output")
        .or_else(|| string_field(raw, "stdout"))
        .or_else(|| string_field(raw, "stderr"))
        .or_else(|| raw.pointer("/item/output").and_then(json_string))
        .or_else(|| raw.pointer("/item/stdout").and_then(json_string))
        .or_else(|| raw.pointer("/item/stderr").and_then(json_string))
        .or_else(|| raw.pointer("/event/item/output").and_then(json_string))
        .or_else(|| raw.pointer("/event/item/stdout").and_then(json_string))
        .or_else(|| raw.pointer("/event/item/stderr").and_then(json_string))
        .or_else(|| raw.pointer("/result/output").and_then(json_string))
        .or_else(|| raw.pointer("/result/stdout").and_then(json_string))
        .or_else(|| raw.pointer("/result/stderr").and_then(json_string))
        .or_else(|| raw.pointer("/tool_result/output").and_then(json_string))
        .or_else(|| raw.pointer("/tool_result/stdout").and_then(json_string))
        .or_else(|| raw.pointer("/tool_result/stderr").and_then(json_string))
        .or_else(|| raw.pointer("/tool_response/output").and_then(json_string))
        .or_else(|| raw.pointer("/tool_response/stdout").and_then(json_string))
        .or_else(|| raw.pointer("/tool_response/stderr").and_then(json_string))
        .or_else(|| raw.pointer("/response/output").and_then(json_string))
        .or_else(|| raw.pointer("/response/stdout").and_then(json_string))
        .or_else(|| raw.pointer("/response/stderr").and_then(json_string))
        .or_else(|| {
            raw.pointer("/event/properties/output")
                .and_then(json_string)
        })
        .or_else(|| {
            raw.pointer("/event/properties/stdout")
                .and_then(json_string)
        })
        .or_else(|| {
            raw.pointer("/event/properties/stderr")
                .and_then(json_string)
        })
        .or_else(|| {
            raw.pointer("/event/properties/result/output")
                .and_then(json_string)
        })
        .or_else(|| {
            raw.pointer("/event/properties/result/stdout")
                .and_then(json_string)
        })
        .or_else(|| {
            raw.pointer("/event/properties/result/stderr")
                .and_then(json_string)
        })
        .or_else(|| {
            raw.pointer("/event/properties/tool_result/output")
                .and_then(json_string)
        })
        .or_else(|| {
            raw.pointer("/event/properties/tool_result/stdout")
                .and_then(json_string)
        })
        .or_else(|| {
            raw.pointer("/event/properties/tool_result/stderr")
                .and_then(json_string)
        })
        .or_else(|| {
            raw.pointer("/event/properties/tool_response/output")
                .and_then(json_string)
        })
        .or_else(|| {
            raw.pointer("/event/properties/tool_response/stdout")
                .and_then(json_string)
        })
        .or_else(|| {
            raw.pointer("/event/properties/tool_response/stderr")
                .and_then(json_string)
        })
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

fn adapter_error_text(raw: &serde_json::Value) -> Option<String> {
    string_field(raw, "error")
        .or_else(|| raw.pointer("/error/message").and_then(json_string))
        .or_else(|| raw.pointer("/item/error").and_then(json_string))
        .or_else(|| raw.pointer("/item/error/message").and_then(json_string))
        .or_else(|| raw.pointer("/event/error/message").and_then(json_string))
        .or_else(|| raw.pointer("/event/item/error").and_then(json_string))
        .or_else(|| {
            raw.pointer("/event/item/error/message")
                .and_then(json_string)
        })
        .or_else(|| raw.pointer("/result/error").and_then(json_string))
        .or_else(|| raw.pointer("/result/error/message").and_then(json_string))
        .or_else(|| raw.pointer("/tool_result/error").and_then(json_string))
        .or_else(|| {
            raw.pointer("/tool_result/error/message")
                .and_then(json_string)
        })
        .or_else(|| raw.pointer("/tool_response/error").and_then(json_string))
        .or_else(|| {
            raw.pointer("/tool_response/error/message")
                .and_then(json_string)
        })
        .or_else(|| raw.pointer("/response/error").and_then(json_string))
        .or_else(|| raw.pointer("/response/error/message").and_then(json_string))
        .or_else(|| raw.pointer("/event/properties/error").and_then(json_string))
        .or_else(|| {
            raw.pointer("/event/properties/error/message")
                .and_then(json_string)
        })
        .or_else(|| {
            raw.pointer("/event/properties/result/error")
                .and_then(json_string)
        })
        .or_else(|| {
            raw.pointer("/event/properties/result/error/message")
                .and_then(json_string)
        })
        .or_else(|| {
            raw.pointer("/event/properties/tool_result/error")
                .and_then(json_string)
        })
        .or_else(|| {
            raw.pointer("/event/properties/tool_result/error/message")
                .and_then(json_string)
        })
        .or_else(|| {
            raw.pointer("/event/properties/tool_response/error")
                .and_then(json_string)
        })
        .or_else(|| {
            raw.pointer("/event/properties/tool_response/error/message")
                .and_then(json_string)
        })
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty())
}

fn adapter_command(raw: &serde_json::Value) -> Option<String> {
    if adapter_tool_name(raw)
        .map(|tool| tool.trim().to_ascii_lowercase().replace(['-', '_'], "") == "applypatch")
        .unwrap_or(false)
        && let Some(path) = adapter_apply_patch_paths(raw).into_iter().next()
    {
        return Some(format!("apply_patch {path}"));
    }

    string_field(raw, "command")
        .or_else(|| raw.pointer("/input/command").and_then(json_string))
        .or_else(|| raw.pointer("/tool_input/command").and_then(json_string))
        .or_else(|| raw.pointer("/payload/command").and_then(json_string))
        .or_else(|| raw.pointer("/item/command").and_then(json_string))
        .or_else(|| raw.pointer("/item/input/command").and_then(json_string))
        .or_else(|| raw.pointer("/event/input/command").and_then(json_string))
        .or_else(|| {
            raw.pointer("/event/tool_input/command")
                .and_then(json_string)
        })
        .or_else(|| raw.pointer("/event/item/command").and_then(json_string))
        .or_else(|| {
            raw.pointer("/event/item/input/command")
                .and_then(json_string)
        })
        .or_else(|| {
            raw.pointer("/event/properties/input/command")
                .and_then(json_string)
        })
        .or_else(|| {
            raw.pointer("/event/properties/tool_input/command")
                .and_then(json_string)
        })
        .or_else(|| {
            raw.pointer("/event/properties/item/command")
                .and_then(json_string)
        })
        .or_else(|| {
            raw.pointer("/event/properties/item/input/command")
                .and_then(json_string)
        })
        .or_else(|| adapter_subagent_tool_label(raw))
        .or_else(|| adapter_tool_target_label(raw))
}

fn adapter_subagent_tool_label(raw: &serde_json::Value) -> Option<String> {
    let tool = adapter_tool_name(raw)?;
    tool.trim()
        .eq_ignore_ascii_case("task")
        .then(|| tool.trim().to_string())
}

fn adapter_successful_file_tool_change_event(
    event: Event,
    raw: &serde_json::Value,
) -> Option<Event> {
    adapter_successful_file_tool_change_events(event, raw)
        .into_iter()
        .next()
}

fn adapter_successful_file_tool_change_events(event: Event, raw: &serde_json::Value) -> Vec<Event> {
    if !adapter_tool_result_succeeded(raw) {
        return Vec::new();
    }
    let files = adapter_mutating_file_tool_paths(raw);
    let total = files.len();
    let source_event_id = event.event_id.clone();
    files
        .into_iter()
        .enumerate()
        .map(|(index, file)| {
            let mut event = event.clone();
            event.kind = EventKind::FileChange;
            event.file = Some(file.clone());
            event.command = adapter_file_tool_change_command(raw, &file);
            event.exit_code = adapter_exit_code(raw);
            event.rationale =
                string_field(raw, "rationale").or_else(|| string_field(raw, "reason"));
            event.event_id =
                adapter_derived_file_change_event_id(source_event_id.as_deref(), total, index);
            if total > 1
                && let Some(source_event_id) = source_event_id.as_deref()
                && !event
                    .related_event_ids
                    .iter()
                    .any(|event_id| event_id == source_event_id)
            {
                event.related_event_ids.push(source_event_id.to_string());
            }
            event.content = adapter_output(raw)
                .or_else(|| adapter_text(raw))
                .or_else(|| {
                    event
                        .command
                        .as_ref()
                        .map(|command| format!("{command} completed"))
                });
            event
        })
        .collect()
}

fn adapter_file_tool_change_command(raw: &serde_json::Value, file: &str) -> Option<String> {
    if adapter_tool_name(raw)
        .map(|tool| tool.trim().to_ascii_lowercase().replace(['-', '_'], "") == "applypatch")
        .unwrap_or(false)
    {
        return Some(format!("apply_patch {file}"));
    }
    adapter_command(raw)
}

fn adapter_derived_file_change_event_id(
    source_event_id: Option<&str>,
    total: usize,
    index: usize,
) -> Option<String> {
    source_event_id.map(|event_id| {
        if total <= 1 {
            event_id.to_string()
        } else {
            format!("{event_id}:file-{}", index + 1)
        }
    })
}

fn adapter_tool_result_succeeded(raw: &serde_json::Value) -> bool {
    if let Some(exit_code) = adapter_exit_code(raw) {
        return exit_code == 0;
    }
    if adapter_error_text(raw).is_some() {
        return false;
    }
    adapter_status(raw)
        .is_none_or(|status| matches!(status.as_str(), "success" | "succeeded" | "ok" | "passed"))
}

fn adapter_mutating_file_tool_paths(raw: &serde_json::Value) -> Vec<String> {
    let Some(tool) = adapter_tool_name(raw) else {
        return Vec::new();
    };
    let tool = tool.trim().to_ascii_lowercase().replace(['-', '_'], "");
    if tool == "applypatch" {
        return adapter_apply_patch_paths(raw);
    }
    let mutating = matches!(
        tool.as_str(),
        "write" | "edit" | "multiedit" | "notebookedit" | "strreplaceeditor"
    );
    if !mutating {
        return Vec::new();
    }
    let Some(input) = adapter_tool_input(raw) else {
        return Vec::new();
    };
    ["file_path", "path", "notebook_path"]
        .into_iter()
        .find_map(|field| input.get(field).and_then(json_string))
        .map(|path| path.trim().to_string())
        .filter(|path| !path.is_empty())
        .into_iter()
        .collect()
}

fn adapter_apply_patch_paths(raw: &serde_json::Value) -> Vec<String> {
    adapter_apply_patch_text(raw)
        .map(|patch| apply_patch_paths(&patch))
        .unwrap_or_default()
}

fn adapter_apply_patch_text(raw: &serde_json::Value) -> Option<String> {
    let input = adapter_tool_input(raw)?;
    if let Some(text) = json_string(input).and_then(non_empty_text) {
        return Some(text);
    }

    ["command", "patch", "input", "content", "body"]
        .into_iter()
        .find_map(|field| {
            input
                .get(field)
                .and_then(json_string)
                .and_then(non_empty_text)
        })
}

fn non_empty_text(text: String) -> Option<String> {
    if text.trim().is_empty() {
        None
    } else {
        Some(text)
    }
}

fn apply_patch_paths(command: &str) -> Vec<String> {
    let mut move_paths = Vec::new();
    let mut other_paths = Vec::new();
    for line in command.lines() {
        if let Some(path) = line.strip_prefix("*** Move to: ") {
            push_unique_patch_path(&mut move_paths, path);
            continue;
        }

        let Some(path) = line
            .strip_prefix("*** Add File: ")
            .or_else(|| line.strip_prefix("*** Update File: "))
            .or_else(|| line.strip_prefix("*** Delete File: "))
        else {
            continue;
        };
        push_unique_patch_path(&mut other_paths, path);
    }

    for path in other_paths {
        if !move_paths.iter().any(|existing| existing == &path) {
            move_paths.push(path);
        }
    }
    move_paths
}

fn push_unique_patch_path(paths: &mut Vec<String>, path: &str) {
    let path = path.trim();
    if !path.is_empty() && !paths.iter().any(|existing| existing == path) {
        paths.push(path.to_string());
    }
}

fn adapter_tool_name(raw: &serde_json::Value) -> Option<String> {
    string_field(raw, "tool_name")
        .or_else(|| string_field(raw, "tool"))
        .or_else(|| raw.pointer("/event/tool_name").and_then(json_string))
        .or_else(|| raw.pointer("/event/tool").and_then(json_string))
        .or_else(|| {
            raw.pointer("/event/properties/tool_name")
                .and_then(json_string)
        })
        .or_else(|| raw.pointer("/event/properties/tool").and_then(json_string))
}

fn adapter_tool_input(raw: &serde_json::Value) -> Option<&serde_json::Value> {
    raw.get("tool_input")
        .or_else(|| raw.get("input"))
        .or_else(|| raw.pointer("/event/tool_input"))
        .or_else(|| raw.pointer("/event/input"))
        .or_else(|| raw.pointer("/item/input"))
        .or_else(|| raw.pointer("/event/item/input"))
        .or_else(|| raw.pointer("/event/properties/tool_input"))
        .or_else(|| raw.pointer("/event/properties/input"))
        .or_else(|| raw.pointer("/event/properties/item/input"))
}

fn adapter_tool_target_label(raw: &serde_json::Value) -> Option<String> {
    let tool = adapter_tool_name(raw)?;
    let input = adapter_tool_input(raw)?;
    let target = [
        "file_path",
        "path",
        "notebook_path",
        "pattern",
        "url",
        "query",
    ]
    .into_iter()
    .find_map(|field| input.get(field).and_then(json_string))
    .map(|target| target.trim().to_string())
    .filter(|target| !target.is_empty())?;

    Some(format!("{} {}", tool.trim(), target))
}

fn adapter_exit_code(raw: &serde_json::Value) -> Option<i32> {
    raw.get("exit_code")
        .or_else(|| raw.get("exitCode"))
        .or_else(|| raw.get("exit"))
        .or_else(|| raw.pointer("/item/exit_code"))
        .or_else(|| raw.pointer("/item/exitCode"))
        .or_else(|| raw.pointer("/event/item/exit_code"))
        .or_else(|| raw.pointer("/event/item/exitCode"))
        .or_else(|| raw.pointer("/result/exit_code"))
        .or_else(|| raw.pointer("/result/exitCode"))
        .or_else(|| raw.pointer("/tool_result/exit_code"))
        .or_else(|| raw.pointer("/tool_result/exitCode"))
        .or_else(|| raw.pointer("/tool_response/exit_code"))
        .or_else(|| raw.pointer("/tool_response/exitCode"))
        .or_else(|| raw.pointer("/response/exit_code"))
        .or_else(|| raw.pointer("/response/exitCode"))
        .or_else(|| raw.pointer("/event/properties/exit_code"))
        .or_else(|| raw.pointer("/event/properties/exitCode"))
        .or_else(|| raw.pointer("/event/properties/exit"))
        .or_else(|| raw.pointer("/event/properties/result/exit_code"))
        .or_else(|| raw.pointer("/event/properties/result/exitCode"))
        .or_else(|| raw.pointer("/event/properties/tool_result/exit_code"))
        .or_else(|| raw.pointer("/event/properties/tool_result/exitCode"))
        .or_else(|| raw.pointer("/event/properties/tool_response/exit_code"))
        .or_else(|| raw.pointer("/event/properties/tool_response/exitCode"))
        .and_then(serde_json::Value::as_i64)
        .and_then(|code| i32::try_from(code).ok())
        .or_else(|| {
            adapter_status(raw).and_then(|status| match status.as_str() {
                "success" | "succeeded" | "ok" | "passed" => Some(0),
                "error" | "failed" | "failure" => Some(1),
                _ => None,
            })
        })
}

fn adapter_item_event(mut event: Event, raw: &serde_json::Value, started: bool) -> Option<Event> {
    let item_type = adapter_item_type(raw)?;
    match item_type.as_str() {
        "agent_message" | "assistant_message" | "message" | "model_message" => {
            event.kind = EventKind::ModelMessage;
            event.content = Some(adapter_text(raw)?);
            Some(event)
        }
        "command_execution" | "exec_command" | "shell_command" | "tool_call" | "tool_use" => {
            event.command = adapter_command(raw);
            if started {
                event.kind = EventKind::ToolCall;
                event.content = event
                    .command
                    .as_ref()
                    .map(|command| format!("tool command: {command}"))
                    .or_else(|| adapter_text(raw));
            } else {
                event.kind = if event.command.is_some() {
                    EventKind::CommandResult
                } else {
                    EventKind::ToolResult
                };
                event.exit_code = adapter_exit_code(raw);
                event.content = adapter_output(raw)
                    .or_else(|| adapter_error_text(raw))
                    .or_else(|| adapter_text(raw))
                    .or_else(|| {
                        event
                            .command
                            .as_ref()
                            .map(|command| format!("tool command completed: {command}"))
                    });
            }
            Some(event)
        }
        "reasoning" | "design_thought" | "memory_candidate" => {
            event.kind = EventKind::DesignThought;
            event.content = Some(adapter_text(raw)?);
            Some(event)
        }
        _ => None,
    }
}

fn adapter_item_type(raw: &serde_json::Value) -> Option<String> {
    raw.pointer("/item/type")
        .and_then(json_string)
        .or_else(|| raw.pointer("/item/kind").and_then(json_string))
        .or_else(|| raw.pointer("/event/item/type").and_then(json_string))
        .or_else(|| raw.pointer("/event/item/kind").and_then(json_string))
        .map(|item_type| item_type.trim().to_ascii_lowercase())
        .filter(|item_type| !item_type.is_empty())
}

fn adapter_status(raw: &serde_json::Value) -> Option<String> {
    string_field(raw, "status")
        .or_else(|| raw.pointer("/item/status").and_then(json_string))
        .or_else(|| raw.pointer("/event/status").and_then(json_string))
        .or_else(|| raw.pointer("/event/item/status").and_then(json_string))
        .or_else(|| raw.pointer("/result/status").and_then(json_string))
        .or_else(|| raw.pointer("/tool_result/status").and_then(json_string))
        .or_else(|| raw.pointer("/tool_response/status").and_then(json_string))
        .or_else(|| raw.pointer("/response/status").and_then(json_string))
        .or_else(|| {
            raw.pointer("/event/properties/status")
                .and_then(json_string)
        })
        .or_else(|| {
            raw.pointer("/event/properties/result/status")
                .and_then(json_string)
        })
        .or_else(|| {
            raw.pointer("/event/properties/tool_result/status")
                .and_then(json_string)
        })
        .or_else(|| {
            raw.pointer("/event/properties/tool_response/status")
                .and_then(json_string)
        })
        .map(|status| status.trim().to_ascii_lowercase())
        .filter(|status| !status.is_empty())
}

fn string_field(raw: &serde_json::Value, field: &str) -> Option<String> {
    raw.get(field).and_then(json_string)
}

fn adapter_string_field(raw: &serde_json::Value, field: &str) -> Option<String> {
    string_field(raw, field)
        .or_else(|| {
            raw.pointer(&format!("/event/properties/{field}"))
                .and_then(json_string)
        })
        .or_else(|| {
            raw.pointer(&format!("/properties/{field}"))
                .and_then(json_string)
        })
}

fn adapter_u64_field(raw: &serde_json::Value, field: &str) -> Option<u64> {
    u64_field(raw, field)
        .or_else(|| {
            raw.pointer(&format!("/event/properties/{field}"))
                .and_then(serde_json::Value::as_u64)
        })
        .or_else(|| {
            raw.pointer(&format!("/properties/{field}"))
                .and_then(serde_json::Value::as_u64)
        })
}

fn adapter_nested_value<'a>(
    raw: &'a serde_json::Value,
    object: &str,
    field: &str,
) -> Option<&'a serde_json::Value> {
    raw.pointer(&format!("/{object}/{field}"))
        .or_else(|| raw.pointer(&format!("/event/{object}/{field}")))
        .or_else(|| raw.pointer(&format!("/event/properties/{object}/{field}")))
        .or_else(|| raw.pointer(&format!("/properties/{object}/{field}")))
}

fn adapter_nested_string_field(
    raw: &serde_json::Value,
    object: &str,
    field: &str,
) -> Option<String> {
    adapter_nested_value(raw, object, field).and_then(json_string)
}

fn adapter_nested_u64_field(raw: &serde_json::Value, object: &str, field: &str) -> Option<u64> {
    adapter_nested_value(raw, object, field).and_then(serde_json::Value::as_u64)
}

fn adapter_nested_bool_field(raw: &serde_json::Value, object: &str, field: &str) -> Option<bool> {
    adapter_nested_value(raw, object, field).and_then(serde_json::Value::as_bool)
}

fn adapter_nested_string_array_field(
    raw: &serde_json::Value,
    object: &str,
    field: &str,
) -> Vec<String> {
    adapter_nested_value(raw, object, field)
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(json_string)
        .collect()
}

fn adapter_path_field(raw: &serde_json::Value) -> Option<String> {
    adapter_string_field(raw, "file").or_else(|| adapter_string_field(raw, "path"))
}

fn u64_field(raw: &serde_json::Value, field: &str) -> Option<u64> {
    raw.get(field).and_then(serde_json::Value::as_u64)
}

fn u32_field(raw: &serde_json::Value, field: &str) -> Option<u32> {
    u64_field(raw, field).and_then(|value| u32::try_from(value).ok())
}

fn adapter_line_field(raw: &serde_json::Value, field: &str) -> Option<u32> {
    u32_field(raw, field)
        .or_else(|| {
            raw.pointer(&format!("/event/properties/{field}"))
                .and_then(serde_json::Value::as_u64)
                .and_then(|value| u32::try_from(value).ok())
        })
        .or_else(|| {
            raw.pointer(&format!("/properties/{field}"))
                .and_then(serde_json::Value::as_u64)
                .and_then(|value| u32::try_from(value).ok())
        })
}

fn json_string(value: &serde_json::Value) -> Option<String> {
    value.as_str().map(str::to_string)
}

fn json_text(value: &serde_json::Value) -> Option<String> {
    if let Some(text) = value.as_str() {
        return Some(text.to_string());
    }
    value.as_array().map(|items| {
        items
            .iter()
            .filter_map(|item| {
                item.as_str()
                    .map(str::to_string)
                    .or_else(|| item.get("text").and_then(json_string))
                    .or_else(|| item.get("content").and_then(json_string))
            })
            .collect::<Vec<_>>()
            .join("\n")
    })
}
