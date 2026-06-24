use coding_agent_monitor::{
    CapturedStream, EventKind, command_output_event, command_result_event, prepare_wrapped_launch,
};

/// Serializes tests that mutate the process-global `PATH`. Without this, the
/// default parallel test runner lets two PATH-rewriting tests interleave, so
/// one test's temporary directory leaks into the other's resolution and the
/// assertions flake.
#[cfg(windows)]
static PATH_GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn command_output_event_preserves_stream_agent_and_session() {
    let event = command_output_event(
        Some("2026-06-22T12:00:00Z".into()),
        "codex",
        Some("session-1".into()),
        CapturedStream::Stderr,
        "rate limit while streaming",
    );

    assert_eq!(event.kind, EventKind::CommandOutput);
    assert_eq!(event.agent, "codex");
    assert_eq!(event.session.as_deref(), Some("session-1"));
    assert_eq!(event.agent_session_id.as_deref(), Some("session-1"));
    assert_eq!(event.stream.as_deref(), Some("stderr"));
    assert_eq!(event.content.as_deref(), Some("rate limit while streaming"));
}

#[test]
fn command_result_event_records_command_and_exit_code() {
    let event = command_result_event(
        Some("2026-06-22T12:00:03Z".into()),
        "claude-code",
        Some("session-2".into()),
        "claude --continue",
        Some(7),
    );

    assert_eq!(event.kind, EventKind::CommandResult);
    assert_eq!(event.agent, "claude-code");
    assert_eq!(event.session.as_deref(), Some("session-2"));
    assert_eq!(event.agent_session_id.as_deref(), Some("session-2"));
    assert_eq!(event.command.as_deref(), Some("claude --continue"));
    assert_eq!(event.exit_code, Some(7));
}

#[cfg(windows)]
#[test]
fn windows_launch_resolution_uses_cmd_for_cmd_shims() {
    let command = vec!["C:\\tools\\codex.cmd".to_string(), "exec".to_string()];
    let launch = prepare_wrapped_launch(&command).expect("prepare launch");

    assert_eq!(launch.program.to_ascii_lowercase(), "cmd.exe");
    assert_eq!(launch.args[0], "/C");
    assert_eq!(launch.args[1], "C:\\tools\\codex.cmd");
    assert_eq!(launch.args[2], "exec");
}

#[cfg(windows)]
#[test]
fn windows_launch_resolution_uses_powershell_for_ps1_shims() {
    let command = vec!["C:\\tools\\codex.ps1".to_string(), "exec".to_string()];
    let launch = prepare_wrapped_launch(&command).expect("prepare launch");

    assert_eq!(launch.program.to_ascii_lowercase(), "powershell.exe");
    assert!(launch.args.contains(&"-File".to_string()));
    assert!(launch.args.contains(&"C:\\tools\\codex.ps1".to_string()));
}

#[cfg(windows)]
#[test]
fn windows_launch_resolution_finds_cmd_shim_on_path() {
    let _guard = PATH_GUARD
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().expect("temp dir");
    let shim = temp.path().join("codex.cmd");
    std::fs::write(&shim, "@echo off\r\n").expect("write shim");
    let previous_path = std::env::var_os("PATH");
    let new_path = match previous_path.as_ref() {
        Some(path) => {
            let mut paths = std::env::split_paths(path).collect::<Vec<_>>();
            paths.insert(0, temp.path().to_path_buf());
            std::env::join_paths(paths).expect("join path")
        }
        None => temp.path().as_os_str().to_os_string(),
    };

    unsafe {
        std::env::set_var("PATH", &new_path);
    }
    let launch = prepare_wrapped_launch(&["codex".to_string()]).expect("prepare launch");
    restore_path(previous_path);

    assert_eq!(launch.program.to_ascii_lowercase(), "cmd.exe");
    assert_eq!(launch.args[1], shim.display().to_string());
}

#[cfg(windows)]
#[test]
fn windows_launch_resolution_prefers_cmd_shim_over_extensionless_script() {
    let _guard = PATH_GUARD
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let temp = tempfile::tempdir().expect("temp dir");
    let extensionless = temp.path().join("codex");
    let shim = temp.path().join("codex.cmd");
    std::fs::write(&extensionless, "#!/usr/bin/env node\r\n").expect("write extensionless");
    std::fs::write(&shim, "@echo off\r\n").expect("write shim");
    let previous_path = std::env::var_os("PATH");
    let new_path = match previous_path.as_ref() {
        Some(path) => {
            let mut paths = std::env::split_paths(path).collect::<Vec<_>>();
            paths.insert(0, temp.path().to_path_buf());
            std::env::join_paths(paths).expect("join path")
        }
        None => temp.path().as_os_str().to_os_string(),
    };

    unsafe {
        std::env::set_var("PATH", &new_path);
    }
    let launch = prepare_wrapped_launch(&["codex".to_string()]).expect("prepare launch");
    restore_path(previous_path);

    assert_eq!(launch.program.to_ascii_lowercase(), "cmd.exe");
    assert_eq!(launch.args[1], shim.display().to_string());
}

#[cfg(windows)]
fn restore_path(previous_path: Option<std::ffi::OsString>) {
    unsafe {
        if let Some(path) = previous_path {
            std::env::set_var("PATH", path);
        } else {
            std::env::remove_var("PATH");
        }
    }
}
