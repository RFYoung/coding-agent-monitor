//! Repo blame and change-audit plumbing: git status/diff parsing and hunk
//! trace attribution that answers "why is this change here, and who added it?".

use crate::*;

pub fn load_blame_report(
    workspace: impl AsRef<Path>,
    query: BlameQuery,
) -> Result<BlameReport, StoreError> {
    let workspace = workspace.as_ref();
    let store_root = workspace.join(".agent-monitor");
    let traces = read_all_jsonl::<TraceEntry>(&store_root.join("trace.jsonl"))?;
    let match_workspace = blame_match_workspace(workspace);
    let target_file = normalize_blame_path(&match_workspace, &query.file);
    let mut matches = traces
        .iter()
        .cloned()
        .enumerate()
        .filter_map(|(sequence, trace)| {
            if normalize_blame_path(&match_workspace, &trace.file) != target_file {
                return None;
            }
            let match_kind = match (query.line, trace.line) {
                (Some(line), Some(_)) if trace_line_range_contains(&trace, line) => {
                    BlameMatchKind::ExactLine
                }
                (Some(_), Some(_)) => return None,
                (Some(_), None) | (None, _) => BlameMatchKind::File,
            };
            Some((sequence, BlameMatch { match_kind, trace }))
        })
        .collect::<Vec<_>>();

    matches.sort_by(|(left_sequence, left), (right_sequence, right)| {
        blame_match_rank(left.match_kind)
            .cmp(&blame_match_rank(right.match_kind))
            .then_with(|| right_sequence.cmp(left_sequence))
    });
    let has_matches = !matches.is_empty();
    let matches = matches
        .into_iter()
        .take(query.limit)
        .map(|(_, entry)| entry)
        .collect::<Vec<_>>();
    let status = if has_matches {
        BlameStatus::Traced
    } else {
        BlameStatus::Untraced
    };

    Ok(BlameReport {
        workspace: workspace.display().to_string(),
        file: query.file,
        line: query.line,
        status,
        trace_count: traces.len(),
        matches,
    })
}

pub fn load_repo_audit(workspace: impl AsRef<Path>) -> Result<RepoAuditReport, RepoAuditError> {
    let workspace = workspace.as_ref();
    let traces = read_all_jsonl::<TraceEntry>(&workspace.join(".agent-monitor/trace.jsonl"))?;
    let changes = git_changed_files(workspace)?;
    let match_workspace = blame_match_workspace(workspace);
    let mut audits = Vec::new();

    for (path, kind) in changes {
        if repo_audit_path_is_ignored(&path) {
            continue;
        }
        let mut hunks = match kind {
            RepoChangeKind::Untracked => Vec::new(),
            RepoChangeKind::Modified | RepoChangeKind::Added | RepoChangeKind::Deleted => {
                git_diff_hunks(workspace, &path)?
            }
        };
        let modified_at = repo_change_fresh_after_seconds(workspace, &path, kind);
        let all_matching_traces =
            traces_for_repo_change(&traces, &match_workspace, &path, &hunks, modified_at);
        annotate_repo_diff_hunks(&all_matching_traces, &mut hunks);
        let trace_status = trace_status_for_hunks(&all_matching_traces, &hunks);
        let matching_traces = bound_repo_audit_traces(all_matching_traces);
        audits.push(RepoChangeAudit {
            path,
            kind,
            trace_status,
            modified_at,
            hunks,
            matching_traces,
        });
    }

    audits.sort_by(|left, right| left.path.cmp(&right.path));
    let untraced_count = audits
        .iter()
        .filter(|change| change.trace_status == RepoTraceStatus::Untraced)
        .count();
    let unexplained_count = audits
        .iter()
        .filter(|change| change.trace_status == RepoTraceStatus::MissingRationale)
        .count();
    let status = if untraced_count == 0 && unexplained_count == 0 {
        RepoAuditStatus::Clean
    } else {
        RepoAuditStatus::Warning
    };

    Ok(RepoAuditReport {
        workspace: workspace.display().to_string(),
        status,
        changes: audits,
        untraced_count,
        unexplained_count,
    })
}

pub fn record_repo_audit_history(
    workspace: impl AsRef<Path>,
) -> Result<RepoAuditReport, RepoAuditError> {
    let workspace = workspace.as_ref();
    let report = load_repo_audit(workspace)?;
    let mut store = ProjectStore::open(workspace)?;
    append_repo_hunk_history_entries(&mut store, &report)?;
    Ok(report)
}

pub(crate) fn append_repo_hunk_history_entries(
    store: &mut ProjectStore,
    report: &RepoAuditReport,
) -> Result<(), StoreError> {
    let observed_at = current_utc_timestamp().unwrap_or_else(|| "1970-01-01T00:00:00Z".into());
    for change in &report.changes {
        for (hunk_index, hunk) in change.hunks.iter().enumerate() {
            store.append_repo_hunk_history(&RepoHunkHistoryEntry {
                history_id: format!("repo-hunk-{}", current_id_fragment()),
                observed_at: observed_at.clone(),
                workspace: report.workspace.clone(),
                path: change.path.clone(),
                kind: change.kind,
                hunk_index,
                old_start: hunk.old_start,
                old_lines: hunk.old_lines,
                new_start: hunk.new_start,
                new_lines: hunk.new_lines,
                trace_status: hunk.trace_status,
                matching_trace_count: hunk.matching_trace_count,
                change_trace_status: change.trace_status,
                modified_at: change.modified_at,
                matching_trace_refs: repo_hunk_trace_refs(change, hunk),
            })?;
        }
    }
    Ok(())
}

pub(crate) fn repo_hunk_trace_refs(
    change: &RepoChangeAudit,
    hunk: &RepoDiffHunk,
) -> Vec<RepoHunkTraceRef> {
    matching_traces_for_hunk(&change.matching_traces, hunk, change.hunks.len())
        .into_iter()
        .take(REPO_AUDIT_MAX_MATCHING_TRACES_PER_CHANGE)
        .map(repo_hunk_trace_ref)
        .collect()
}

pub(crate) fn matching_traces_for_hunk<'a>(
    matches: &'a [TraceEntry],
    hunk: &RepoDiffHunk,
    hunk_count: usize,
) -> Vec<&'a TraceEntry> {
    let hunk_matches = matches
        .iter()
        .filter(|trace| {
            trace.line.is_some_and(|_| trace_matches_hunk(trace, hunk))
                || (hunk_count == 1 && trace.line.is_none())
        })
        .collect::<Vec<_>>();
    if !hunk_matches.is_empty() {
        return hunk_matches;
    }
    matches
        .iter()
        .filter(|trace| trace.line.is_none())
        .collect::<Vec<_>>()
}

pub(crate) fn repo_hunk_trace_ref(trace: &TraceEntry) -> RepoHunkTraceRef {
    RepoHunkTraceRef {
        event_id: trace.event_id.clone(),
        agent: Some(trace.agent.clone()).filter(|agent| !agent.trim().is_empty()),
        session: trace.session.clone(),
        line: trace.line,
        line_end: trace.line_end,
        rationale: trace.rationale.clone(),
        related_event_ids: trace.related_event_ids.clone(),
    }
}

pub(crate) fn git_changed_files(
    workspace: &Path,
) -> Result<Vec<(String, RepoChangeKind)>, RepoAuditError> {
    let output = git_output(
        workspace,
        ["status", "--porcelain=v1", "--untracked-files=all"],
    )?;
    let mut changes = Vec::new();
    for line in output.lines() {
        if line.len() < 4 {
            continue;
        }
        let status = &line[..2];
        let path = parse_git_status_path(&line[3..]);
        if path.is_empty() {
            continue;
        }
        let kind = if status == "??" {
            RepoChangeKind::Untracked
        } else if status.contains('D') {
            RepoChangeKind::Deleted
        } else if status.contains('A') {
            RepoChangeKind::Added
        } else {
            RepoChangeKind::Modified
        };
        changes.push((path, kind));
    }
    Ok(changes)
}

pub(crate) fn parse_git_status_path(path: &str) -> String {
    path.rsplit_once(" -> ")
        .map(|(_, renamed_to)| renamed_to)
        .unwrap_or(path)
        .trim_matches('"')
        .replace('\\', "/")
}

pub(crate) fn git_diff_hunks(
    workspace: &Path,
    path: &str,
) -> Result<Vec<RepoDiffHunk>, RepoAuditError> {
    let output = git_output(
        workspace,
        ["diff", "--unified=0", "--no-ext-diff", "HEAD", "--", path],
    )?;
    Ok(output
        .lines()
        .filter_map(parse_git_diff_hunk)
        .collect::<Vec<_>>())
}

pub(crate) fn parse_git_diff_hunk(line: &str) -> Option<RepoDiffHunk> {
    let rest = line.strip_prefix("@@ -")?;
    let (old_range, rest) = rest.split_once(" +")?;
    let (new_range, _) = rest.split_once(" @@")?;
    let (old_start, old_lines) = parse_diff_range(old_range)?;
    let (new_start, new_lines) = parse_diff_range(new_range)?;
    Some(RepoDiffHunk {
        old_start,
        old_lines,
        new_start,
        new_lines,
        trace_status: RepoTraceStatus::Untraced,
        matching_trace_count: 0,
    })
}

pub(crate) fn parse_diff_range(range: &str) -> Option<(u32, u32)> {
    if let Some((start, count)) = range.split_once(',') {
        Some((start.parse().ok()?, count.parse().ok()?))
    } else {
        Some((range.parse().ok()?, 1))
    }
}

pub(crate) fn traces_for_repo_change(
    traces: &[TraceEntry],
    workspace: &Path,
    path: &str,
    hunks: &[RepoDiffHunk],
    fresh_after: Option<i64>,
) -> Vec<TraceEntry> {
    let target = normalize_blame_path(workspace, path);
    traces
        .iter()
        .filter(|trace| normalize_blame_path(workspace, &trace.file) == target)
        .filter(|trace| trace_is_fresh_for_repo_audit(trace, fresh_after))
        .filter(|trace| trace_matches_hunks(trace, hunks))
        .cloned()
        .collect()
}

pub(crate) fn repo_change_fresh_after_seconds(
    workspace: &Path,
    path: &str,
    kind: RepoChangeKind,
) -> Option<i64> {
    if kind == RepoChangeKind::Deleted {
        return deleted_path_parent_modified_seconds(workspace, path)
            .or_else(|| git_head_commit_seconds(workspace));
    }
    fs::metadata(workspace.join(path))
        .ok()
        .and_then(|metadata| metadata.modified().ok())
        .and_then(system_time_seconds)
        .or_else(|| git_head_commit_seconds(workspace))
}

pub(crate) fn deleted_path_parent_modified_seconds(workspace: &Path, path: &str) -> Option<i64> {
    let mut current = workspace.join(path);
    while let Some(parent) = current.parent() {
        if let Ok(metadata) = fs::metadata(parent) {
            return metadata.modified().ok().and_then(system_time_seconds);
        }
        current = parent.to_path_buf();
    }
    None
}

pub(crate) fn git_head_commit_seconds(workspace: &Path) -> Option<i64> {
    git_output(workspace, ["log", "-1", "--format=%ct", "HEAD"])
        .ok()?
        .trim()
        .parse()
        .ok()
}

pub(crate) fn system_time_seconds(time: SystemTime) -> Option<i64> {
    let duration = time.duration_since(UNIX_EPOCH).ok()?;
    i64::try_from(duration.as_secs()).ok()
}

pub(crate) fn trace_is_fresh_for_repo_audit(trace: &TraceEntry, fresh_after: Option<i64>) -> bool {
    let Some(fresh_after) = fresh_after else {
        return true;
    };
    trace
        .time
        .as_deref()
        .and_then(parse_utc_seconds)
        .is_some_and(|trace_time| trace_time >= fresh_after)
}

pub(crate) fn bound_repo_audit_traces(traces: Vec<TraceEntry>) -> Vec<TraceEntry> {
    traces
        .into_iter()
        .rev()
        .take(REPO_AUDIT_MAX_MATCHING_TRACES_PER_CHANGE)
        .collect()
}

pub(crate) fn trace_matches_hunks(trace: &TraceEntry, hunks: &[RepoDiffHunk]) -> bool {
    if hunks.is_empty() || trace.line.is_none() {
        return true;
    }
    hunks.iter().any(|hunk| trace_matches_hunk(trace, hunk))
}

pub(crate) fn trace_matches_hunk(trace: &TraceEntry, hunk: &RepoDiffHunk) -> bool {
    trace_overlaps_diff_range(trace, hunk.new_start, hunk.new_lines)
        || trace_overlaps_diff_range(trace, hunk.old_start, hunk.old_lines)
}

pub(crate) fn trace_line_range_contains(trace: &TraceEntry, line: u32) -> bool {
    let Some(start) = trace.line else {
        return false;
    };
    let end = trace.line_end.unwrap_or(start).max(start);
    line >= start && line <= end
}

pub(crate) fn trace_overlaps_diff_range(trace: &TraceEntry, start: u32, lines: u32) -> bool {
    let Some(trace_start) = trace.line else {
        return true;
    };
    if lines == 0 {
        return false;
    }
    let trace_end = trace.line_end.unwrap_or(trace_start).max(trace_start);
    let range_end = start.saturating_add(lines - 1);
    trace_start <= range_end && trace_end >= start
}

pub(crate) fn trace_status_for_hunks(
    matches: &[TraceEntry],
    hunks: &[RepoDiffHunk],
) -> RepoTraceStatus {
    if hunks.is_empty() {
        return trace_status_for_matches(matches);
    }

    let mut has_missing_rationale = false;
    for hunk in hunks {
        match trace_status_for_hunk(matches, hunk, hunks.len()).0 {
            RepoTraceStatus::Traced => {}
            RepoTraceStatus::MissingRationale => has_missing_rationale = true,
            RepoTraceStatus::Untraced => return RepoTraceStatus::Untraced,
        }
    }

    if has_missing_rationale {
        RepoTraceStatus::MissingRationale
    } else {
        RepoTraceStatus::Traced
    }
}

pub(crate) fn annotate_repo_diff_hunks(matches: &[TraceEntry], hunks: &mut [RepoDiffHunk]) {
    let hunk_count = hunks.len();
    for hunk in hunks {
        let (trace_status, matching_trace_count) = trace_status_for_hunk(matches, hunk, hunk_count);
        hunk.trace_status = trace_status;
        hunk.matching_trace_count = matching_trace_count;
    }
}

pub(crate) fn trace_status_for_hunk(
    matches: &[TraceEntry],
    hunk: &RepoDiffHunk,
    hunk_count: usize,
) -> (RepoTraceStatus, usize) {
    let hunk_matches = matches
        .iter()
        .filter(|trace| {
            trace.line.is_some_and(|_| trace_matches_hunk(trace, hunk))
                || (hunk_count == 1 && trace.line.is_none())
        })
        .collect::<Vec<_>>();
    if !hunk_matches.is_empty() {
        let status = if hunk_matches.iter().any(|trace| trace_has_rationale(trace)) {
            RepoTraceStatus::Traced
        } else {
            RepoTraceStatus::MissingRationale
        };
        return (status, hunk_matches.len());
    }

    let file_level_trace_count = matches.iter().filter(|trace| trace.line.is_none()).count();
    if file_level_trace_count > 0 {
        (RepoTraceStatus::MissingRationale, file_level_trace_count)
    } else {
        (RepoTraceStatus::Untraced, 0)
    }
}

pub(crate) fn trace_status_for_matches(matches: &[TraceEntry]) -> RepoTraceStatus {
    if matches.is_empty() {
        RepoTraceStatus::Untraced
    } else if matches.iter().any(trace_has_rationale) {
        RepoTraceStatus::Traced
    } else {
        RepoTraceStatus::MissingRationale
    }
}

pub(crate) fn trace_has_rationale(trace: &TraceEntry) -> bool {
    trace
        .rationale
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
}

pub(crate) fn repo_audit_path_is_ignored(path: &str) -> bool {
    let normalized = normalize_path_text(path);
    normalized.starts_with(".agent-monitor/")
        || normalized == ".agent-monitor"
        || normalized.starts_with("target/")
}

pub(crate) fn git_output<const N: usize>(
    workspace: &Path,
    args: [&str; N],
) -> Result<String, RepoAuditError> {
    let args_display = args.join(" ");
    let output = Command::new("git")
        .current_dir(workspace)
        .args(args)
        .output()
        .map_err(|source| RepoAuditError::GitSpawn {
            args: args_display.clone(),
            source,
        })?;
    if !output.status.success() {
        return Err(RepoAuditError::GitFailed {
            args: args_display,
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }
    String::from_utf8(output.stdout).map_err(|source| RepoAuditError::GitUtf8 {
        args: args_display,
        source,
    })
}

pub(crate) fn blame_match_rank(kind: BlameMatchKind) -> u8 {
    match kind {
        BlameMatchKind::ExactLine => 0,
        BlameMatchKind::File => 1,
    }
}

pub(crate) fn evidence_from_snapshot(snapshot: &DashboardSnapshot) -> Vec<EvidenceItem> {
    let mut evidence = Vec::new();
    for (index, event) in snapshot.recent_events.iter().enumerate() {
        let (summary, redaction_status) =
            sanitize_evidence_summary(&truncate_evidence(&event_summary(event)));
        let redaction_status =
            strongest_redaction_status(redaction_status, event_redaction_status(event));
        evidence.push(EvidenceItem {
            id: event
                .event_id
                .clone()
                .unwrap_or_else(|| format!("event-{}", index + 1)),
            kind: format!("{:?}", event.kind),
            agent: Some(event.agent.clone()),
            session: event.session.clone(),
            run_id: event.run_id.clone(),
            agent_session_id: event.agent_session_id.clone(),
            summary,
            redaction_status,
            source: event.file.clone().or_else(|| event.command.clone()),
            source_type: event.source_type.clone(),
            source_path: event.source_path.clone(),
            source_offset: event.source_offset,
            source_hash: event.source_hash.clone(),
            redaction_rules: event.redaction_rules.clone(),
        });
    }
    for (index, intervention) in snapshot.recent_interventions.iter().enumerate() {
        let (summary, redaction_status) = sanitize_evidence_summary(&intervention.reason);
        evidence.push(EvidenceItem {
            id: format!("intervention-{}", index + 1),
            kind: format!("{:?}", intervention.kind),
            agent: intervention.agent.clone(),
            session: None,
            run_id: None,
            agent_session_id: None,
            summary: truncate_evidence(&summary),
            redaction_status,
            source: None,
            source_type: None,
            source_path: None,
            source_offset: None,
            source_hash: None,
            redaction_rules: Vec::new(),
        });
    }
    for run in &snapshot.recent_verifier_runs {
        let (summary, redaction_status) =
            sanitize_evidence_summary(&truncate_evidence(&verifier_run_summary(run)));
        evidence.push(EvidenceItem {
            id: run.verifier_run_id.clone(),
            kind: "VerifierRun".into(),
            agent: None,
            session: None,
            run_id: None,
            agent_session_id: None,
            summary,
            redaction_status,
            source: Some(run.command.clone()),
            source_type: Some("verifier".into()),
            source_path: None,
            source_offset: None,
            source_hash: None,
            redaction_rules: Vec::new(),
        });
    }
    for run in &snapshot.recent_probe_runs {
        let (summary, redaction_status) =
            sanitize_evidence_summary(&truncate_evidence(&probe_run_summary(run)));
        evidence.push(EvidenceItem {
            id: run.probe_run_id.clone(),
            kind: "ProbeRun".into(),
            agent: None,
            session: None,
            run_id: None,
            agent_session_id: None,
            summary,
            redaction_status,
            source: Some(run.advice_id.clone()),
            source_type: Some("probe".into()),
            source_path: Some("probe-runs.jsonl".into()),
            source_offset: None,
            source_hash: None,
            redaction_rules: Vec::new(),
        });
    }
    for report in &snapshot.recent_dev_history {
        for (finding_index, finding) in report.findings.iter().enumerate() {
            let (summary, redaction_status) = sanitize_evidence_summary(&truncate_evidence(
                &dev_history_finding_evidence_summary(report, finding),
            ));
            evidence.push(EvidenceItem {
                id: dev_history_finding_evidence_id(report, finding_index, finding),
                kind: "DevHistoryFinding".into(),
                agent: None,
                session: None,
                run_id: None,
                agent_session_id: None,
                summary,
                redaction_status,
                source: Some(report.workspace.clone()),
                source_type: Some("dev_history".into()),
                source_path: Some("dev-history.jsonl".into()),
                source_offset: None,
                source_hash: None,
                redaction_rules: Vec::new(),
            });
        }
    }
    evidence
}

pub(crate) fn evidence_from_project_contract_requirements(
    requirements: &[ProjectContractRequirement],
) -> Vec<EvidenceItem> {
    requirements
        .iter()
        .map(|requirement| {
            let raw_summary = format!(
                "Project contract requirement from {}:{}: {}",
                requirement.source_path, requirement.line, requirement.text
            );
            let (summary, redaction_status) =
                sanitize_evidence_summary(&truncate_evidence(&raw_summary));
            EvidenceItem {
                id: requirement.evidence_id.clone(),
                kind: "ProjectContract".into(),
                agent: None,
                session: None,
                run_id: None,
                agent_session_id: None,
                summary,
                redaction_status,
                source: Some(format!("{}:{}", requirement.source_path, requirement.line)),
                source_type: Some("project_contract".into()),
                source_path: Some(requirement.source_path.clone()),
                source_offset: Some(requirement.line),
                source_hash: Some(requirement.source_hash.clone()),
                redaction_rules: Vec::new(),
            }
        })
        .collect()
}

pub(crate) fn dev_history_finding_evidence_id(
    report: &DevHistoryReport,
    finding_index: usize,
    finding: &DevHistoryFinding,
) -> String {
    let mut seed = String::new();
    push_dev_history_id_field(&mut seed, &report.workspace);
    push_dev_history_id_field(&mut seed, &report.generated_at);
    push_dev_history_id_field(&mut seed, &finding_index.to_string());
    for source in &report.sources {
        push_dev_history_id_field(&mut seed, &source.source);
        push_dev_history_id_field(&mut seed, &source.history_root);
        push_dev_history_id_field(&mut seed, &source.files.to_string());
        push_dev_history_id_field(&mut seed, &source.bytes.to_string());
        push_dev_history_id_field(&mut seed, &source.lines.to_string());
        push_dev_history_id_field(&mut seed, &source.parsed.to_string());
        push_dev_history_id_field(&mut seed, &source.sessions.to_string());
    }
    push_dev_history_id_field(&mut seed, &finding.kind);
    push_dev_history_id_field(&mut seed, &finding.severity);
    push_dev_history_id_field(&mut seed, &finding.summary);
    for evidence in &finding.evidence {
        push_dev_history_id_field(&mut seed, evidence);
    }
    for response in &finding.monitor_response {
        push_dev_history_id_field(&mut seed, response);
    }
    let digest = fnv1a64_digest(seed.as_bytes())
        .strip_prefix("fnv1a64:")
        .unwrap_or("unknown")
        .to_string();
    format!("dev-history-{}-{digest}", safe_slug(&finding.kind))
}

pub(crate) fn push_dev_history_id_field(seed: &mut String, value: &str) {
    seed.push_str(&value.len().to_string());
    seed.push(':');
    seed.push_str(value);
    seed.push('\n');
}

pub(crate) fn dev_history_finding_evidence_summary(
    report: &DevHistoryReport,
    finding: &DevHistoryFinding,
) -> String {
    let evidence = if finding.evidence.is_empty() {
        "no aggregate evidence details".into()
    } else {
        finding.evidence.join("; ")
    };
    let sources = report
        .sources
        .iter()
        .map(|source| format!("{}:{} file(s)", source.source, source.files))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "{} {} finding: {} Evidence: {}. Sources: {}.",
        finding.severity, finding.kind, finding.summary, evidence, sources
    )
}

pub(crate) fn evidence_from_repo_audit(report: &RepoAuditReport) -> Vec<EvidenceItem> {
    report
        .changes
        .iter()
        .map(|change| {
            let status = match change.trace_status {
                RepoTraceStatus::Traced => "traced",
                RepoTraceStatus::MissingRationale => "missing rationale",
                RepoTraceStatus::Untraced => "untraced",
            };
            let summary = format!("{} has {status} dirty git hunks", change.path);
            let (summary, redaction_status) = sanitize_evidence_summary(&summary);
            EvidenceItem {
                id: format!("repo-audit-{}", safe_slug(&change.path)),
                kind: "repo_audit".into(),
                agent: None,
                session: None,
                run_id: None,
                agent_session_id: None,
                summary: truncate_evidence(&summary),
                redaction_status,
                source: Some(change.path.clone()),
                source_type: Some("git".into()),
                source_path: Some(change.path.clone()),
                source_offset: None,
                source_hash: None,
                redaction_rules: Vec::new(),
            }
        })
        .collect()
}

pub(crate) fn memory_candidates_from_snapshot(
    snapshot: &DashboardSnapshot,
) -> Vec<MemoryCandidate> {
    let mut candidates = Vec::new();
    for (index, event) in snapshot.recent_events.iter().enumerate() {
        if event.redaction_status.as_deref() == Some("tainted") {
            continue;
        }
        let Some((claim, source, confidence)) = memory_candidate_claim_from_event(event) else {
            continue;
        };
        let evidence_id = event
            .event_id
            .clone()
            .unwrap_or_else(|| format!("event-{}", index + 1));
        candidates.push(MemoryCandidate {
            memory_id: format!("mem-{}", safe_slug(&evidence_id)),
            scope: MemoryScope::Project,
            claim,
            status: MemoryStatus::Unverified,
            source,
            evidence_ids: vec![evidence_id],
            confidence,
        });
    }
    candidates
}

pub(crate) fn memory_candidate_claim_from_event(
    event: &Event,
) -> Option<(String, MemorySource, u8)> {
    let content = event.content.as_ref()?.trim();
    if content.is_empty() {
        return None;
    }
    match event.kind {
        EventKind::DesignThought => Some((content.to_string(), MemorySource::AgentClaim, 50)),
        EventKind::UserInstruction => {
            durable_user_instruction_claim(content).map(|claim| (claim, MemorySource::User, 80))
        }
        _ => None,
    }
}

pub(crate) fn durable_user_instruction_claim(content: &str) -> Option<String> {
    content
        .lines()
        .map(strip_user_memory_line_prefix)
        .find(|line| durable_user_memory_line(line))
        .map(clean_user_memory_claim)
        .filter(|claim| !claim.is_empty())
}

pub(crate) fn strip_user_memory_line_prefix(line: &str) -> &str {
    line.trim_start_matches([' ', '\t', '-', '*', '#'])
        .trim_start()
}

pub(crate) fn durable_user_memory_line(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    [
        "remember:",
        "remember this:",
        "keep in mind:",
        "project constraint:",
        "constraint:",
        "preference:",
        "prefer ",
        "do not ",
        "never ",
    ]
    .iter()
    .any(|marker| lower.starts_with(marker))
}

pub(crate) fn clean_user_memory_claim(line: &str) -> String {
    for prefix in [
        "remember this:",
        "remember:",
        "keep in mind:",
        "project constraint:",
        "constraint:",
        "preference:",
    ] {
        if line
            .get(..prefix.len())
            .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
        {
            return line[prefix.len()..].trim().to_string();
        }
    }
    line.trim().to_string()
}
