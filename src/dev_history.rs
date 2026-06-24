use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DevHistoryAnalysisOptions {
    pub workspace: PathBuf,
    pub codex_sessions_root: Option<PathBuf>,
    pub claude_projects_root: Option<PathBuf>,
    pub top_limit: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DevHistoryRawExportOptions {
    pub workspace: PathBuf,
    pub codex_sessions_root: Option<PathBuf>,
    pub claude_projects_root: Option<PathBuf>,
    pub output_root: PathBuf,
    pub package_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DevHistoryReport {
    pub workspace: String,
    pub generated_at: String,
    pub sources: Vec<DevHistorySourceReport>,
    pub findings: Vec<DevHistoryFinding>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DevHistorySourceReport {
    pub source: String,
    pub history_root: String,
    pub files: usize,
    pub bytes: u64,
    pub lines: u64,
    pub parsed: u64,
    pub sessions: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_time: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_time: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagent_files: Option<usize>,
    pub top_types: Vec<DevHistoryCount>,
    pub top_payload_types: Vec<DevHistoryCount>,
    pub top_content_types: Vec<DevHistoryCount>,
    pub top_tools: Vec<DevHistoryCount>,
    pub top_command_heads: Vec<DevHistoryCount>,
    pub top_signals: Vec<DevHistoryCount>,
    pub top_file_refs: Vec<DevHistoryCount>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DevHistoryFinding {
    pub kind: String,
    pub severity: String,
    pub summary: String,
    pub evidence: Vec<String>,
    pub monitor_response: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DevHistoryCount {
    pub key: String,
    pub count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DevHistoryRawExportReport {
    pub package: String,
    pub workspace: String,
    pub generated_at: String,
    pub package_dir: String,
    pub warning: String,
    pub matching_rules: DevHistoryRawMatchingRules,
    pub included: DevHistoryRawExportIncluded,
    pub excluded: Vec<String>,
    pub copy_errors: Vec<DevHistoryRawCopyError>,
    pub files: Vec<DevHistoryRawExportFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DevHistoryRawMatchingRules {
    pub codex: String,
    pub claude_code: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DevHistoryRawExportIncluded {
    pub codex_sessions_root: Option<String>,
    pub codex_files_matched: usize,
    pub claude_projects_root: Option<String>,
    pub claude_files_matched: usize,
    pub total_files_copied: usize,
    pub total_raw_bytes_copied: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DevHistoryRawExportFile {
    pub source: String,
    pub original_path: String,
    pub package_path: String,
    pub bytes: u64,
    pub digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DevHistoryRawCopyError {
    pub source: String,
    pub original_path: String,
    pub error: String,
}

#[derive(Debug, thiserror::Error)]
pub enum DevHistoryError {
    #[error("read history directory {path}: {source}")]
    ReadDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("read history file {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("read metadata for {path}: {source}")]
    Metadata {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("create history export directory {path}: {source}")]
    CreateDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("history export package already exists: {path}")]
    PackageExists { path: PathBuf },
    #[error(
        "raw history export output root {path} is inside workspace {workspace} but outside .agent-monitor"
    )]
    UnsafeOutputRoot { path: PathBuf, workspace: PathBuf },
    #[error("copy history file {source_path} to {dest_path}: {source}")]
    Copy {
        source_path: PathBuf,
        dest_path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("write history export file {path}: {source}")]
    Write {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("encode history export manifest: {0}")]
    EncodeManifest(serde_json::Error),
}

pub fn analyze_local_dev_history(
    options: DevHistoryAnalysisOptions,
) -> Result<DevHistoryReport, DevHistoryError> {
    let top_limit = options.top_limit.max(1);
    let workspace = normalize_path_text(&options.workspace.display().to_string());
    let mut sources = Vec::new();

    if let Some(root) = options.codex_sessions_root.as_deref() {
        sources.push(analyze_codex_history(root, &workspace, top_limit)?);
    }
    if let Some(root) = options.claude_projects_root.as_deref() {
        sources.push(analyze_claude_history(
            root,
            &options.workspace,
            &workspace,
            top_limit,
        )?);
    }

    let findings = dev_history_findings(&sources);
    Ok(DevHistoryReport {
        workspace,
        generated_at: crate::current_utc_timestamp()
            .unwrap_or_else(|| "1970-01-01T00:00:00Z".into()),
        sources,
        findings,
    })
}

pub fn export_raw_dev_history(
    options: DevHistoryRawExportOptions,
) -> Result<DevHistoryRawExportReport, DevHistoryError> {
    let workspace = normalize_path_text(&options.workspace.display().to_string());
    let generated_at =
        crate::current_utc_timestamp().unwrap_or_else(|| "1970-01-01T00:00:00Z".into());
    let package = raw_export_package_name(&options.workspace, options.package_name.as_deref());
    let package_dir = options.output_root.join(&package);
    ensure_raw_export_output_root_allowed(&options.workspace, &options.output_root)?;
    fs::create_dir_all(&options.output_root).map_err(|source| DevHistoryError::CreateDir {
        path: options.output_root.clone(),
        source,
    })?;
    match fs::create_dir(&package_dir) {
        Ok(()) => {}
        Err(source) if source.kind() == std::io::ErrorKind::AlreadyExists => {
            return Err(DevHistoryError::PackageExists { path: package_dir });
        }
        Err(source) => {
            return Err(DevHistoryError::CreateDir {
                path: package_dir,
                source,
            });
        }
    }

    let mut report = DevHistoryRawExportReport {
        package,
        workspace,
        generated_at,
        package_dir: package_dir.display().to_string(),
        warning: "RAW CHAT TRANSCRIPTS: may contain prompts, model outputs, tool payloads, file paths, pasted secrets, credentials, or other sensitive project/user data. Handle this export as private.".into(),
        matching_rules: DevHistoryRawMatchingRules {
            codex: "JSONL files under the Codex sessions root whose payload.cwd or payload.workspace_roots exactly normalize to the workspace.".into(),
            claude_code: "All JSONL files under the encoded Claude Code project directory for the workspace, including subagents.".into(),
        },
        included: DevHistoryRawExportIncluded {
            codex_sessions_root: options
                .codex_sessions_root
                .as_ref()
                .map(|path| path.display().to_string()),
            codex_files_matched: 0,
            claude_projects_root: options
                .claude_projects_root
                .as_ref()
                .map(|path| path.display().to_string()),
            claude_files_matched: 0,
            total_files_copied: 0,
            total_raw_bytes_copied: 0,
        },
        excluded: vec![
            "Codex auth/config files".into(),
            "Claude auth/config files".into(),
            "Claude project directories outside the matched workspace project".into(),
            "Project source files outside local agent transcript history".into(),
        ],
        copy_errors: Vec::new(),
        files: Vec::new(),
    };

    if let Some(root) = options.codex_sessions_root.as_deref() {
        for path in jsonl_files(root)? {
            if !codex_file_matches_workspace(&path, &report.workspace)? {
                continue;
            }
            report.included.codex_files_matched += 1;
            record_raw_export_copy(
                &mut report,
                "codex",
                root,
                &path,
                "raw/codex-sessions",
                &package_dir,
            );
        }
    }

    if let Some(projects_root) = options.claude_projects_root.as_deref() {
        for project_root in claude_project_roots(projects_root, &options.workspace)? {
            let project_name = project_root
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_string)
                .unwrap_or_else(|| "claude-project".into());
            let package_prefix = format!(
                "raw/claude-code-projects/{}",
                crate::safe_slug(&project_name)
            );
            for path in jsonl_files(&project_root)? {
                report.included.claude_files_matched += 1;
                record_raw_export_copy(
                    &mut report,
                    "claude-code",
                    &project_root,
                    &path,
                    &package_prefix,
                    &package_dir,
                );
            }
        }
    }

    write_raw_export_readme(&package_dir)?;
    write_raw_export_manifest(&package_dir, &report)?;
    Ok(report)
}

fn analyze_codex_history(
    root: &Path,
    workspace: &str,
    top_limit: usize,
) -> Result<DevHistorySourceReport, DevHistoryError> {
    let mut stats = SourceStats::new("codex", root);
    for path in jsonl_files(root)? {
        if !codex_file_matches_workspace(&path, workspace)? {
            continue;
        }
        scan_file(&path, &mut stats, |value, stats| {
            scan_codex_value(value, stats, workspace);
        })?;
    }
    Ok(stats.into_report(top_limit))
}

fn record_raw_export_copy(
    report: &mut DevHistoryRawExportReport,
    source: &str,
    source_root: &Path,
    source_path: &Path,
    package_prefix: &str,
    package_dir: &Path,
) {
    match copy_raw_history_file(
        source,
        source_root,
        source_path,
        package_prefix,
        package_dir,
    ) {
        Ok(file) => {
            report.included.total_files_copied += 1;
            report.included.total_raw_bytes_copied = report
                .included
                .total_raw_bytes_copied
                .saturating_add(file.bytes);
            report.files.push(file);
        }
        Err(error) => report.copy_errors.push(DevHistoryRawCopyError {
            source: source.into(),
            original_path: source_path.display().to_string(),
            error: error.to_string(),
        }),
    }
}

fn copy_raw_history_file(
    source: &str,
    source_root: &Path,
    source_path: &Path,
    package_prefix: &str,
    package_dir: &Path,
) -> Result<DevHistoryRawExportFile, DevHistoryError> {
    let package_path = raw_export_package_path(source_root, source_path, package_prefix);
    let dest_path = package_dir.join(package_path.replace('/', std::path::MAIN_SEPARATOR_STR));
    if let Some(parent) = dest_path.parent() {
        fs::create_dir_all(parent).map_err(|source| DevHistoryError::CreateDir {
            path: parent.to_path_buf(),
            source,
        })?;
    }
    fs::copy(source_path, &dest_path).map_err(|source| DevHistoryError::Copy {
        source_path: source_path.to_path_buf(),
        dest_path: dest_path.clone(),
        source,
    })?;
    let bytes = fs::read(&dest_path).map_err(|source| DevHistoryError::Read {
        path: dest_path.clone(),
        source,
    })?;
    Ok(DevHistoryRawExportFile {
        source: source.into(),
        original_path: source_path.display().to_string(),
        package_path,
        bytes: bytes.len() as u64,
        digest: crate::fnv1a64_digest(&bytes),
    })
}

fn raw_export_package_path(source_root: &Path, source_path: &Path, package_prefix: &str) -> String {
    let relative = source_path
        .strip_prefix(source_root)
        .unwrap_or(source_path)
        .iter()
        .filter_map(|component| component.to_str())
        .collect::<Vec<_>>()
        .join("/");
    if relative.is_empty() {
        package_prefix.into()
    } else {
        format!("{package_prefix}/{relative}")
    }
}

fn raw_export_package_name(workspace: &Path, requested: Option<&str>) -> String {
    if let Some(requested) = requested {
        let package = crate::safe_slug(requested);
        if package != "item" {
            return package;
        }
    }
    let workspace_name = workspace
        .file_name()
        .and_then(|name| name.to_str())
        .map(crate::safe_slug)
        .unwrap_or_else(|| "workspace".into());
    let generated_at = crate::current_utc_timestamp()
        .map(|timestamp| crate::safe_slug(&timestamp))
        .unwrap_or_else(|| "1970-01-01T00-00-00Z".into());
    format!("{workspace_name}-raw-transcripts-{generated_at}")
}

fn write_raw_export_manifest(
    package_dir: &Path,
    report: &DevHistoryRawExportReport,
) -> Result<(), DevHistoryError> {
    let path = package_dir.join("manifest.json");
    let bytes = serde_json::to_vec_pretty(report).map_err(DevHistoryError::EncodeManifest)?;
    fs::write(&path, bytes).map_err(|source| DevHistoryError::Write { path, source })
}

fn write_raw_export_readme(package_dir: &Path) -> Result<(), DevHistoryError> {
    let path = package_dir.join("README.md");
    let text = "# Raw Transcript Package\n\nThis package intentionally contains raw Codex and Claude Code transcript JSONL files for the selected workspace.\n\nWARNING: raw transcripts may contain prompts, model outputs, tool payloads, file paths, pasted secrets, credentials, or other sensitive project/user data. Treat this package as private.\n\nIncluded transcript files are selected from Codex session metadata and the encoded Claude Code project directory. Codex/Claude auth and runtime credential files are not copied.\n";
    fs::write(&path, text).map_err(|source| DevHistoryError::Write { path, source })
}

fn ensure_raw_export_output_root_allowed(
    workspace: &Path,
    output_root: &Path,
) -> Result<(), DevHistoryError> {
    let workspace_abs = absolute_policy_path(workspace);
    let output_abs = absolute_policy_path(output_root);
    let monitor_abs = absolute_policy_path(&workspace.join(".agent-monitor"));

    let workspace_norm = normalize_path_text(&workspace_abs.display().to_string());
    let output_norm = normalize_path_text(&output_abs.display().to_string());
    let monitor_norm = normalize_path_text(&monitor_abs.display().to_string());

    if path_text_is_same_or_child(&output_norm, &workspace_norm)
        && !path_text_is_same_or_child(&output_norm, &monitor_norm)
    {
        return Err(DevHistoryError::UnsafeOutputRoot {
            path: output_root.to_path_buf(),
            workspace: workspace.to_path_buf(),
        });
    }

    Ok(())
}

fn absolute_policy_path(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|current| current.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    }
}

fn path_text_is_same_or_child(candidate: &str, parent: &str) -> bool {
    candidate == parent
        || candidate
            .strip_prefix(parent)
            .is_some_and(|rest| rest.starts_with('/'))
}

fn analyze_claude_history(
    projects_root: &Path,
    workspace_path: &Path,
    workspace: &str,
    top_limit: usize,
) -> Result<DevHistorySourceReport, DevHistoryError> {
    let mut stats = SourceStats::new("claude-code", projects_root);
    for project_root in claude_project_roots(projects_root, workspace_path)? {
        for path in jsonl_files(&project_root)? {
            if normalize_path_text(&path.display().to_string()).contains("/subagents/") {
                stats.subagent_files += 1;
            }
            scan_file(&path, &mut stats, |value, stats| {
                scan_claude_value(value, stats, workspace);
            })?;
        }
    }
    Ok(stats.into_report(top_limit))
}

fn jsonl_files(root: &Path) -> Result<Vec<PathBuf>, DevHistoryError> {
    let mut out = Vec::new();
    if !root.exists() {
        return Ok(out);
    }
    collect_jsonl_files(root, &mut out)?;
    out.sort();
    Ok(out)
}

fn collect_jsonl_files(root: &Path, out: &mut Vec<PathBuf>) -> Result<(), DevHistoryError> {
    for entry in fs::read_dir(root).map_err(|source| DevHistoryError::ReadDir {
        path: root.to_path_buf(),
        source,
    })? {
        let entry = entry.map_err(|source| DevHistoryError::ReadDir {
            path: root.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|source| DevHistoryError::Metadata {
                path: path.clone(),
                source,
            })?;
        if file_type.is_dir() {
            collect_jsonl_files(&path, out)?;
        } else if file_type.is_file()
            && path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("jsonl"))
        {
            out.push(path);
        }
    }
    Ok(())
}

fn scan_file<F>(
    path: &Path,
    stats: &mut SourceStats,
    mut scan_value: F,
) -> Result<(), DevHistoryError>
where
    F: FnMut(&Value, &mut SourceStats),
{
    let metadata = fs::metadata(path).map_err(|source| DevHistoryError::Metadata {
        path: path.to_path_buf(),
        source,
    })?;
    stats.files += 1;
    stats.bytes = stats.bytes.saturating_add(metadata.len());

    let file = File::open(path).map_err(|source| DevHistoryError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    for line in BufReader::new(file).lines() {
        let line = line.map_err(|source| DevHistoryError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        stats.lines = stats.lines.saturating_add(1);
        if let Ok(value) = serde_json::from_str::<Value>(&line) {
            stats.parsed = stats.parsed.saturating_add(1);
            scan_value(&value, stats);
        }
    }
    Ok(())
}

fn codex_file_matches_workspace(path: &Path, workspace: &str) -> Result<bool, DevHistoryError> {
    let file = File::open(path).map_err(|source| DevHistoryError::Read {
        path: path.to_path_buf(),
        source,
    })?;
    for line in BufReader::new(file).lines() {
        let line = line.map_err(|source| DevHistoryError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let Some(payload) = value.get("payload") else {
            continue;
        };
        if payload
            .get("cwd")
            .and_then(Value::as_str)
            .is_some_and(|cwd| normalize_path_text(cwd) == workspace)
        {
            return Ok(true);
        }
        if payload
            .get("workspace_roots")
            .and_then(Value::as_array)
            .is_some_and(|roots| {
                roots.iter().any(|root| {
                    if let Some(path) = root.as_str() {
                        return normalize_path_text(path) == workspace;
                    }
                    ["path", "root", "cwd"].iter().any(|key| {
                        root.get(key)
                            .and_then(Value::as_str)
                            .is_some_and(|path| normalize_path_text(path) == workspace)
                    })
                })
            })
        {
            return Ok(true);
        }
    }
    Ok(false)
}

fn claude_project_roots(
    projects_root: &Path,
    workspace_path: &Path,
) -> Result<Vec<PathBuf>, DevHistoryError> {
    if !projects_root.exists() {
        return Ok(Vec::new());
    }

    let expected = claude_project_dir_name(workspace_path);
    let direct = projects_root.join(&expected);
    if direct.is_dir() {
        return Ok(vec![direct]);
    }

    let workspace_tail = workspace_path
        .file_name()
        .and_then(|value| value.to_str())
        .map(project_slug)
        .unwrap_or_default();
    let mut roots = Vec::new();
    for entry in fs::read_dir(projects_root).map_err(|source| DevHistoryError::ReadDir {
        path: projects_root.to_path_buf(),
        source,
    })? {
        let entry = entry.map_err(|source| DevHistoryError::ReadDir {
            path: projects_root.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|source| DevHistoryError::Metadata {
                path: path.clone(),
                source,
            })?;
        if !file_type.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
        if name == expected || (!workspace_tail.is_empty() && name.ends_with(&workspace_tail)) {
            roots.push(path);
        }
    }
    roots.sort();
    Ok(roots)
}

fn claude_project_dir_name(workspace_path: &Path) -> String {
    let normalized = normalize_path_text(&workspace_path.display().to_string());
    let mut parts = normalized.split('/').filter(|part| !part.is_empty());
    let Some(first) = parts.next() else {
        return String::new();
    };
    let first = first.strip_suffix(':').unwrap_or(first);
    let rest = parts.map(project_slug).collect::<Vec<_>>();
    if rest.is_empty() {
        project_slug(first)
    } else {
        format!("{}--{}", project_slug(first), rest.join("-"))
    }
}

fn scan_codex_value(value: &Value, stats: &mut SourceStats, workspace: &str) {
    scan_common_top_level(value, stats);
    let Some(payload) = value.get("payload") else {
        return;
    };
    for key in ["id", "session_id", "sessionId"] {
        if let Some(session) = payload.get(key).and_then(Value::as_str) {
            stats.sessions.insert(session.to_string());
        }
    }
    if let Some(payload_type) = payload.get("type").and_then(Value::as_str) {
        stats.inc_payload_type(payload_type);
    }
    for key in ["message", "text", "output"] {
        if let Some(text) = payload.get(key).and_then(Value::as_str) {
            scan_text(stats, text, codex_text_source_for_key(key), workspace);
        }
    }
    if let Some(content) = payload.get("content").and_then(Value::as_array) {
        for item in content {
            if let Some(text) = item.as_str() {
                scan_text(stats, text, "codex", workspace);
            } else if let Some(object) = item.as_object() {
                if let Some(content_type) = object.get("type").and_then(Value::as_str) {
                    stats.inc_content_type(content_type);
                }
                if let Some(text) = object.get("text").and_then(Value::as_str) {
                    scan_text(stats, text, "codex", workspace);
                }
            }
        }
    }
    if let Some(name) = payload.get("name").and_then(Value::as_str) {
        stats.inc_tool(&format!("codex:{name}"));
    }
    if let Some(arguments) = payload.get("arguments").and_then(Value::as_str) {
        scan_codex_arguments(arguments, stats, workspace);
    }
}

fn codex_text_source_for_key(key: &str) -> &'static str {
    match key {
        "output" => "codex-output",
        _ => "codex",
    }
}

fn scan_codex_arguments(arguments: &str, stats: &mut SourceStats, workspace: &str) {
    let Ok(value) = serde_json::from_str::<Value>(arguments) else {
        scan_text(stats, arguments, "codex-arguments", workspace);
        return;
    };
    for key in ["command", "cmd"] {
        if let Some(command) = command_text(value.get(key)) {
            stats.inc_command_head(&command_head(&command));
            scan_text(stats, &command, "codex-command", workspace);
        }
    }
    for key in ["path", "file_path", "workdir"] {
        if let Some(path) = value.get(key).and_then(Value::as_str) {
            add_path_ref(stats, path, workspace);
        }
    }
}

fn command_text(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(command) => Some(command.clone()),
        Value::Array(parts) => Some(
            parts
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(" "),
        ),
        _ => None,
    }
}

fn scan_claude_value(value: &Value, stats: &mut SourceStats, workspace: &str) {
    scan_common_top_level(value, stats);
    for key in ["lastPrompt", "aiTitle", "content"] {
        if let Some(text) = value.get(key).and_then(Value::as_str) {
            scan_text(stats, text, "claude", workspace);
        }
    }
    let Some(message) = value.get("message") else {
        return;
    };
    if let Some(text) = message.as_str() {
        scan_text(stats, text, "claude", workspace);
        return;
    }
    if let Some(text) = message.get("content").and_then(Value::as_str) {
        scan_text(stats, text, "claude", workspace);
    }
    if let Some(content) = message.get("content").and_then(Value::as_array) {
        for item in content {
            scan_claude_content_item(item, stats, workspace);
        }
    }
}

fn scan_claude_content_item(item: &Value, stats: &mut SourceStats, workspace: &str) {
    if let Some(content_type) = item.get("type").and_then(Value::as_str) {
        stats.inc_content_type(content_type);
    }
    if let Some(text) = item.get("text").and_then(Value::as_str) {
        scan_text(stats, text, "claude", workspace);
    }
    if item.get("type").and_then(Value::as_str) == Some("tool_use") {
        if let Some(name) = item.get("name").and_then(Value::as_str) {
            stats.inc_tool(&format!("claude:{name}"));
        }
        if let Some(input) = item.get("input") {
            if let Some(command) = input.get("command").and_then(Value::as_str) {
                stats.inc_command_head(&command_head(command));
                scan_text(stats, command, "claude-command", workspace);
            }
            for key in ["file_path", "path", "notebook_path"] {
                if let Some(path) = input.get(key).and_then(Value::as_str) {
                    add_path_ref(stats, path, workspace);
                }
            }
        }
    }
    if item.get("type").and_then(Value::as_str) == Some("tool_result") {
        if let Some(content) = item.get("content").and_then(Value::as_str) {
            scan_text(stats, content, "claude-tool-result", workspace);
        }
        if let Some(parts) = item.get("content").and_then(Value::as_array) {
            for part in parts {
                if let Some(text) = part.get("text").and_then(Value::as_str) {
                    scan_text(stats, text, "claude-tool-result", workspace);
                }
            }
        }
    }
}

fn scan_common_top_level(value: &Value, stats: &mut SourceStats) {
    if let Some(kind) = value.get("type").and_then(Value::as_str) {
        stats.inc_type(kind);
    }
    if let Some(session) = value.get("sessionId").and_then(Value::as_str) {
        stats.sessions.insert(session.to_string());
    }
    if let Some(timestamp) = value.get("timestamp").and_then(Value::as_str) {
        stats.note_timestamp(timestamp);
    }
}

fn scan_text(stats: &mut SourceStats, text: &str, source: &str, workspace: &str) {
    let lower = text.to_ascii_lowercase();
    if contains_any(
        &lower,
        &[
            "should i continue",
            "would you like me to",
            "do you want me to",
            "shall i",
            "需要我",
            "要不要",
            "是否继续",
            "继续吗",
        ],
    ) {
        stats.inc_signal(&format!("{source}:agent-question"));
    }
    if contains_any(
        &lower,
        &[
            "good point to stop",
            "stop here",
            "pause here",
            "remaining work",
            "left to do",
            "not run",
            "did not run",
            "haven't run",
            "未运行",
            "没有运行",
        ],
    ) {
        stats.inc_signal(&format!("{source}:premature-stop-or-unverified"));
    }
    if contains_any(
        &lower,
        &[
            "rate limit",
            "429",
            "timeout",
            "timed out",
            "econnreset",
            "network",
            "service unavailable",
            "overloaded",
            "connection",
            "context limit",
            "context window",
            "token limit",
            "上下文",
        ],
    ) || contains_5xx(&lower)
    {
        stats.inc_signal(&format!("{source}:service-or-context-instability"));
    }
    if contains_any(
        &lower,
        &[
            "failed",
            "error",
            "exception",
            "permission denied",
            "denied",
            "cannot",
            "can't",
            "blocked",
            "失败",
            "错误",
            "拒绝",
        ],
    ) {
        stats.inc_signal(&format!("{source}:failure-language"));
    }
    if contains_any(
        &lower,
        &[
            "test",
            "tests",
            "pytest",
            "cargo test",
            "npm run test",
            "pnpm test",
            "mvn test",
            "gradle test",
            "vitest",
            "playwright",
            "build",
            "lint",
            "typecheck",
            "测试",
            "构建",
            "验证",
        ],
    ) {
        stats.inc_signal(&format!("{source}:verification-language"));
    }
    for path in extract_path_refs(text, workspace) {
        stats.inc_file_ref(&path);
    }
}

fn contains_any(text: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| text.contains(needle))
}

fn contains_5xx(text: &str) -> bool {
    ["500", "502", "503", "504", "529"]
        .iter()
        .any(|status| text.contains(status))
}

fn extract_path_refs(text: &str, workspace: &str) -> Vec<String> {
    let normalized = text.replace('\\', "/");
    normalized
        .split(|ch: char| {
            ch.is_whitespace()
                || matches!(
                    ch,
                    '`' | '\'' | '"' | '(' | ')' | '[' | ']' | '{' | '}' | ',' | ';' | '<' | '>'
                )
        })
        .filter_map(|token| clean_path_token(token, workspace))
        .take(20)
        .collect()
}

fn clean_path_token(token: &str, workspace: &str) -> Option<String> {
    let token = token.trim_matches(|ch: char| matches!(ch, '.' | ':' | '!' | '?' | '#'));
    if token.is_empty() {
        return None;
    }
    let lower = token.to_ascii_lowercase();
    let relative_prefixes = [
        "frontend/",
        "backend/",
        "docs/",
        "scripts/",
        "deploy/",
        "docker/",
        "e2e/",
        "tools/",
        "data/",
        "runtime/",
        "ref_code/",
        ".agent-monitor/",
        ".claude/",
        ".pi/",
    ];
    if normalize_path_text(token).starts_with(workspace)
        || relative_prefixes
            .iter()
            .any(|prefix| lower.starts_with(prefix))
    {
        Some(
            token
                .trim_matches(|ch: char| matches!(ch, '.' | ',' | ';' | ':'))
                .to_string(),
        )
    } else {
        None
    }
}

fn add_path_ref(stats: &mut SourceStats, path: &str, workspace: &str) {
    if let Some(path) = clean_path_token(&path.replace('\\', "/"), workspace) {
        stats.inc_file_ref(&path);
    }
}

fn command_head(command: &str) -> String {
    let command = command
        .trim()
        .trim_matches(|ch| matches!(ch, '`' | '\'' | '"'));
    if command.is_empty() {
        "<empty>".into()
    } else {
        command
            .split_whitespace()
            .next()
            .unwrap_or("<empty>")
            .trim_start_matches(".\\")
            .trim_start_matches("./")
            .to_ascii_lowercase()
    }
}

fn dev_history_findings(sources: &[DevHistorySourceReport]) -> Vec<DevHistoryFinding> {
    let total_files = sources.iter().map(|source| source.files).sum::<usize>();
    let mut findings = Vec::new();
    if total_files > 0 {
        findings.push(DevHistoryFinding {
            kind: "external_history_present".into(),
            severity: "info".into(),
            summary:
                "Local Codex/Claude histories contain project evidence outside monitor storage."
                    .into(),
            evidence: vec![format!("{total_files} local history files matched the workspace")],
            monitor_response: vec![
                "Import safe aggregate history before handoff or blame analysis.".into(),
                "Keep raw transcript text out of packets unless a bounded excerpt is explicitly needed."
                    .into(),
            ],
        });
    }

    let verification = signal_total(sources, "verification-language")
        + signal_total(sources, "premature-stop-or-unverified");
    if verification > 0 {
        findings.push(DevHistoryFinding {
            kind: "verification_entropy".into(),
            severity: if verification > 100 {
                "critical".into()
            } else {
                "warning".into()
            },
            summary:
                "History shows verification-heavy work with stale or unverified completion risk."
                    .into(),
            evidence: vec![format!("{verification} verification or unverified-stop signals")],
            monitor_response: vec![
                "Track verifier freshness against the latest write before allowing continue.".into(),
                "Emit force-verification packets when code changed after the last relevant verifier."
                    .into(),
            ],
        });
    }

    let interrupts = signal_total(sources, "agent-question");
    if interrupts > 0 {
        findings.push(DevHistoryFinding {
            kind: "user_interrupt_entropy".into(),
            severity: "warning".into(),
            summary: "History includes agent questions that may be avoidable with local evidence."
                .into(),
            evidence: vec![format!("{interrupts} agent-question signals")],
            monitor_response: vec![
                "Gate AskUser behind deterministic probes and a value-of-information check.".into(),
                "Prefer bounded follow-up packets when logs, diffs, or verifiers can resolve the uncertainty."
                    .into(),
            ],
        });
    }

    let instability = signal_total(sources, "service-or-context-instability");
    if instability > 0 {
        findings.push(DevHistoryFinding {
            kind: "agent_health_entropy".into(),
            severity: if instability > 100 {
                "critical".into()
            } else {
                "warning".into()
            },
            summary: "History contains service, timeout, connection, or context-instability signals."
                .into(),
            evidence: vec![format!("{instability} instability signals")],
            monitor_response: vec![
                "Classify provider/tool failures separately from task difficulty.".into(),
                "Retry transient failures before switching agents; compact and hand off only when loop signatures persist."
                    .into(),
            ],
        });
    }

    let codex_spawns = tool_total(sources, "codex:spawn_agent");
    let codex_closes = tool_total(sources, "codex:close_agent");
    let codex_waits = tool_total(sources, "codex:wait_agent");
    let codex_open_workers = codex_spawns.saturating_sub(codex_closes + codex_waits);
    let claude_subagent_files = subagent_file_total(sources);
    if codex_open_workers > 0 || claude_subagent_files > 0 {
        let mut evidence = Vec::new();
        if codex_spawns > 0 || codex_closes > 0 || codex_waits > 0 {
            evidence.push(format!(
                "Codex lifecycle tool counts: spawn_agent={codex_spawns}, close_agent={codex_closes}, wait_agent={codex_waits}"
            ));
        }
        if claude_subagent_files > 0 {
            evidence.push(format!(
                "Claude Code subagent transcript files: {claude_subagent_files}"
            ));
        }
        findings.push(DevHistoryFinding {
            kind: "subagent_lifecycle_entropy".into(),
            severity: if codex_open_workers > 20 || claude_subagent_files > 100 {
                "critical".into()
            } else {
                "warning".into()
            },
            summary:
                "History shows subagent fan-out without enough observable join or integration outcomes."
                    .into(),
            evidence,
            monitor_response: vec![
                "Require every spawned worker to end as joined_with_summary, cancelled_with_reason, timed_out, superseded, or failed before completion.".into(),
                "Block overlapping subagent fan-out until worker paths, result schemas, and integration status are known.".into(),
            ],
        });
    }

    let hotspots = sources
        .iter()
        .flat_map(|source| source.top_file_refs.iter().take(3))
        .map(|item| format!("{} ({})", item.key, item.count))
        .collect::<Vec<_>>();
    if !hotspots.is_empty() {
        findings.push(DevHistoryFinding {
            kind: "blame_hotspots".into(),
            severity: "info".into(),
            summary: "History identifies files that should become first-class blame and rationale targets."
                .into(),
            evidence: hotspots,
            monitor_response: vec![
                "Link imported history signals to repo hunk history and design-memory evidence ids.".into(),
                "Prefer file-scoped follow-up packets for repeatedly touched hotspots.".into(),
            ],
        });
    }

    findings
}

fn signal_total(sources: &[DevHistorySourceReport], suffix: &str) -> u64 {
    sources
        .iter()
        .flat_map(|source| source.top_signals.iter())
        .filter(|item| item.key.ends_with(suffix))
        .map(|item| item.count)
        .sum()
}

fn tool_total(sources: &[DevHistorySourceReport], key: &str) -> u64 {
    sources
        .iter()
        .flat_map(|source| source.top_tools.iter())
        .filter(|item| item.key == key)
        .map(|item| item.count)
        .sum()
}

fn subagent_file_total(sources: &[DevHistorySourceReport]) -> usize {
    sources
        .iter()
        .filter_map(|source| source.subagent_files)
        .sum()
}

#[derive(Debug)]
struct SourceStats {
    source: String,
    history_root: String,
    files: usize,
    bytes: u64,
    lines: u64,
    parsed: u64,
    subagent_files: usize,
    first_time: Option<String>,
    last_time: Option<String>,
    sessions: BTreeSet<String>,
    types: BTreeMap<String, u64>,
    payload_types: BTreeMap<String, u64>,
    content_types: BTreeMap<String, u64>,
    tools: BTreeMap<String, u64>,
    command_heads: BTreeMap<String, u64>,
    signals: BTreeMap<String, u64>,
    file_refs: BTreeMap<String, u64>,
}

impl SourceStats {
    fn new(source: impl Into<String>, history_root: &Path) -> Self {
        Self {
            source: source.into(),
            history_root: history_root.display().to_string(),
            files: 0,
            bytes: 0,
            lines: 0,
            parsed: 0,
            subagent_files: 0,
            first_time: None,
            last_time: None,
            sessions: BTreeSet::new(),
            types: BTreeMap::new(),
            payload_types: BTreeMap::new(),
            content_types: BTreeMap::new(),
            tools: BTreeMap::new(),
            command_heads: BTreeMap::new(),
            signals: BTreeMap::new(),
            file_refs: BTreeMap::new(),
        }
    }

    fn inc_type(&mut self, key: &str) {
        increment(&mut self.types, key);
    }

    fn inc_payload_type(&mut self, key: &str) {
        increment(&mut self.payload_types, key);
    }

    fn inc_content_type(&mut self, key: &str) {
        increment(&mut self.content_types, key);
    }

    fn inc_tool(&mut self, key: &str) {
        increment(&mut self.tools, key);
    }

    fn inc_command_head(&mut self, key: &str) {
        increment(&mut self.command_heads, key);
    }

    fn inc_signal(&mut self, key: &str) {
        increment(&mut self.signals, key);
    }

    fn inc_file_ref(&mut self, key: &str) {
        increment(&mut self.file_refs, key);
    }

    fn note_timestamp(&mut self, timestamp: &str) {
        if self
            .first_time
            .as_ref()
            .is_none_or(|current| timestamp < current.as_str())
        {
            self.first_time = Some(timestamp.to_string());
        }
        if self
            .last_time
            .as_ref()
            .is_none_or(|current| timestamp > current.as_str())
        {
            self.last_time = Some(timestamp.to_string());
        }
    }

    fn into_report(self, top_limit: usize) -> DevHistorySourceReport {
        DevHistorySourceReport {
            source: self.source,
            history_root: self.history_root,
            files: self.files,
            bytes: self.bytes,
            lines: self.lines,
            parsed: self.parsed,
            sessions: self.sessions.len(),
            first_time: self.first_time,
            last_time: self.last_time,
            subagent_files: if self.subagent_files > 0 {
                Some(self.subagent_files)
            } else {
                None
            },
            top_types: top_counts(&self.types, top_limit),
            top_payload_types: top_counts(&self.payload_types, top_limit),
            top_content_types: top_counts(&self.content_types, top_limit),
            top_tools: top_counts(&self.tools, top_limit),
            top_command_heads: top_counts(&self.command_heads, top_limit),
            top_signals: top_counts(&self.signals, top_limit.max(self.signals.len())),
            top_file_refs: top_counts(&self.file_refs, top_limit),
        }
    }
}

fn increment(counts: &mut BTreeMap<String, u64>, key: &str) {
    *counts.entry(key.to_string()).or_default() += 1;
}

fn top_counts(counts: &BTreeMap<String, u64>, limit: usize) -> Vec<DevHistoryCount> {
    let mut values = counts
        .iter()
        .map(|(key, count)| DevHistoryCount {
            key: key.clone(),
            count: *count,
        })
        .collect::<Vec<_>>();
    values.sort_by(|left, right| {
        right
            .count
            .cmp(&left.count)
            .then_with(|| left.key.cmp(&right.key))
    });
    values.truncate(limit);
    values
}

fn normalize_path_text(value: &str) -> String {
    value.replace('\\', "/").to_ascii_lowercase()
}

fn project_slug(value: &str) -> String {
    let mut output = String::new();
    let mut last_dash = false;
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            output.push(ch.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            output.push('-');
            last_dash = true;
        }
    }
    output.trim_matches('-').to_string()
}
