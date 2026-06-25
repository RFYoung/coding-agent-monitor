//! `agent-monitor wrap` support: supervise a child coding-agent under a worktree lock.

use crate::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrappedCommand {
    pub agent: AgentKind,
    pub session: Option<String>,
    pub command: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrappedCommandResult {
    pub exit_code: Option<i32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WrappedLaunch {
    pub program: String,
    pub args: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum WrappedCommandError {
    #[error("wrapped command is empty")]
    EmptyCommand,
    #[error("agent {agent} requires an external sandbox; generic wrap cannot launch it directly")]
    ExternalSandboxRequired { agent: String },
    #[error("spawn wrapped command {program}: {source}")]
    Spawn {
        program: String,
        #[source]
        source: std::io::Error,
    },
    #[error("read wrapped command output: {0}")]
    Read(#[source] std::io::Error),
    #[error("capture thread panicked")]
    CaptureThreadPanicked,
    #[error("persist wrapped command event: {0}")]
    Persist(#[from] StoreError),
    #[error("wrapped command control loop: {0}")]
    ControlLoop(#[from] AdviceError),
}

pub fn run_wrapped_command(
    wrapped: WrappedCommand,
    store: &mut ProjectStore,
    stdout: impl Write,
    stderr: impl Write,
) -> Result<WrappedCommandResult, WrappedCommandError> {
    if adapter_capabilities_for(wrapped.agent).requires_external_sandbox {
        return Err(WrappedCommandError::ExternalSandboxRequired {
            agent: agent_kind_label(wrapped.agent).into(),
        });
    }

    let lock = acquire_wrapped_command_lock(&wrapped, store)?;
    let result = run_wrapped_command_with_lock(&wrapped, store, stdout, stderr);
    let release_result = store
        .release_worktree_lock(&lock.worktree, &lock.lock_id)
        .map_err(WrappedCommandError::Persist);
    match (result, release_result) {
        (Ok(result), Ok(_)) => Ok(result),
        (Err(error), Ok(_)) => Err(error),
        (Ok(_), Err(error)) => Err(error),
        (Err(error), Err(_release_error)) => Err(error),
    }
}

pub(crate) fn acquire_wrapped_command_lock(
    wrapped: &WrappedCommand,
    store: &mut ProjectStore,
) -> Result<WorktreeLock, WrappedCommandError> {
    let owner_agent = agent_kind_label(wrapped.agent).to_string();
    match store.try_acquire_worktree_lock(&WorktreeLockRequest {
        worktree: store.workspace_root.display().to_string(),
        owner_agent,
        session: wrapped.session.clone(),
    })? {
        WorktreeLockResult::Acquired(lock) => Ok(lock),
        WorktreeLockResult::Conflict { existing } => Err(StoreError::WorktreeLockConflict {
            worktree: existing.worktree,
            existing_owner: existing.owner_agent,
            requested_owner: agent_kind_label(wrapped.agent).into(),
        }
        .into()),
    }
}

pub(crate) fn run_wrapped_command_with_lock(
    wrapped: &WrappedCommand,
    store: &mut ProjectStore,
    mut stdout: impl Write,
    mut stderr: impl Write,
) -> Result<WrappedCommandResult, WrappedCommandError> {
    let launch = prepare_wrapped_launch(&wrapped.command)?;
    let mut child = Command::new(&launch.program)
        .args(&launch.args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|source| WrappedCommandError::Spawn {
            program: launch.program.clone(),
            source,
        })?;

    let child_stdout = child.stdout.take().expect("stdout should be piped");
    let child_stderr = child.stderr.take().expect("stderr should be piped");
    let (sender, receiver) = mpsc::channel();
    let stdout_reader = spawn_capture_reader(CapturedStream::Stdout, child_stdout, sender.clone());
    let stderr_reader = spawn_capture_reader(CapturedStream::Stderr, child_stderr, sender);
    let agent = agent_kind_label(wrapped.agent).to_string();
    let mut monitor = Monitor::new(Config::default());

    for (stream, line) in receiver {
        let event = command_output_event(
            current_utc_timestamp(),
            agent.clone(),
            wrapped.session.clone(),
            stream,
            line.clone(),
        );
        store.append_event(&event)?;
        record_event_outcome_for_latest_advice(store, &event)?;
        let trigger_control_evaluation = wrapped_event_triggers_control_evaluation(store, &event)?;
        for intervention in monitor.ingest(event) {
            store.append_intervention(&intervention)?;
        }
        if trigger_control_evaluation {
            advise_workspace(store.workspace_root.clone())?;
        }
        match stream {
            CapturedStream::Stdout => {
                writeln!(stdout, "{line}").map_err(WrappedCommandError::Read)?;
            }
            CapturedStream::Stderr => {
                writeln!(stderr, "{line}").map_err(WrappedCommandError::Read)?;
            }
        }
    }

    join_capture_reader(stdout_reader)?;
    join_capture_reader(stderr_reader)?;

    let status = child.wait().map_err(WrappedCommandError::Read)?;
    let exit_code = status.code();
    let result_event = command_result_event(
        current_utc_timestamp(),
        agent,
        wrapped.session.clone(),
        wrapped.command.join(" "),
        exit_code,
    );
    store.append_event(&result_event)?;
    record_event_outcome_for_latest_advice(store, &result_event)?;
    if wrapped_event_triggers_control_evaluation(store, &result_event)? {
        advise_workspace(store.workspace_root.clone())?;
    }

    Ok(WrappedCommandResult { exit_code })
}

pub(crate) fn wrapped_event_triggers_control_evaluation(
    store: &ProjectStore,
    event: &Event,
) -> Result<bool, StoreError> {
    if event_is_low_signal_message_delta(event) {
        return Ok(false);
    }
    event_triggers_streaming_control_evaluation(store, event)
}

pub fn prepare_wrapped_launch(command: &[String]) -> Result<WrappedLaunch, WrappedCommandError> {
    let program = command
        .first()
        .ok_or(WrappedCommandError::EmptyCommand)?
        .clone();
    let args = command.iter().skip(1).cloned().collect::<Vec<_>>();
    prepare_wrapped_launch_parts(program, args)
}

#[cfg(windows)]
pub(crate) fn prepare_wrapped_launch_parts(
    program: String,
    args: Vec<String>,
) -> Result<WrappedLaunch, WrappedCommandError> {
    let program = resolve_windows_program(&program).unwrap_or(program);
    let extension = Path::new(&program)
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase);

    match extension.as_deref() {
        Some("cmd" | "bat") => {
            let mut launch_args = vec!["/C".into(), program];
            launch_args.extend(args);
            Ok(WrappedLaunch {
                program: "cmd.exe".into(),
                args: launch_args,
            })
        }
        Some("ps1") => {
            let mut launch_args = vec![
                "-NoProfile".into(),
                "-ExecutionPolicy".into(),
                "Bypass".into(),
                "-File".into(),
                program,
            ];
            launch_args.extend(args);
            Ok(WrappedLaunch {
                program: "powershell.exe".into(),
                args: launch_args,
            })
        }
        _ => Ok(WrappedLaunch { program, args }),
    }
}

#[cfg(windows)]
pub(crate) fn resolve_windows_program(program: &str) -> Option<String> {
    let path = Path::new(program);
    if path.is_absolute() || program.contains('\\') || program.contains('/') {
        return path.exists().then(|| program.to_string());
    }

    let extensions = std::env::var("PATHEXT")
        .ok()
        .map(|value| {
            value
                .split(';')
                .filter(|extension| !extension.is_empty())
                .map(|extension| extension.to_ascii_lowercase())
                .collect::<Vec<_>>()
        })
        .filter(|extensions| !extensions.is_empty())
        .unwrap_or_else(|| {
            vec![
                ".com".into(),
                ".exe".into(),
                ".bat".into(),
                ".cmd".into(),
                ".ps1".into(),
            ]
        });

    let has_extension = Path::new(program).extension().is_some();

    for dir in std::env::split_paths(&std::env::var_os("PATH")?) {
        if has_extension {
            let candidate = dir.join(program);
            if candidate.is_file() {
                return Some(candidate.display().to_string());
            }
        } else {
            for extension in &extensions {
                let candidate = dir.join(format!("{program}{extension}"));
                if candidate.is_file() {
                    return Some(candidate.display().to_string());
                }
            }
            let candidate = dir.join(program);
            if candidate.is_file() {
                return Some(candidate.display().to_string());
            }
        }
    }

    None
}

#[cfg(not(windows))]
pub(crate) fn prepare_wrapped_launch_parts(
    program: String,
    args: Vec<String>,
) -> Result<WrappedLaunch, WrappedCommandError> {
    Ok(WrappedLaunch { program, args })
}

pub(crate) fn spawn_capture_reader<R: Read + Send + 'static>(
    stream: CapturedStream,
    reader: R,
    sender: mpsc::Sender<(CapturedStream, String)>,
) -> thread::JoinHandle<Result<(), std::io::Error>> {
    thread::spawn(move || {
        // Read raw bytes and decode lossily rather than using `.lines()`, which
        // hard-errors on the first non-UTF-8 byte. Agent CLIs on Windows often
        // emit output in the OEM/ANSI code page, so strict UTF-8 would drop the
        // entire capture stream the moment a non-ASCII byte appeared.
        let mut buffered = BufReader::new(reader);
        let mut line = Vec::new();
        loop {
            line.clear();
            let read = buffered.read_until(b'\n', &mut line)?;
            if read == 0 {
                break;
            }
            while matches!(line.last(), Some(b'\n' | b'\r')) {
                line.pop();
            }
            let decoded = decode_console_line(&line);
            if sender.send((stream, decoded)).is_err() {
                break;
            }
        }
        Ok(())
    })
}

/// Decode one captured output line. Agent CLIs increasingly emit UTF-8, so try
/// that first; only fall back to the platform console code page when the bytes
/// are not valid UTF-8. This keeps modern UTF-8 output pristine while still
/// rendering legacy code-page output (e.g. GBK/CP936 on a Chinese Windows)
/// correctly instead of as replacement characters.
pub(crate) fn decode_console_line(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_string(),
        Err(_) => decode_console_code_page(bytes),
    }
}

#[cfg(windows)]
pub(crate) fn decode_console_code_page(bytes: &[u8]) -> String {
    use windows_sys::Win32::Globalization::{GetOEMCP, MultiByteToWideChar};

    if bytes.is_empty() {
        return String::new();
    }
    // Console subprocesses write in the OEM code page; decode against it.
    let code_page = unsafe { GetOEMCP() };
    let wide_len = unsafe {
        MultiByteToWideChar(
            code_page,
            0,
            bytes.as_ptr(),
            bytes.len() as i32,
            std::ptr::null_mut(),
            0,
        )
    };
    if wide_len <= 0 {
        return String::from_utf8_lossy(bytes).into_owned();
    }
    let mut wide = vec![0u16; wide_len as usize];
    let written = unsafe {
        MultiByteToWideChar(
            code_page,
            0,
            bytes.as_ptr(),
            bytes.len() as i32,
            wide.as_mut_ptr(),
            wide_len,
        )
    };
    if written <= 0 {
        return String::from_utf8_lossy(bytes).into_owned();
    }
    wide.truncate(written as usize);
    String::from_utf16_lossy(&wide)
}

#[cfg(not(windows))]
pub(crate) fn decode_console_code_page(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

pub(crate) fn join_capture_reader(
    handle: thread::JoinHandle<Result<(), std::io::Error>>,
) -> Result<(), WrappedCommandError> {
    handle
        .join()
        .map_err(|_| WrappedCommandError::CaptureThreadPanicked)?
        .map_err(WrappedCommandError::Read)
}
