use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use crate::{
    AgentKind, ProjectConfig, ProjectConfigError, adapter_capabilities_for_config, agent_kind_label,
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InjectionPlan {
    pub agent: AgentKind,
    pub files: Vec<InjectionFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InjectionFile {
    pub relative_path: String,
    pub content: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallMode {
    CreateNew,
    CreateOrOverwrite,
    MergeManagedBlock,
}

const INJECTION_BLOCK_BEGIN: &str = "<!-- BEGIN AGENT MONITOR MANAGED BLOCK -->";
const INJECTION_BLOCK_END: &str = "<!-- END AGENT MONITOR MANAGED BLOCK -->";

pub fn injection_plan_for(agent: AgentKind) -> InjectionPlan {
    match agent {
        AgentKind::Codex => InjectionPlan {
            agent,
            files: vec![
                InjectionFile {
                    relative_path: "AGENTS.md".into(),
                    content: codex_injection(),
                },
                InjectionFile {
                    relative_path: ".codex/hooks/agent-monitor-pre-tool.ps1".into(),
                    content: codex_hook_script(),
                },
                InjectionFile {
                    relative_path: ".codex/hooks/agent-monitor-event.ps1".into(),
                    content: codex_event_hook_script(),
                },
                InjectionFile {
                    relative_path: ".codex/hooks.json".into(),
                    content: codex_hook_settings(),
                },
            ],
        },
        AgentKind::ClaudeCode => InjectionPlan {
            agent,
            files: vec![
                InjectionFile {
                    relative_path: "CLAUDE.md".into(),
                    content: claude_code_injection(),
                },
                InjectionFile {
                    relative_path: ".claude/hooks/agent-monitor-pre-tool.ps1".into(),
                    content: claude_code_hook_script(),
                },
                InjectionFile {
                    relative_path: ".claude/hooks/agent-monitor-event.ps1".into(),
                    content: claude_code_event_hook_script(),
                },
                InjectionFile {
                    relative_path: ".claude/settings.json".into(),
                    content: claude_code_hook_settings(),
                },
            ],
        },
        AgentKind::Pi => InjectionPlan {
            agent,
            files: vec![InjectionFile {
                relative_path: ".agent-monitor/injections/pi.md".into(),
                content: pi_injection(),
            }],
        },
        AgentKind::OpenCode => InjectionPlan {
            agent,
            files: vec![
                InjectionFile {
                    relative_path: "AGENTS.md".into(),
                    content: opencode_injection(),
                },
                InjectionFile {
                    relative_path: ".opencode/plugins/agent-monitor.js".into(),
                    content: opencode_hook_plugin(),
                },
            ],
        },
    }
}

pub fn injection_plan_for_workspace(
    workspace: impl AsRef<Path>,
    agent: AgentKind,
) -> Result<InjectionPlan, InjectionInstallError> {
    let store_root = workspace.as_ref().join(".agent-monitor");
    let config = ProjectConfig::load(&store_root)?;
    let capabilities = adapter_capabilities_for_config(agent, &config.adapters);
    if !capabilities.enabled {
        return Err(InjectionInstallError::AdapterDisabled {
            agent: agent_kind_label(agent).into(),
        });
    }
    Ok(injection_plan_for(agent))
}

pub fn install_agent_injection(
    workspace: impl AsRef<Path>,
    agent: AgentKind,
    mode: InstallMode,
) -> Result<Vec<PathBuf>, InjectionInstallError> {
    let plan = injection_plan_for_workspace(workspace.as_ref(), agent)?;
    install_injection_plan(workspace, &plan, mode)
}

pub fn install_injection_plan(
    workspace_root: impl AsRef<Path>,
    plan: &InjectionPlan,
    mode: InstallMode,
) -> Result<Vec<PathBuf>, InjectionInstallError> {
    let workspace_root = workspace_root.as_ref();
    let mut written = Vec::with_capacity(plan.files.len());

    for file in &plan.files {
        let relative_path = Path::new(&file.relative_path);
        if !is_safe_relative_path(relative_path) {
            return Err(InjectionInstallError::UnsafePath {
                path: file.relative_path.clone(),
            });
        }

        let target = workspace_root.join(relative_path);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|source| InjectionInstallError::CreateDir {
                path: parent.to_path_buf(),
                source,
            })?;
        }
        if mode == InstallMode::CreateNew && target.exists() {
            return Err(InjectionInstallError::AlreadyExists { path: target });
        }

        if mode == InstallMode::MergeManagedBlock && injection_file_uses_managed_block(file) {
            let current = if target.exists() {
                fs::read_to_string(&target).map_err(|source| InjectionInstallError::Read {
                    path: target.clone(),
                    source,
                })?
            } else {
                String::new()
            };
            let merged = merge_managed_injection_block(&current, &file.content, plan.agent);
            let mut handle = OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&target)
                .map_err(|source| InjectionInstallError::Write {
                    path: target.clone(),
                    source,
                })?;
            handle
                .write_all(merged.as_bytes())
                .map_err(|source| InjectionInstallError::Write {
                    path: target.clone(),
                    source,
                })?;
            written.push(target);
            continue;
        }

        if mode == InstallMode::MergeManagedBlock && injection_file_uses_json_merge(file) {
            let mut current = if target.exists() {
                let current =
                    fs::read_to_string(&target).map_err(|source| InjectionInstallError::Read {
                        path: target.clone(),
                        source,
                    })?;
                if current.trim().is_empty() {
                    Value::Object(Default::default())
                } else {
                    serde_json::from_str(&current).map_err(|source| {
                        InjectionInstallError::JsonDecode {
                            path: target.clone(),
                            source,
                        }
                    })?
                }
            } else {
                Value::Object(Default::default())
            };
            let injection: Value = serde_json::from_str(&file.content).map_err(|source| {
                InjectionInstallError::JsonDecode {
                    path: target.clone(),
                    source,
                }
            })?;
            merge_json_values(&mut current, injection);
            let merged = serde_json::to_string_pretty(&current).map_err(|source| {
                InjectionInstallError::JsonEncode {
                    path: target.clone(),
                    source,
                }
            })?;

            let mut handle = OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(&target)
                .map_err(|source| InjectionInstallError::Write {
                    path: target.clone(),
                    source,
                })?;
            handle
                .write_all(merged.as_bytes())
                .map_err(|source| InjectionInstallError::Write {
                    path: target.clone(),
                    source,
                })?;
            handle
                .write_all(b"\n")
                .map_err(|source| InjectionInstallError::Write {
                    path: target.clone(),
                    source,
                })?;
            written.push(target);
            continue;
        }

        let mut options = OpenOptions::new();
        options.write(true);
        match mode {
            InstallMode::CreateNew => {
                options.create_new(true);
            }
            InstallMode::CreateOrOverwrite => {
                options.create(true).truncate(true);
            }
            InstallMode::MergeManagedBlock => {
                options.create(true).truncate(true);
            }
        }

        let mut handle = options
            .open(&target)
            .map_err(|source| InjectionInstallError::Write {
                path: target.clone(),
                source,
            })?;
        handle
            .write_all(file.content.as_bytes())
            .map_err(|source| InjectionInstallError::Write {
                path: target.clone(),
                source,
            })?;
        written.push(target);
    }

    Ok(written)
}

fn injection_file_uses_managed_block(file: &InjectionFile) -> bool {
    let path = Path::new(&file.relative_path);
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
}

fn injection_file_uses_json_merge(file: &InjectionFile) -> bool {
    let path = Path::new(&file.relative_path);
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
}

fn merge_json_values(current: &mut Value, injection: Value) {
    match (current, injection) {
        (Value::Object(current), Value::Object(injection)) => {
            for (key, injected_value) in injection {
                if let Some(current_value) = current.get_mut(&key) {
                    merge_json_values(current_value, injected_value);
                } else {
                    current.insert(key, injected_value);
                }
            }
        }
        (Value::Array(current), Value::Array(injection)) => {
            for injected_value in injection {
                if !current
                    .iter()
                    .any(|current_value| current_value == &injected_value)
                {
                    current.push(injected_value);
                }
            }
        }
        (current, injection) => {
            *current = injection;
        }
    }
}

fn merge_managed_injection_block(existing: &str, injection: &str, agent: AgentKind) -> String {
    let agent_slug = agent_kind_label(agent);
    let block_begin = format!("<!-- BEGIN AGENT MONITOR MANAGED BLOCK: {agent_slug} -->");
    let block_end = format!("<!-- END AGENT MONITOR MANAGED BLOCK: {agent_slug} -->");
    let block = format!("{block_begin}\n{}\n{block_end}", injection.trim());

    if let Some(begin) = existing.find(&block_begin)
        && let Some(end_offset) = existing[begin..].find(&block_end)
    {
        let end = begin + end_offset + block_end.len();
        let mut merged = String::new();
        merged.push_str(existing[..begin].trim_end());
        merged.push_str("\n\n");
        merged.push_str(&block);
        merged.push('\n');
        merged.push_str(existing[end..].trim_start_matches(['\r', '\n']));
        return merged;
    }

    if let Some(begin) = existing.find(INJECTION_BLOCK_BEGIN)
        && let Some(end_offset) = existing[begin..].find(INJECTION_BLOCK_END)
    {
        let end = begin + end_offset + INJECTION_BLOCK_END.len();
        let mut merged = String::new();
        merged.push_str(existing[..begin].trim_end());
        merged.push_str("\n\n");
        merged.push_str(&block);
        merged.push('\n');
        merged.push_str(existing[end..].trim_start_matches(['\r', '\n']));
        return merged;
    }

    if existing.trim().is_empty() {
        format!("{block}\n")
    } else {
        format!("{}\n\n{block}\n", existing.trim_end())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum InjectionInstallError {
    #[error("load injection project config: {0}")]
    Config(#[from] ProjectConfigError),
    #[error("adapter {agent} is disabled in project config; refusing injection")]
    AdapterDisabled { agent: String },
    #[error("unsafe injection path: {path}")]
    UnsafePath { path: String },
    #[error("injection target already exists: {path}")]
    AlreadyExists { path: PathBuf },
    #[error("create directory {path}: {source}")]
    CreateDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("read {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("decode json {path}: {source}")]
    JsonDecode {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("encode json {path}: {source}")]
    JsonEncode {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("write {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

fn shared_injection_body() -> &'static str {
    "External Supervisor Rules\n\
    - If a monitor packet exists, it overrides older plans until its required action is closed; do not dilute it with stale context.\n\
    - Closed means success criteria met, packet stale, or superseded by a newer monitor packet.\n\
    - Do not stop early while requested work, tests, verifier/probe results, or trace rationale remain open.\n\
    - Ask the user only for destructive actions, credentials, spending, external side effects, or ambiguous product authority.\n\
    - Keep generated temporary files under .agent-monitor/tmp.\n\
    - For meaningful changes, keep trace rationale tied to durable memory, user request, failing verifier, or recovery action.\n\
    - If blocked, state the exact blocker and local evidence checked; do not ask routine sequencing questions.\n\
    - Status format: state | action | verification/probe | blocker.\n\
    - status fields mean state=working/blocked/done, action=next command/edit, verification/probe=latest result or needed run, blocker=none or exact missing authority/evidence.\n"
}

fn latest_outbox_instruction(agent_slug: &str) -> String {
    format!(
        "    - At the start of each turn, read `.agent-monitor/outbox/{agent_slug}/latest.md` if it exists; follow the packet according to its urgency before continuing an older plan, treating blocking/urgent packets as immediate gates.\n"
    )
}

fn codex_injection() -> String {
    format!(
        "{}\
        {}\
        - Before final response, run relevant verification or state the concrete blocker.\n\
        - Project hook `.codex/hooks.json` is monitor-owned; let it enforce hook-response and event ingest.\n",
        shared_injection_body(),
        latest_outbox_instruction("codex")
    )
}

fn codex_hook_settings() -> String {
    r#"{
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "*",
        "hooks": [
          {
            "type": "command",
            "command": "powershell -NoProfile -ExecutionPolicy Bypass -File .codex/hooks/agent-monitor-pre-tool.ps1",
            "timeout": 30
          }
        ]
      }
    ],
    "PostToolUse": [
      {
        "matcher": "*",
        "hooks": [
          {
            "type": "command",
            "command": "powershell -NoProfile -ExecutionPolicy Bypass -File .codex/hooks/agent-monitor-event.ps1",
            "timeout": 30
          }
        ]
      }
    ],
    "Stop": [
      {
        "matcher": "*",
        "hooks": [
          {
            "type": "command",
            "command": "powershell -NoProfile -ExecutionPolicy Bypass -File .codex/hooks/agent-monitor-event.ps1",
            "timeout": 30
          }
        ]
      }
    ],
    "PreCompact": [
      {
        "matcher": "*",
        "hooks": [
          {
            "type": "command",
            "command": "powershell -NoProfile -ExecutionPolicy Bypass -File .codex/hooks/agent-monitor-event.ps1",
            "timeout": 30
          }
        ]
      }
    ]
  }
}
"#
    .into()
}

fn codex_hook_script() -> String {
    r#"$ErrorActionPreference = "Stop"

try {
  $inputJson = [Console]::In.ReadToEnd()
  if ($env:CODEX_PROJECT_DIR) {
    $workspace = $env:CODEX_PROJECT_DIR
  } else {
    $workspace = (Get-Location).Path
  }
  if ($env:AGENT_MONITOR_BIN) {
    $monitor = $env:AGENT_MONITOR_BIN
  } else {
    $monitor = "agent-monitor"
  }

  $arguments = @(
    "hook-response",
    "--adapter=codex",
    "--workspace=$workspace",
    "--format=codex"
  )
  $output = $inputJson | & $monitor @arguments 2>&1
  $exitCode = $LASTEXITCODE
  $text = ($output | Out-String).Trim()

  if ($exitCode -ne 0) {
    if ($text.Length -gt 0) {
      [Console]::Error.WriteLine($text)
    } else {
      [Console]::Error.WriteLine("agent-monitor hook policy unavailable")
    }
    exit 2
  }

  if ($text.Length -gt 0) {
    [Console]::Out.Write($text)
  }
  exit 0
} catch {
  [Console]::Error.WriteLine($_.Exception.Message)
  exit 2
}
"#
    .into()
}

fn codex_event_hook_script() -> String {
    r#"$ErrorActionPreference = "Continue"

try {
  $inputJson = [Console]::In.ReadToEnd()
  if ($env:CODEX_PROJECT_DIR) {
    $workspace = $env:CODEX_PROJECT_DIR
  } else {
    $workspace = (Get-Location).Path
  }
  if ($env:AGENT_MONITOR_BIN) {
    $monitor = $env:AGENT_MONITOR_BIN
  } else {
    $monitor = "agent-monitor"
  }

  $arguments = @(
    "ingest",
    "--adapter=codex",
    "--workspace=$workspace"
  )
  $output = $inputJson | & $monitor @arguments 2>&1
  $exitCode = $LASTEXITCODE
  $text = ($output | Out-String).Trim()

  if ($exitCode -ne 0 -and $text.Length -gt 0) {
    [Console]::Error.WriteLine($text)
  }
  exit 0
} catch {
  [Console]::Error.WriteLine($_.Exception.Message)
  exit 0
}
"#
    .into()
}

fn claude_code_injection() -> String {
    format!(
        "{}\
        {}\
        - Handoff or stop summaries must include open work, verification, unresolved workers, and the next safe action.\n\
        - If service instability appears, record the failure class and continue through retry or handoff instead of stopping.\n",
        shared_injection_body(),
        latest_outbox_instruction("claude-code")
    )
}

fn claude_code_hook_settings() -> String {
    r#"{
  "hooks": {
    "SessionStart": [
      {
        "matcher": "*",
        "hooks": [
          {
            "type": "command",
            "shell": "powershell",
            "command": "& \"${CLAUDE_PROJECT_DIR}/.claude/hooks/agent-monitor-event.ps1\"",
            "timeout": 30
          }
        ]
      }
    ],
    "PreToolUse": [
      {
        "matcher": "*",
        "hooks": [
          {
            "type": "command",
            "shell": "powershell",
            "command": "& \"${CLAUDE_PROJECT_DIR}/.claude/hooks/agent-monitor-pre-tool.ps1\"",
            "timeout": 30
          }
        ]
      }
    ],
    "UserPromptSubmit": [
      {
        "matcher": "*",
        "hooks": [
          {
            "type": "command",
            "shell": "powershell",
            "command": "& \"${CLAUDE_PROJECT_DIR}/.claude/hooks/agent-monitor-event.ps1\"",
            "timeout": 30
          }
        ]
      }
    ],
    "PostToolUse": [
      {
        "matcher": "*",
        "hooks": [
          {
            "type": "command",
            "shell": "powershell",
            "command": "& \"${CLAUDE_PROJECT_DIR}/.claude/hooks/agent-monitor-event.ps1\"",
            "timeout": 30
          }
        ]
      }
    ],
    "Stop": [
      {
        "matcher": "*",
        "hooks": [
          {
            "type": "command",
            "shell": "powershell",
            "command": "& \"${CLAUDE_PROJECT_DIR}/.claude/hooks/agent-monitor-event.ps1\"",
            "timeout": 30
          }
        ]
      }
    ],
    "SubagentStop": [
      {
        "matcher": "*",
        "hooks": [
          {
            "type": "command",
            "shell": "powershell",
            "command": "& \"${CLAUDE_PROJECT_DIR}/.claude/hooks/agent-monitor-event.ps1\"",
            "timeout": 30
          }
        ]
      }
    ],
    "PreCompact": [
      {
        "matcher": "*",
        "hooks": [
          {
            "type": "command",
            "shell": "powershell",
            "command": "& \"${CLAUDE_PROJECT_DIR}/.claude/hooks/agent-monitor-event.ps1\"",
            "timeout": 30
          }
        ]
      }
    ],
    "Notification": [
      {
        "matcher": "*",
        "hooks": [
          {
            "type": "command",
            "shell": "powershell",
            "command": "& \"${CLAUDE_PROJECT_DIR}/.claude/hooks/agent-monitor-event.ps1\"",
            "timeout": 30
          }
        ]
      }
    ]
  }
}
"#
    .into()
}

fn claude_code_hook_script() -> String {
    r#"$ErrorActionPreference = "Stop"

try {
  $inputJson = [Console]::In.ReadToEnd()
  if ($env:CLAUDE_PROJECT_DIR) {
    $workspace = $env:CLAUDE_PROJECT_DIR
  } else {
    $workspace = (Get-Location).Path
  }
  if ($env:AGENT_MONITOR_BIN) {
    $monitor = $env:AGENT_MONITOR_BIN
  } else {
    $monitor = "agent-monitor"
  }

  $arguments = @(
    "hook-response",
    "--adapter=claude-code",
    "--workspace=$workspace",
    "--format=claude-code"
  )
  $output = $inputJson | & $monitor @arguments 2>&1
  $exitCode = $LASTEXITCODE
  $text = ($output | Out-String).Trim()

  if ($exitCode -ne 0) {
    if ($text.Length -gt 0) {
      [Console]::Error.WriteLine($text)
    } else {
      [Console]::Error.WriteLine("agent-monitor hook policy unavailable")
    }
    exit 2
  }

  if ($text.Length -gt 0) {
    [Console]::Out.Write($text)
  }
  exit 0
} catch {
  [Console]::Error.WriteLine($_.Exception.Message)
  exit 2
}
"#
    .into()
}

fn claude_code_event_hook_script() -> String {
    r#"$ErrorActionPreference = "Continue"

try {
  $inputJson = [Console]::In.ReadToEnd()
  if ($env:CLAUDE_PROJECT_DIR) {
    $workspace = $env:CLAUDE_PROJECT_DIR
  } else {
    $workspace = (Get-Location).Path
  }
  if ($env:AGENT_MONITOR_BIN) {
    $monitor = $env:AGENT_MONITOR_BIN
  } else {
    $monitor = "agent-monitor"
  }

  $arguments = @(
    "ingest",
    "--adapter=claude-code",
    "--workspace=$workspace"
  )
  $output = $inputJson | & $monitor @arguments 2>&1
  $exitCode = $LASTEXITCODE
  $text = ($output | Out-String).Trim()

  if ($exitCode -ne 0 -and $text.Length -gt 0) {
    [Console]::Error.WriteLine($text)
  }
  exit 0
} catch {
  [Console]::Error.WriteLine($_.Exception.Message)
  exit 0
}
"#
    .into()
}

fn pi_injection() -> String {
    format!(
        "{}\
        {}\
        - Act as a bounded helper: focused read-only judgment or drafting unless a monitor packet grants mutation.\n\
        - Handoff context must include durable design memory, recent trace, top failure hypothesis, and unresolved verification.\n",
        shared_injection_body(),
        latest_outbox_instruction("pi")
    )
}

fn opencode_injection() -> String {
    format!(
        "{}\
        {}\
        - Project plugin `.opencode/plugins/agent-monitor.js` is monitor-owned; let it enforce hook-response and normalized events.\n\
        - Keep provider-specific behavior behind adapter boundaries; follow monitor packets in project terms.\n",
        shared_injection_body(),
        latest_outbox_instruction("opencode")
    )
}

fn opencode_hook_plugin() -> String {
    r#"import { spawnSync } from "node:child_process";

export const AgentMonitor = async ({ directory, worktree }) => {
  const monitor = process.env.AGENT_MONITOR_BIN || "agent-monitor";
  const ingestEvent = (event) => {
    const result = spawnSync(
      monitor,
      [
        "ingest",
        "--adapter=opencode",
        `--workspace=${directory}`,
      ],
      {
        cwd: directory,
        input: `${JSON.stringify(event)}\n`,
        encoding: "utf8",
      },
    );
    if (result.status !== 0) {
      const message = result.stderr?.trim() || "agent-monitor ingest unavailable";
      console.error(message);
    }
  };

  return {
    "tool.execute.before": async (input, output) => {
      const args = output?.args ?? {};
      const toolInput = {
        ...args,
        command: args.command,
        path: args.path ?? args.filePath ?? args.file_path,
        file_path: args.file_path ?? args.filePath ?? args.path,
        notebook_path: args.notebook_path ?? args.notebookPath,
      };
      const hookEvent = {
        event: "tool.execute.before",
        tool: input?.tool,
        input: toolInput,
        session: input?.session,
        cwd: directory,
        worktree,
      };
      const result = spawnSync(
        monitor,
        [
          "hook-response",
          "--adapter=opencode",
          `--workspace=${directory}`,
          "--format=opencode",
        ],
        {
          cwd: directory,
          input: JSON.stringify(hookEvent),
          encoding: "utf8",
        },
      );

      if (result.status !== 0) {
        throw new Error(
          result.stderr?.trim() || "agent-monitor hook policy unavailable",
        );
      }
      const response = result.stdout?.trim()
        ? JSON.parse(result.stdout)
        : { action: "allow" };
      if (response.action === "block" || response.decision === "block") {
        throw new Error(
          response.message || response.reason || "blocked by agent-monitor policy",
        );
      }
    },
    "tool.execute.after": async (input, output) => {
      ingestEvent({
        event: "tool.execute.after",
        tool: input?.tool,
        input: output?.args ?? output?.input,
        output: output?.output ?? output?.result,
        exit_code: output?.exit_code ?? output?.exitCode,
        session: input?.session,
        cwd: directory,
        worktree,
      });
    },
    "session.idle": async (input, output) => {
      ingestEvent({
        event: "session.idle",
        session: input?.session,
        content: output?.message ?? output?.content,
        cwd: directory,
        worktree,
      });
    },
    "session.error": async (input, output) => {
      ingestEvent({
        event: "session.error",
        session: input?.session,
        error: output?.error ?? output?.message,
        cwd: directory,
        worktree,
      });
    },
  };
};
"#
    .into()
}

fn is_safe_relative_path(path: &Path) -> bool {
    path.components()
        .all(|component| matches!(component, Component::Normal(_)))
}
