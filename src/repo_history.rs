use crate::{
    RepoHunkHistoryEntry, RepoTraceStatus, StoreError, blame_match_workspace, normalize_blame_path,
    read_all_jsonl,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::Path};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepoHunkHistoryQuery {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    pub limit: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepoHunkHistoryReport {
    pub workspace: String,
    pub entry_count: usize,
    pub file_count: usize,
    pub files: Vec<RepoHunkFileSummary>,
    pub entries: Vec<RepoHunkHistoryEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RepoHunkFileSummary {
    pub path: String,
    pub entry_count: usize,
    pub traced_count: usize,
    pub missing_rationale_count: usize,
    pub untraced_count: usize,
    pub matching_trace_count: usize,
    pub worst_trace_status: RepoTraceStatus,
    pub latest_trace_status: RepoTraceStatus,
    pub latest_history_id: String,
    pub latest_observed_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_modified_at: Option<i64>,
}

#[derive(Default)]
pub(crate) struct RepoHunkFileSummaryAccumulator {
    files: BTreeMap<String, RepoHunkFileSummary>,
}

impl RepoHunkFileSummaryAccumulator {
    pub(crate) fn observe(&mut self, entry: &RepoHunkHistoryEntry) {
        let summary = self
            .files
            .entry(entry.path.clone())
            .or_insert_with(|| new_repo_hunk_file_summary(entry));
        summary.entry_count += 1;
        summary.matching_trace_count += entry.matching_trace_count;
        match entry.trace_status {
            RepoTraceStatus::Traced => summary.traced_count += 1,
            RepoTraceStatus::MissingRationale => summary.missing_rationale_count += 1,
            RepoTraceStatus::Untraced => summary.untraced_count += 1,
        }
        summary.worst_trace_status =
            worst_repo_trace_status(summary.worst_trace_status, entry.trace_status);
        if is_newer_repo_hunk_entry(entry, summary) {
            summary.latest_trace_status = entry.trace_status;
            summary.latest_history_id.clone_from(&entry.history_id);
            summary.latest_observed_at.clone_from(&entry.observed_at);
        }
        summary.latest_modified_at =
            max_optional_i64(summary.latest_modified_at, entry.modified_at);
    }

    pub(crate) fn finish(self) -> Vec<RepoHunkFileSummary> {
        let mut files = self.files.into_values().collect::<Vec<_>>();
        files.sort_by(|left, right| {
            right
                .latest_observed_at
                .cmp(&left.latest_observed_at)
                .then_with(|| left.path.cmp(&right.path))
        });
        files
    }
}

pub fn summarize_repo_hunk_files(entries: &[RepoHunkHistoryEntry]) -> Vec<RepoHunkFileSummary> {
    let mut accumulator = RepoHunkFileSummaryAccumulator::default();
    for entry in entries {
        accumulator.observe(entry);
    }
    accumulator.finish()
}

pub fn load_repo_hunk_history(
    workspace: impl AsRef<Path>,
    query: RepoHunkHistoryQuery,
) -> Result<RepoHunkHistoryReport, StoreError> {
    let workspace = workspace.as_ref();
    let match_workspace = blame_match_workspace(workspace);
    let target_file = query
        .file
        .as_ref()
        .map(|file| normalize_blame_path(&match_workspace, file));
    let mut entries =
        read_all_jsonl::<RepoHunkHistoryEntry>(&workspace.join(".agent-monitor/repo-hunks.jsonl"))?
            .into_iter()
            .filter(|entry| {
                target_file.as_ref().is_none_or(|target| {
                    normalize_blame_path(&match_workspace, &entry.path) == *target
                })
            })
            .filter(|entry| {
                query.line.is_none_or(|line| {
                    hunk_range_contains(entry.new_start, entry.new_lines, line)
                        || hunk_range_contains(entry.old_start, entry.old_lines, line)
                })
            })
            .collect::<Vec<_>>();

    let files = summarize_repo_hunk_files(&entries);
    let file_count = files.len();
    let entry_count = entries.len();
    entries.reverse();
    entries.truncate(query.limit);

    Ok(RepoHunkHistoryReport {
        workspace: workspace.display().to_string(),
        entry_count,
        file_count,
        files,
        entries,
    })
}

fn hunk_range_contains(start: u32, lines: u32, line: u32) -> bool {
    if lines == 0 {
        return false;
    }
    let end = start.saturating_add(lines - 1);
    line >= start && line <= end
}

fn new_repo_hunk_file_summary(entry: &RepoHunkHistoryEntry) -> RepoHunkFileSummary {
    RepoHunkFileSummary {
        path: entry.path.clone(),
        entry_count: 0,
        traced_count: 0,
        missing_rationale_count: 0,
        untraced_count: 0,
        matching_trace_count: 0,
        worst_trace_status: entry.trace_status,
        latest_trace_status: entry.trace_status,
        latest_history_id: entry.history_id.clone(),
        latest_observed_at: entry.observed_at.clone(),
        latest_modified_at: entry.modified_at,
    }
}

fn is_newer_repo_hunk_entry(entry: &RepoHunkHistoryEntry, summary: &RepoHunkFileSummary) -> bool {
    (entry.observed_at.as_str(), entry.history_id.as_str())
        > (
            summary.latest_observed_at.as_str(),
            summary.latest_history_id.as_str(),
        )
}

fn worst_repo_trace_status(left: RepoTraceStatus, right: RepoTraceStatus) -> RepoTraceStatus {
    if repo_trace_status_rank(right) > repo_trace_status_rank(left) {
        right
    } else {
        left
    }
}

fn repo_trace_status_rank(status: RepoTraceStatus) -> u8 {
    match status {
        RepoTraceStatus::Traced => 0,
        RepoTraceStatus::MissingRationale => 1,
        RepoTraceStatus::Untraced => 2,
    }
}

fn max_optional_i64(left: Option<i64>, right: Option<i64>) -> Option<i64> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.max(right)),
        (Some(value), None) | (None, Some(value)) => Some(value),
        (None, None) => None,
    }
}
