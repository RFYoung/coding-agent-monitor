use coding_agent_monitor::{
    AgentKind, InstallMode, injection_plan_for, injection_plan_for_workspace,
    install_agent_injection, install_injection_plan,
};

#[test]
fn codex_injection_targets_agents_md() {
    let plan = injection_plan_for(AgentKind::Codex);

    assert_eq!(plan.agent, AgentKind::Codex);
    let rules = plan
        .files
        .iter()
        .find(|file| file.relative_path == "AGENTS.md")
        .expect("Codex project rules");
    assert!(rules.content.contains("External Supervisor Rules"));
    assert!(
        rules
            .content
            .contains("monitor packet exists, it overrides older plans")
    );
    assert!(rules.content.contains(".agent-monitor/tmp"));
    assert!(
        rules
            .content
            .contains("Status format: state | action | verification/probe | blocker")
    );
    assert!(rules.content.contains(
        "Closed means success criteria met, packet stale, or superseded by a newer monitor packet"
    ));
    assert!(
        rules
            .content
            .contains("status fields mean state=working/blocked/done")
    );
    assert!(
        rules
            .content
            .contains(".agent-monitor/outbox/codex/latest.md")
    );
    assert!(rules.content.contains("according to its urgency"));
    assert!(
        !rules
            .content
            .contains("Let monitor hooks capture normalized events"),
        "injection should not spend prompt budget on telemetry narration"
    );
}

#[test]
fn codex_injection_installs_hook_response_bridge() {
    let plan = injection_plan_for(AgentKind::Codex);
    let hook = plan
        .files
        .iter()
        .find(|file| file.relative_path == ".codex/hooks/agent-monitor-pre-tool.ps1")
        .expect("Codex monitor hook script");
    let hooks = plan
        .files
        .iter()
        .find(|file| file.relative_path == ".codex/hooks.json")
        .expect("Codex project hooks");

    assert!(hook.content.contains("agent-monitor"));
    assert!(hook.content.contains("hook-response"));
    assert!(hook.content.contains("--adapter=codex"));
    assert!(hook.content.contains("--format=codex"));
    assert!(hook.content.contains("ReadToEnd"));

    let hooks_json: serde_json::Value = serde_json::from_str(&hooks.content).expect("hooks json");
    let pre_tool_hooks = hooks_json
        .pointer("/hooks/PreToolUse")
        .and_then(serde_json::Value::as_array)
        .expect("PreToolUse hooks");
    let rendered = serde_json::to_string(pre_tool_hooks).expect("render hooks");
    assert!(rendered.contains("agent-monitor-pre-tool.ps1"));
    assert!(rendered.contains("command"));
}

#[test]
fn codex_injection_installs_ingest_event_bridge() {
    let plan = injection_plan_for(AgentKind::Codex);
    let hook = plan
        .files
        .iter()
        .find(|file| file.relative_path == ".codex/hooks/agent-monitor-event.ps1")
        .expect("Codex monitor event hook script");
    let hooks = plan
        .files
        .iter()
        .find(|file| file.relative_path == ".codex/hooks.json")
        .expect("Codex project hooks");

    assert!(hook.content.contains("agent-monitor"));
    assert!(hook.content.contains("ingest"));
    assert!(hook.content.contains("--adapter=codex"));
    assert!(hook.content.contains("ReadToEnd"));

    let hooks_json: serde_json::Value = serde_json::from_str(&hooks.content).expect("hooks json");
    let rendered = serde_json::to_string(&hooks_json).expect("render hooks");
    assert!(rendered.contains("PostToolUse"));
    assert!(rendered.contains("Stop"));
    assert!(rendered.contains("PreCompact"));
    assert!(rendered.contains("agent-monitor-event.ps1"));
}

#[test]
fn claude_code_injection_targets_claude_md() {
    let plan = injection_plan_for(AgentKind::ClaudeCode);

    let rules = plan
        .files
        .iter()
        .find(|file| file.relative_path == "CLAUDE.md")
        .expect("Claude project rules");
    assert!(rules.content.contains("Do not stop early"));
    assert!(rules.content.contains("verification"));
    assert!(
        rules
            .content
            .contains("open work, verification, unresolved workers")
    );
    assert!(
        rules
            .content
            .contains(".agent-monitor/outbox/claude-code/latest.md")
    );
    assert!(rules.content.contains("according to its urgency"));
    assert!(rules.content.contains(
        "Closed means success criteria met, packet stale, or superseded by a newer monitor packet"
    ));
}

#[test]
fn claude_code_injection_installs_hook_response_bridge() {
    let plan = injection_plan_for(AgentKind::ClaudeCode);
    let hook = plan
        .files
        .iter()
        .find(|file| file.relative_path == ".claude/hooks/agent-monitor-pre-tool.ps1")
        .expect("Claude Code monitor hook script");
    let settings = plan
        .files
        .iter()
        .find(|file| file.relative_path == ".claude/settings.json")
        .expect("Claude Code project settings");

    assert!(hook.content.contains("agent-monitor"));
    assert!(hook.content.contains("hook-response"));
    assert!(hook.content.contains("--adapter=claude-code"));
    assert!(hook.content.contains("--format=claude-code"));
    assert!(hook.content.contains("ReadToEnd"));

    let settings_json: serde_json::Value =
        serde_json::from_str(&settings.content).expect("settings json");
    let pre_tool_hooks = settings_json
        .pointer("/hooks/PreToolUse")
        .and_then(serde_json::Value::as_array)
        .expect("PreToolUse hooks");
    let rendered = serde_json::to_string(pre_tool_hooks).expect("render hooks");
    assert!(rendered.contains("agent-monitor-pre-tool.ps1"));
    assert!(rendered.contains("command"));
}

#[test]
fn claude_code_injection_installs_ingest_event_bridge() {
    let plan = injection_plan_for(AgentKind::ClaudeCode);
    let hook = plan
        .files
        .iter()
        .find(|file| file.relative_path == ".claude/hooks/agent-monitor-event.ps1")
        .expect("Claude Code monitor event hook script");
    let settings = plan
        .files
        .iter()
        .find(|file| file.relative_path == ".claude/settings.json")
        .expect("Claude Code project settings");

    assert!(hook.content.contains("agent-monitor"));
    assert!(hook.content.contains("ingest"));
    assert!(hook.content.contains("--adapter=claude-code"));
    assert!(hook.content.contains("ReadToEnd"));

    let settings_json: serde_json::Value =
        serde_json::from_str(&settings.content).expect("settings json");
    let rendered = serde_json::to_string(&settings_json).expect("render settings");
    assert!(rendered.contains("SessionStart"));
    assert!(rendered.contains("UserPromptSubmit"));
    assert!(rendered.contains("PostToolUse"));
    assert!(rendered.contains("Stop"));
    assert!(rendered.contains("SubagentStop"));
    assert!(rendered.contains("PreCompact"));
    assert!(rendered.contains("Notification"));
    assert!(rendered.contains("agent-monitor-event.ps1"));
}

#[test]
fn pi_injection_is_exportable_as_context() {
    let plan = injection_plan_for(AgentKind::Pi);

    assert_eq!(
        plan.files[0].relative_path,
        ".agent-monitor/injections/pi.md"
    );
    assert!(plan.files[0].content.contains("Handoff"));
    assert!(plan.files[0].content.contains("durable design memory"));
    assert!(plan.files[0].content.contains("bounded helper"));
    assert!(
        plan.files[0]
            .content
            .contains(".agent-monitor/outbox/pi/latest.md")
    );
    assert!(plan.files[0].content.contains("according to its urgency"));
}

#[test]
fn opencode_injection_uses_agents_md_project_rules() {
    let plan = injection_plan_for(AgentKind::OpenCode);

    assert_eq!(plan.files[0].relative_path, "AGENTS.md");
    assert!(plan.files[0].content.contains("normalized events"));
    assert!(
        !plan.files[0].content.contains("telemetry cover"),
        "OpenCode injection should be operational, not explanatory"
    );
    assert!(
        plan.files[0]
            .content
            .contains(".agent-monitor/outbox/opencode/latest.md")
    );
    assert!(plan.files[0].content.contains("according to its urgency"));
}

#[test]
fn opencode_injection_installs_hook_response_plugin() {
    let plan = injection_plan_for(AgentKind::OpenCode);
    let plugin = plan
        .files
        .iter()
        .find(|file| file.relative_path == ".opencode/plugins/agent-monitor.js")
        .expect("OpenCode monitor plugin");

    assert!(plugin.content.contains("tool.execute.before"));
    assert!(plugin.content.contains("agent-monitor"));
    assert!(plugin.content.contains("hook-response"));
    assert!(plugin.content.contains("--adapter=opencode"));
    assert!(plugin.content.contains("--format=opencode"));
    assert!(plugin.content.contains("throw new Error"));
}

#[test]
fn opencode_injection_installs_ingest_event_plugin_hooks() {
    let plan = injection_plan_for(AgentKind::OpenCode);
    let plugin = plan
        .files
        .iter()
        .find(|file| file.relative_path == ".opencode/plugins/agent-monitor.js")
        .expect("OpenCode monitor plugin");

    assert!(plugin.content.contains("tool.execute.after"));
    assert!(plugin.content.contains("session.idle"));
    assert!(plugin.content.contains("session.error"));
    assert!(plugin.content.contains("ingest"));
    assert!(plugin.content.contains("--adapter=opencode"));
}

#[test]
fn agent_kind_parses_supported_slugs() {
    assert_eq!("codex".parse(), Ok(AgentKind::Codex));
    assert_eq!("claude-code".parse(), Ok(AgentKind::ClaudeCode));
    assert_eq!("pi".parse(), Ok(AgentKind::Pi));
    assert_eq!("opencode".parse(), Ok(AgentKind::OpenCode));
    assert!("unknown".parse::<AgentKind>().is_err());
}

#[test]
fn install_injection_plan_writes_nested_files() {
    let temp = tempfile::tempdir().expect("temp dir");
    let plan = injection_plan_for(AgentKind::Pi);

    install_injection_plan(temp.path(), &plan, InstallMode::CreateOrOverwrite)
        .expect("install injection");

    let path = temp.path().join(".agent-monitor/injections/pi.md");
    let content = std::fs::read_to_string(path).expect("pi injection");
    assert!(content.contains("durable design memory"));
}

#[test]
fn injection_plan_for_workspace_rejects_disabled_adapter() {
    let temp = tempfile::tempdir().expect("temp dir");
    std::fs::create_dir_all(temp.path().join(".agent-monitor")).expect("store dir");
    std::fs::write(
        temp.path().join(".agent-monitor").join("config.json"),
        r#"{
          "adapters": {
            "claude_code": {
              "enabled": false
            }
          }
        }"#,
    )
    .expect("config");

    let err = injection_plan_for_workspace(temp.path(), AgentKind::ClaudeCode)
        .expect_err("disabled adapter should not receive injection");

    assert!(err.to_string().contains("disabled"));
}

#[test]
fn install_agent_injection_rejects_disabled_adapter_without_writing() {
    let temp = tempfile::tempdir().expect("temp dir");
    std::fs::create_dir_all(temp.path().join(".agent-monitor")).expect("store dir");
    std::fs::write(
        temp.path().join(".agent-monitor").join("config.json"),
        r#"{
          "adapters": {
            "claude_code": {
              "enabled": false
            }
          }
        }"#,
    )
    .expect("config");

    let err = install_agent_injection(
        temp.path(),
        AgentKind::ClaudeCode,
        InstallMode::MergeManagedBlock,
    )
    .expect_err("disabled adapter should not be installed");

    assert!(err.to_string().contains("disabled"));
    assert!(!temp.path().join("CLAUDE.md").exists());
}

#[test]
fn install_injection_plan_refuses_to_overwrite_by_default() {
    let temp = tempfile::tempdir().expect("temp dir");
    let plan = injection_plan_for(AgentKind::Codex);
    let path = temp.path().join("AGENTS.md");
    std::fs::write(&path, "existing").expect("seed AGENTS.md");

    let err = install_injection_plan(temp.path(), &plan, InstallMode::CreateNew)
        .expect_err("install should refuse overwrite");

    assert!(err.to_string().contains("already exists"));
    assert_eq!(
        std::fs::read_to_string(path).expect("unchanged file"),
        "existing"
    );
}

#[test]
fn install_injection_plan_merges_managed_block_without_overwriting_user_content() {
    let temp = tempfile::tempdir().expect("temp dir");
    let plan = injection_plan_for(AgentKind::Codex);
    let path = temp.path().join("AGENTS.md");
    std::fs::write(&path, "# Project Rules\n\nKeep this user rule.\n").expect("seed AGENTS.md");

    install_injection_plan(temp.path(), &plan, InstallMode::MergeManagedBlock)
        .expect("merge injection");

    let content = std::fs::read_to_string(path).expect("merged AGENTS.md");
    assert!(content.contains("Keep this user rule."));
    assert!(content.contains("BEGIN AGENT MONITOR MANAGED BLOCK"));
    assert!(content.contains("External Supervisor Rules"));
}

#[test]
fn install_injection_plan_keeps_codex_and_opencode_blocks_in_shared_agents_md() {
    let temp = tempfile::tempdir().expect("temp dir");
    let codex = injection_plan_for(AgentKind::Codex);
    let opencode = injection_plan_for(AgentKind::OpenCode);
    let path = temp.path().join("AGENTS.md");
    std::fs::write(&path, "# Project Rules\n\nKeep this user rule.\n").expect("seed AGENTS.md");

    install_injection_plan(temp.path(), &codex, InstallMode::MergeManagedBlock)
        .expect("merge Codex injection");
    install_injection_plan(temp.path(), &opencode, InstallMode::MergeManagedBlock)
        .expect("merge OpenCode injection");

    let content = std::fs::read_to_string(path).expect("merged AGENTS.md");
    assert!(content.contains("Keep this user rule."));
    assert!(content.contains("BEGIN AGENT MONITOR MANAGED BLOCK: codex"));
    assert!(content.contains("BEGIN AGENT MONITOR MANAGED BLOCK: opencode"));
    assert!(content.contains(".agent-monitor/outbox/codex/latest.md"));
    assert!(content.contains(".agent-monitor/outbox/opencode/latest.md"));
    assert!(content.contains("Project hook `.codex/hooks.json`"));
    assert!(content.contains("Project plugin `.opencode/plugins/agent-monitor.js`"));
}

#[test]
fn install_injection_plan_updates_only_existing_managed_block() {
    let temp = tempfile::tempdir().expect("temp dir");
    let plan = injection_plan_for(AgentKind::ClaudeCode);
    let path = temp.path().join("CLAUDE.md");
    std::fs::write(
        &path,
        "# Claude Rules\nbefore\n\n<!-- BEGIN AGENT MONITOR MANAGED BLOCK -->\nold monitor text\n<!-- END AGENT MONITOR MANAGED BLOCK -->\n\nafter\n",
    )
    .expect("seed CLAUDE.md");

    install_injection_plan(temp.path(), &plan, InstallMode::MergeManagedBlock)
        .expect("merge injection");

    let content = std::fs::read_to_string(path).expect("merged CLAUDE.md");
    assert!(content.contains("# Claude Rules\nbefore"));
    assert!(content.contains("\nafter"));
    assert!(!content.contains("old monitor text"));
    assert_eq!(
        content.matches("BEGIN AGENT MONITOR MANAGED BLOCK").count(),
        1
    );
    assert!(content.contains("Handoff or stop summaries"));
}

#[test]
fn install_injection_plan_migrates_legacy_markdown_block_to_agent_scoped_block() {
    let temp = tempfile::tempdir().expect("temp dir");
    let plan = injection_plan_for(AgentKind::Codex);
    let path = temp.path().join("AGENTS.md");
    std::fs::write(
        &path,
        "# Project Rules\nbefore\n\n<!-- BEGIN AGENT MONITOR MANAGED BLOCK -->\nold monitor text\n<!-- END AGENT MONITOR MANAGED BLOCK -->\n\nafter\n",
    )
    .expect("seed AGENTS.md");

    install_injection_plan(temp.path(), &plan, InstallMode::MergeManagedBlock)
        .expect("merge injection");

    let content = std::fs::read_to_string(path).expect("merged AGENTS.md");
    assert!(content.contains("# Project Rules\nbefore"));
    assert!(content.contains("\nafter"));
    assert!(!content.contains("old monitor text"));
    assert!(!content.contains("BEGIN AGENT MONITOR MANAGED BLOCK -->"));
    assert_eq!(
        content
            .matches("BEGIN AGENT MONITOR MANAGED BLOCK: codex")
            .count(),
        1
    );
    assert!(content.contains(".agent-monitor/outbox/codex/latest.md"));
}

#[test]
fn install_injection_plan_writes_non_markdown_hook_files_without_managed_block() {
    let temp = tempfile::tempdir().expect("temp dir");
    let plan = injection_plan_for(AgentKind::OpenCode);

    install_injection_plan(temp.path(), &plan, InstallMode::MergeManagedBlock)
        .expect("merge opencode injection");

    let rules = std::fs::read_to_string(temp.path().join("AGENTS.md")).expect("AGENTS.md");
    assert!(rules.contains("BEGIN AGENT MONITOR MANAGED BLOCK"));

    let plugin = std::fs::read_to_string(
        temp.path()
            .join(".opencode")
            .join("plugins")
            .join("agent-monitor.js"),
    )
    .expect("OpenCode plugin");
    assert!(plugin.trim_start().starts_with("import "));
    assert!(!plugin.contains("BEGIN AGENT MONITOR MANAGED BLOCK"));
}

#[test]
fn install_injection_plan_merges_codex_hooks_json_without_overwriting_user_hooks() {
    let temp = tempfile::tempdir().expect("temp dir");
    let plan = injection_plan_for(AgentKind::Codex);
    let hooks_path = temp.path().join(".codex").join("hooks.json");
    std::fs::create_dir_all(hooks_path.parent().expect("hooks parent")).expect("hooks dir");
    std::fs::write(
        &hooks_path,
        r#"{
          "hooks": {
            "PreToolUse": [
              {
                "matcher": "Shell",
                "hooks": [
                  {
                    "type": "command",
                    "command": "existing-codex-policy"
                  }
                ]
              }
            ]
          }
        }"#,
    )
    .expect("seed hooks");

    install_injection_plan(temp.path(), &plan, InstallMode::MergeManagedBlock)
        .expect("merge Codex injection");

    let hooks_text = std::fs::read_to_string(&hooks_path).expect("hooks json");
    assert!(!hooks_text.contains("BEGIN AGENT MONITOR MANAGED BLOCK"));
    let hooks: serde_json::Value = serde_json::from_str(&hooks_text).expect("merged hooks json");
    let pre_tool_hooks = hooks
        .pointer("/hooks/PreToolUse")
        .and_then(serde_json::Value::as_array)
        .expect("PreToolUse hooks");
    let rendered = serde_json::to_string(pre_tool_hooks).expect("render hooks");
    assert!(rendered.contains("existing-codex-policy"));
    assert!(rendered.contains("agent-monitor-pre-tool.ps1"));

    let hook_script = std::fs::read_to_string(
        temp.path()
            .join(".codex")
            .join("hooks")
            .join("agent-monitor-pre-tool.ps1"),
    )
    .expect("hook script");
    assert!(hook_script.contains("ReadToEnd"));
    assert!(!hook_script.contains("BEGIN AGENT MONITOR MANAGED BLOCK"));
}

#[test]
fn install_injection_plan_merges_claude_settings_json_without_overwriting_user_hooks() {
    let temp = tempfile::tempdir().expect("temp dir");
    let plan = injection_plan_for(AgentKind::ClaudeCode);
    let settings_path = temp.path().join(".claude").join("settings.json");
    std::fs::create_dir_all(settings_path.parent().expect("settings parent"))
        .expect("settings dir");
    std::fs::write(
        &settings_path,
        r#"{
          "model": "sonnet",
          "hooks": {
            "PreToolUse": [
              {
                "matcher": "Bash",
                "hooks": [
                  {
                    "type": "command",
                    "command": "existing-policy"
                  }
                ]
              }
            ]
          }
        }"#,
    )
    .expect("seed settings");

    install_injection_plan(temp.path(), &plan, InstallMode::MergeManagedBlock)
        .expect("merge Claude injection");

    let settings_text = std::fs::read_to_string(&settings_path).expect("settings");
    assert!(!settings_text.contains("BEGIN AGENT MONITOR MANAGED BLOCK"));
    let settings: serde_json::Value =
        serde_json::from_str(&settings_text).expect("merged settings json");
    assert_eq!(
        settings
            .pointer("/model")
            .and_then(serde_json::Value::as_str),
        Some("sonnet")
    );

    let pre_tool_hooks = settings
        .pointer("/hooks/PreToolUse")
        .and_then(serde_json::Value::as_array)
        .expect("PreToolUse hooks");
    let rendered = serde_json::to_string(pre_tool_hooks).expect("render hooks");
    assert!(rendered.contains("existing-policy"));
    assert!(rendered.contains("agent-monitor-pre-tool.ps1"));

    let hook_script = std::fs::read_to_string(
        temp.path()
            .join(".claude")
            .join("hooks")
            .join("agent-monitor-pre-tool.ps1"),
    )
    .expect("hook script");
    assert!(hook_script.contains("ReadToEnd"));
    assert!(!hook_script.contains("BEGIN AGENT MONITOR MANAGED BLOCK"));
}
