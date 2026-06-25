//! Append-only project store: the on-disk `.agent-monitor` event/trace/advice
//! log surface plus the worktree-lock and control-packet persistence helpers.
//!
//! `ProjectStore` owns all reads and writes to the JSONL contract described in
//! `CLAUDE.md`; pure control logic lives in the engine free functions and the
//! calibration/monitor layers in `lib.rs`.

use crate::*;
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::thread;

#[derive(Debug)]
pub struct ProjectStore {
    pub(crate) workspace_root: PathBuf,
    pub(crate) root: PathBuf,
    pub(crate) temp_dir: PathBuf,
}

pub(crate) const JSONL_APPEND_LOCK_ATTEMPTS: usize = 2_000;
pub(crate) const JSONL_APPEND_LOCK_SLEEP: Duration = Duration::from_millis(5);

pub(crate) struct JsonlAppendLock {
    path: PathBuf,
    file: Option<fs::File>,
}

impl Drop for JsonlAppendLock {
    fn drop(&mut self) {
        drop(self.file.take());
        let _ = fs::remove_file(&self.path);
    }
}

impl ProjectStore {
    pub fn open(workspace_root: impl AsRef<Path>) -> Result<Self, StoreError> {
        let workspace_root = workspace_root.as_ref().to_path_buf();
        let root = workspace_root.join(".agent-monitor");
        let temp_dir = root.join("tmp");
        fs::create_dir_all(&temp_dir).map_err(|source| StoreError::CreateDir {
            path: temp_dir.clone(),
            source,
        })?;
        Ok(Self {
            workspace_root,
            root,
            temp_dir,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn temp_dir(&self) -> PathBuf {
        self.temp_dir.clone()
    }

    pub fn append_event(&mut self, event: &Event) -> Result<(), StoreError> {
        self.append_event_and_return(event).map(|_| ())
    }

    pub(crate) fn append_event_and_return(&mut self, event: &Event) -> Result<Event, StoreError> {
        let _lock = self.acquire_jsonl_append_lock("events.jsonl")?;
        let event = self.prepare_event_for_persistence(event)?;
        self.append_prepared_event(&event)?;
        Ok(event)
    }

    pub(crate) fn prepare_event_for_persistence(&self, event: &Event) -> Result<Event, StoreError> {
        let mut event = event.clone();
        if event.event_id.as_deref().is_none_or(str::is_empty) {
            event.event_id = Some(format!("event-{}", current_id_fragment()));
        }
        if event.seq.is_none() {
            event.seq = Some(self.next_event_seq()?);
        }
        stamp_store_event_provenance(&mut event, &self.workspace_root);
        Ok(event)
    }

    pub(crate) fn next_event_seq(&self) -> Result<u64, StoreError> {
        let max_seq = read_all_jsonl::<Event>(&self.root.join("events.jsonl"))?
            .into_iter()
            .filter_map(|event| event.seq)
            .max()
            .unwrap_or(0);
        Ok(max_seq.saturating_add(1))
    }

    pub(crate) fn acquire_jsonl_append_lock(
        &self,
        name: &str,
    ) -> Result<JsonlAppendLock, StoreError> {
        let path = self.jsonl_append_lock_path(name);
        for _ in 0..JSONL_APPEND_LOCK_ATTEMPTS {
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(file) => {
                    return Ok(JsonlAppendLock {
                        path,
                        file: Some(file),
                    });
                }
                Err(source)
                    if matches!(
                        source.kind(),
                        std::io::ErrorKind::AlreadyExists | std::io::ErrorKind::PermissionDenied
                    ) =>
                {
                    thread::sleep(JSONL_APPEND_LOCK_SLEEP);
                }
                Err(source) => {
                    return Err(StoreError::Append {
                        path: path.clone(),
                        source,
                    });
                }
            }
        }
        Err(StoreError::JsonlAppendLockTimeout { path })
    }

    pub(crate) fn jsonl_append_lock_path(&self, name: &str) -> PathBuf {
        let stem = name.strip_suffix(".jsonl").unwrap_or(name);
        self.root.join(format!("{}.lock", safe_slug(stem)))
    }

    pub(crate) fn append_prepared_event(&mut self, event: &Event) -> Result<(), StoreError> {
        let event = storage_redacted_event(event);
        self.append_jsonl_unlocked("events.jsonl", &event)
    }

    pub fn append_intervention(&mut self, intervention: &Intervention) -> Result<(), StoreError> {
        self.append_jsonl("interventions.jsonl", intervention)
    }

    pub fn append_design(&mut self, entry: &DesignEntry) -> Result<(), StoreError> {
        self.append_jsonl("design.jsonl", entry)
    }

    pub fn append_memory(&mut self, memory: &MemoryCandidate) -> Result<(), StoreError> {
        self.append_jsonl("memories.jsonl", memory)
    }

    pub fn append_trace(&mut self, entry: &TraceEntry) -> Result<(), StoreError> {
        let entry = storage_redacted_trace(entry);
        self.append_jsonl("trace.jsonl", &entry)
    }

    pub fn append_case_file(&mut self, case_file: &ControlCaseFile) -> Result<(), StoreError> {
        self.append_jsonl("case-files.jsonl", case_file)
    }

    pub fn append_advice(&mut self, advice: &AdviceRun) -> Result<(), StoreError> {
        self.validate_advice_for_persistence(advice)?;
        self.append_jsonl("advice.jsonl", advice)
    }

    pub fn append_packet(&mut self, packet: &ControlPacket) -> Result<(), StoreError> {
        self.validate_control_packet_for_persistence(packet)?;
        self.append_packet_unchecked(packet)
    }

    pub(crate) fn append_packet_unchecked(
        &mut self,
        packet: &ControlPacket,
    ) -> Result<(), StoreError> {
        self.append_jsonl("packets.jsonl", packet)
    }

    pub fn append_dispatch(&mut self, dispatch: &DispatchResult) -> Result<(), StoreError> {
        self.append_jsonl("dispatch.jsonl", dispatch)
    }

    pub fn append_action_outcome(&mut self, outcome: &ActionOutcome) -> Result<(), StoreError> {
        self.append_jsonl("outcomes.jsonl", outcome)
    }

    pub fn append_verifier_run(&mut self, run: &VerifierRun) -> Result<(), StoreError> {
        self.append_jsonl("verifier-runs.jsonl", run)
    }

    pub fn append_probe_run(&mut self, run: &ProbeRun) -> Result<(), StoreError> {
        self.append_jsonl("probe-runs.jsonl", run)
    }

    pub fn append_repo_hunk_history(
        &mut self,
        entry: &RepoHunkHistoryEntry,
    ) -> Result<(), StoreError> {
        self.append_jsonl("repo-hunks.jsonl", entry)
    }

    pub fn append_dev_history_report(
        &mut self,
        report: &DevHistoryReport,
    ) -> Result<(), StoreError> {
        self.append_jsonl("dev-history.jsonl", report)
    }

    pub fn dispatch_control_packet(
        &mut self,
        packet: &ControlPacket,
    ) -> Result<DispatchResult, StoreError> {
        self.validate_control_packet_for_persistence(packet)?;
        let agent_dir = self.control_packet_agent_dir(packet);
        fs::create_dir_all(&agent_dir).map_err(|source| StoreError::CreateDir {
            path: agent_dir.clone(),
            source,
        })?;
        let path = immutable_control_packet_path(&agent_dir, packet);
        let rendered = render_control_packet(packet);
        write_new_control_packet_file(&path, &rendered)?;
        self.append_packet_unchecked(packet)?;
        let dispatch = DispatchResult {
            dispatch_id: format!("dispatch-{}", current_id_fragment()),
            packet_id: packet.packet_id.clone(),
            target_agent: packet.target_agent.clone(),
            status: DispatchStatus::OutboxWritten,
            path: Some(path.display().to_string()),
            reason: None,
        };
        publish_latest_control_packet(&agent_dir, packet, &rendered)?;
        self.append_dispatch(&dispatch)?;
        Ok(dispatch)
    }

    pub fn try_acquire_worktree_lock(
        &mut self,
        request: &WorktreeLockRequest,
    ) -> Result<WorktreeLockResult, StoreError> {
        let lock_dir = self.root.join("locks").join("worktrees");
        fs::create_dir_all(&lock_dir).map_err(|source| StoreError::CreateDir {
            path: lock_dir.clone(),
            source,
        })?;
        let path = worktree_lock_path(&self.root, &request.worktree);

        let lock = WorktreeLock {
            lock_id: format!("lock-{}", current_id_fragment()),
            worktree: request.worktree.clone(),
            owner_agent: request.owner_agent.clone(),
            session: request.session.clone(),
            acquired_at: current_utc_timestamp().unwrap_or_else(|| "1970-01-01T00:00:00Z".into()),
        };
        let mut file = match OpenOptions::new().create_new(true).write(true).open(&path) {
            Ok(file) => file,
            Err(source) if source.kind() == std::io::ErrorKind::AlreadyExists => {
                let existing = read_worktree_lock(&path)?;
                self.append_lock_event(&WorktreeLockEvent {
                    kind: "conflict".into(),
                    lock: existing.clone(),
                    requested_owner: Some(request.owner_agent.clone()),
                })?;
                return Ok(WorktreeLockResult::Conflict { existing });
            }
            Err(source) => {
                return Err(StoreError::Append {
                    path: path.clone(),
                    source,
                });
            }
        };
        serde_json::to_writer(&mut file, &lock).map_err(|source| StoreError::Encode {
            path: path.clone(),
            source,
        })?;
        file.write_all(b"\n").map_err(|source| StoreError::Append {
            path: path.clone(),
            source,
        })?;
        self.append_lock_event(&WorktreeLockEvent {
            kind: "acquired".into(),
            lock: lock.clone(),
            requested_owner: None,
        })?;
        Ok(WorktreeLockResult::Acquired(lock))
    }

    pub fn release_worktree_lock(
        &mut self,
        worktree: &str,
        lock_id: &str,
    ) -> Result<bool, StoreError> {
        let path = worktree_lock_path(&self.root, worktree);
        if !path.exists() {
            return Ok(false);
        }
        let lock = read_worktree_lock(&path)?;
        if lock.lock_id != lock_id {
            return Ok(false);
        }
        fs::remove_file(&path).map_err(|source| StoreError::Remove {
            path: path.clone(),
            source,
        })?;
        self.append_lock_event(&WorktreeLockEvent {
            kind: "released".into(),
            lock,
            requested_owner: None,
        })?;
        Ok(true)
    }

    pub fn release_stale_worktree_locks(
        &mut self,
        stale_after_secs: i64,
    ) -> Result<Vec<WorktreeLock>, StoreError> {
        if stale_after_secs <= 0 {
            return Ok(Vec::new());
        }
        let Some(now) = current_utc_seconds() else {
            return Ok(Vec::new());
        };
        let lock_dir = self.root.join("locks").join("worktrees");
        if !lock_dir.exists() {
            return Ok(Vec::new());
        }

        let mut released = Vec::new();
        for entry in fs::read_dir(&lock_dir).map_err(|source| StoreError::Read {
            path: lock_dir.clone(),
            source,
        })? {
            let path = entry
                .map_err(|source| StoreError::Read {
                    path: lock_dir.clone(),
                    source,
                })?
                .path();
            if path.extension().is_none_or(|extension| extension != "json") {
                continue;
            }
            let lock = read_worktree_lock(&path)?;
            let Some(acquired_at) = parse_utc_seconds(&lock.acquired_at) else {
                continue;
            };
            if now - acquired_at < stale_after_secs {
                continue;
            }
            fs::remove_file(&path).map_err(|source| StoreError::Remove {
                path: path.clone(),
                source,
            })?;
            self.append_lock_event(&WorktreeLockEvent {
                kind: "expired".into(),
                lock: lock.clone(),
                requested_owner: None,
            })?;
            released.push(lock);
        }
        Ok(released)
    }

    pub(crate) fn active_worktree_lock_for(
        &self,
        worktree: &str,
    ) -> Result<Option<WorktreeLock>, StoreError> {
        let path = worktree_lock_path(&self.root, worktree);
        if !path.exists() {
            return Ok(None);
        }
        read_worktree_lock(&path).map(Some)
    }

    pub(crate) fn active_worktree_lock_count(&self) -> Result<usize, StoreError> {
        let lock_dir = self.root.join("locks").join("worktrees");
        if !lock_dir.exists() {
            return Ok(0);
        }

        let mut count = 0;
        for entry in fs::read_dir(&lock_dir).map_err(|source| StoreError::Read {
            path: lock_dir.clone(),
            source,
        })? {
            let path = entry
                .map_err(|source| StoreError::Read {
                    path: lock_dir.clone(),
                    source,
                })?
                .path();
            if path
                .extension()
                .is_some_and(|extension| extension == "json")
            {
                read_worktree_lock(&path)?;
                count += 1;
            }
        }
        Ok(count)
    }

    pub fn write_control_packet(&mut self, packet: &ControlPacket) -> Result<PathBuf, StoreError> {
        self.validate_control_packet_for_persistence(packet)?;
        let agent_dir = self.control_packet_agent_dir(packet);
        fs::create_dir_all(&agent_dir).map_err(|source| StoreError::CreateDir {
            path: agent_dir.clone(),
            source,
        })?;
        let path = immutable_control_packet_path(&agent_dir, packet);
        let rendered = render_control_packet(packet);
        write_new_control_packet_file(&path, &rendered)?;
        self.append_packet_unchecked(packet)?;
        publish_latest_control_packet(&agent_dir, packet, &rendered)?;
        Ok(path)
    }

    pub(crate) fn control_packet_agent_dir(&self, packet: &ControlPacket) -> PathBuf {
        self.root
            .join("outbox")
            .join(safe_slug(&packet.target_agent))
    }

    pub(crate) fn validate_control_packet_for_persistence(
        &self,
        packet: &ControlPacket,
    ) -> Result<(), StoreError> {
        self.validate_packet_preconditions(packet)?;
        validate_control_packet_is_clean(packet)?;
        self.validate_packet_evidence_refs(packet)
    }

    pub(crate) fn validate_advice_for_persistence(
        &self,
        advice: &AdviceRun,
    ) -> Result<(), StoreError> {
        validate_control_packet_is_clean(&advice.packet)?;
        let case_file = self.case_file_by_id(&advice.case_file_id)?;
        let refs = packet_evidence_refs(&advice.packet);
        if refs.is_empty() {
            return Ok(());
        }

        let known_ids = case_file_known_evidence_ids(&case_file);

        for evidence_ref in refs {
            if !known_ids.contains(evidence_ref) {
                return Err(StoreError::UnknownPacketEvidenceRef {
                    evidence_ref: evidence_ref.into(),
                });
            }
        }
        Ok(())
    }

    pub(crate) fn case_file_by_id(
        &self,
        case_file_id: &str,
    ) -> Result<ControlCaseFile, StoreError> {
        read_all_jsonl::<ControlCaseFile>(&self.root.join("case-files.jsonl"))?
            .into_iter()
            .rev()
            .find(|case_file| case_file.case_file_id == case_file_id)
            .ok_or_else(|| StoreError::AdviceCaseFileMissing {
                case_file_id: case_file_id.into(),
            })
    }

    pub(crate) fn validate_packet_evidence_refs(
        &self,
        packet: &ControlPacket,
    ) -> Result<(), StoreError> {
        let refs = packet_evidence_refs(packet);
        if refs.is_empty() {
            return Ok(());
        }

        let snapshot = DashboardSnapshot::load(&self.root, 500)?;
        let case_file = build_control_case_file(&self.workspace_root, &snapshot);
        let known_ids = case_file_known_evidence_ids(&case_file);

        for evidence_ref in refs {
            if !known_ids.contains(evidence_ref) {
                return Err(StoreError::UnknownPacketEvidenceRef {
                    evidence_ref: evidence_ref.into(),
                });
            }
        }
        Ok(())
    }

    pub(crate) fn validate_packet_preconditions(
        &self,
        packet: &ControlPacket,
    ) -> Result<(), StoreError> {
        if let Some(expected_adapter) = &packet.preconditions.adapter
            && normalize_agent_label(expected_adapter)
                != normalize_agent_label(&packet.target_agent)
        {
            return Err(StoreError::PacketPrecondition {
                field: "adapter".into(),
                expected: expected_adapter.clone(),
                actual: packet.target_agent.clone(),
            });
        }

        if let Some(expected_worktree) = &packet.preconditions.worktree {
            let actual_worktree = self.workspace_root.display().to_string();
            if normalize_path_for_match(expected_worktree)
                != normalize_path_for_match(&actual_worktree)
            {
                return Err(StoreError::PacketPrecondition {
                    field: "worktree".into(),
                    expected: expected_worktree.clone(),
                    actual: actual_worktree,
                });
            }
        }

        if let Some(expected_head) = &packet.preconditions.git_head {
            let actual_head =
                current_git_head(&self.workspace_root).unwrap_or_else(|| "<unavailable>".into());
            if expected_head != &actual_head {
                return Err(StoreError::PacketPrecondition {
                    field: "git_head".into(),
                    expected: expected_head.clone(),
                    actual: actual_head,
                });
            }
        }

        if let Some(expected_run_id) = &packet.preconditions.run_id {
            let target_agent = packet.target_agent.as_str();
            let actual_run_id = self
                .latest_event_precondition_value(target_agent, |event| event.run_id.as_deref())?
                .unwrap_or_else(|| "<unavailable>".into());
            if expected_run_id != &actual_run_id {
                return Err(StoreError::PacketPrecondition {
                    field: "run_id".into(),
                    expected: expected_run_id.clone(),
                    actual: actual_run_id,
                });
            }
        }

        if let Some(expected_session_id) = &packet.preconditions.agent_session_id {
            let target_agent = packet.target_agent.as_str();
            let actual_session_id = self
                .latest_event_precondition_value(target_agent, |event| {
                    event.agent_session_id.as_deref()
                })?
                .or(self.latest_event_precondition_value(target_agent, |event| {
                    event.session.as_deref()
                })?)
                .unwrap_or_else(|| "<unavailable>".into());
            if expected_session_id != &actual_session_id {
                return Err(StoreError::PacketPrecondition {
                    field: "agent_session_id".into(),
                    expected: expected_session_id.clone(),
                    actual: actual_session_id,
                });
            }
        }

        Ok(())
    }

    pub(crate) fn latest_event_precondition_value<F>(
        &self,
        target_agent: &str,
        mut extract: F,
    ) -> Result<Option<String>, StoreError>
    where
        F: FnMut(&Event) -> Option<&str>,
    {
        let target_agent = normalize_agent_label(target_agent);
        Ok(read_all_jsonl::<Event>(&self.root.join("events.jsonl"))?
            .into_iter()
            .rev()
            .filter(|event| normalize_agent_label(&event.agent) == target_agent)
            .find_map(|event| {
                extract(&event)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
            }))
    }

    pub(crate) fn append_jsonl<T: Serialize>(
        &mut self,
        name: &str,
        value: &T,
    ) -> Result<(), StoreError> {
        let _lock = self.acquire_jsonl_append_lock(name)?;
        self.append_jsonl_unlocked(name, value)
    }

    pub(crate) fn append_jsonl_unlocked<T: Serialize>(
        &mut self,
        name: &str,
        value: &T,
    ) -> Result<(), StoreError> {
        let path = self.root.join(name);
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|source| StoreError::Append {
                path: path.clone(),
                source,
            })?;

        serde_json::to_writer(&mut file, value).map_err(|source| StoreError::Encode {
            path: path.clone(),
            source,
        })?;
        writeln!(file).map_err(|source| StoreError::Append { path, source })?;
        Ok(())
    }

    pub(crate) fn append_lock_event(
        &mut self,
        event: &WorktreeLockEvent,
    ) -> Result<(), StoreError> {
        self.append_jsonl("locks.jsonl", event)
    }
}

pub(crate) fn stamp_store_event_provenance(event: &mut Event, workspace_root: &Path) {
    let observed_at = current_utc_timestamp().unwrap_or_else(|| "1970-01-01T00:00:00Z".into());
    fill_empty_string(&mut event.observed_at, observed_at.clone());
    let occurred_at = event.time.clone().unwrap_or_else(|| observed_at.clone());
    fill_empty_string(&mut event.occurred_at, occurred_at);

    let workspace = workspace_root.display().to_string();
    fill_empty_string(&mut event.workspace, workspace.clone());
    fill_empty_string(&mut event.cwd, workspace.clone());
    fill_empty_string(&mut event.worktree, workspace);

    if event.git_head.as_deref().is_none_or(str::is_empty) {
        event.git_head = current_git_head(workspace_root);
    }
    if event.git_branch.as_deref().is_none_or(str::is_empty) {
        event.git_branch = current_git_branch(workspace_root);
    }
    if event.git_dirty.is_none() {
        event.git_dirty = current_git_dirty(workspace_root);
    }

    fill_empty_string(&mut event.source_type, "monitor".into());
    fill_empty_string(&mut event.source_path, "ProjectStore::append_event".into());
    if event.source_hash.as_deref().is_none_or(str::is_empty) {
        let bytes = serde_json::to_vec(event).unwrap_or_default();
        event.source_hash = Some(fnv1a64_digest(&bytes));
    }
    fill_empty_string(&mut event.redaction_status, "clean".into());
}

pub(crate) fn fill_empty_string(target: &mut Option<String>, value: String) {
    if target.as_deref().is_none_or(str::is_empty) {
        *target = Some(value);
    }
}

pub(crate) fn immutable_control_packet_path(agent_dir: &Path, packet: &ControlPacket) -> PathBuf {
    agent_dir.join(format!("{}.md", safe_slug(&packet.packet_id)))
}

pub(crate) fn write_new_control_packet_file(path: &Path, rendered: &str) -> Result<(), StoreError> {
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(|source| {
            if source.kind() == std::io::ErrorKind::AlreadyExists {
                StoreError::PacketExists {
                    path: path.to_path_buf(),
                }
            } else {
                StoreError::Append {
                    path: path.to_path_buf(),
                    source,
                }
            }
        })?;
    file.write_all(rendered.as_bytes())
        .map_err(|source| StoreError::Append {
            path: path.to_path_buf(),
            source,
        })?;
    Ok(())
}

pub(crate) fn publish_latest_control_packet(
    agent_dir: &Path,
    packet: &ControlPacket,
    rendered: &str,
) -> Result<(), StoreError> {
    let latest_path = agent_dir.join("latest.md");
    let temp_path = agent_dir.join(format!(
        ".latest-{}-{}.tmp",
        safe_slug(&packet.packet_id),
        current_id_fragment()
    ));
    {
        let mut temp = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&temp_path)
            .map_err(|source| StoreError::Append {
                path: temp_path.clone(),
                source,
            })?;
        temp.write_all(rendered.as_bytes())
            .map_err(|source| StoreError::Append {
                path: temp_path.clone(),
                source,
            })?;
        temp.sync_all().map_err(|source| StoreError::Append {
            path: temp_path.clone(),
            source,
        })?;
    }
    replace_file(&temp_path, &latest_path).map_err(|source| StoreError::Append {
        path: latest_path,
        source,
    })?;
    Ok(())
}

#[cfg(windows)]
pub(crate) fn replace_file(from: &Path, to: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
    };

    let from = from
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let to = to
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let ok = unsafe {
        MoveFileExW(
            from.as_ptr(),
            to.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if ok == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
pub(crate) fn replace_file(from: &Path, to: &Path) -> std::io::Result<()> {
    fs::rename(from, to)
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("create directory {path}: {source}")]
    CreateDir {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("append {path}: {source}")]
    Append {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("timed out waiting for JSONL append lock: {path}")]
    JsonlAppendLockTimeout { path: PathBuf },
    #[error("encode {path}: {source}")]
    Encode {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("read {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("decode {path} line {line}: {source}")]
    Decode {
        path: PathBuf,
        line: usize,
        #[source]
        source: serde_json::Error,
    },
    #[error("remove {path}: {source}")]
    Remove {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("packet already exists: {path}")]
    PacketExists { path: PathBuf },
    #[error("secret-like packet content in {field}")]
    SecretLikePacket { field: String },
    #[error("advice references missing case file: {case_file_id}")]
    AdviceCaseFileMissing { case_file_id: String },
    #[error("unknown packet evidence ref: {evidence_ref}")]
    UnknownPacketEvidenceRef { evidence_ref: String },
    #[error("packet precondition failed for {field}: expected {expected}, actual {actual}")]
    PacketPrecondition {
        field: String,
        expected: String,
        actual: String,
    },
    #[error(
        "worktree {worktree} is already locked by {existing_owner}; cannot assign writable handoff to {requested_owner}"
    )]
    WorktreeLockConflict {
        worktree: String,
        existing_owner: String,
        requested_owner: String,
    },
    #[error(
        "max_parallel_writable_agents limit reached: {active}/{max}; cannot assign writable handoff to {requested_owner}"
    )]
    WorktreeCapacityExceeded {
        active: usize,
        max: usize,
        requested_owner: String,
    },
}
