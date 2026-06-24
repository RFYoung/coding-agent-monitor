use crate::outcome_recording::record_verifier_outcome_for_latest_advice;
use crate::*;
use std::io::Read;
use std::path::Path;
use std::process::ExitStatus;
#[cfg(not(windows))]
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

pub fn run_verifier(
    workspace: impl AsRef<Path>,
    verifier_id: &str,
) -> Result<VerifierRun, VerifyError> {
    let workspace = workspace.as_ref();
    let mut store = ProjectStore::open(workspace)?;
    let config = ProjectConfig::load(store.root())?;
    let verifier = config
        .verifiers
        .iter()
        .find(|verifier| verifier.id == verifier_id)
        .cloned()
        .ok_or_else(|| VerifyError::UnknownVerifier(verifier_id.into()))?;

    let started_at = current_utc_timestamp().unwrap_or_else(|| "1970-01-01T00:00:00Z".into());
    let output = run_verifier_command(workspace, &verifier)?;
    let completed_at = current_utc_timestamp();
    let mut combined_output = Vec::new();
    combined_output.extend_from_slice(&output.stdout);
    combined_output.extend_from_slice(&output.stderr);
    let run = VerifierRun {
        verifier_run_id: format!("verifier-run-{}", current_id_fragment()),
        verifier_id: Some(verifier.id),
        command: verifier.command,
        status: if output.timed_out {
            VerificationRunStatus::TimedOut
        } else if output.status.is_some_and(|status| status.success()) {
            VerificationRunStatus::Passed
        } else {
            VerificationRunStatus::Failed
        },
        started_at,
        completed_at,
        exit_code: if output.timed_out {
            None
        } else {
            output.status.and_then(|status| status.code())
        },
        output_digest: fnv1a64_digest(&combined_output),
        failure_class: classify_verifier_failure(output.timed_out, output.status, &combined_output),
    };
    store.append_verifier_run(&run)?;
    record_verifier_outcome_for_latest_advice(&mut store, &run)?;
    Ok(run)
}

fn classify_verifier_failure(
    timed_out: bool,
    status: Option<ExitStatus>,
    combined_output: &[u8],
) -> Option<VerificationFailureClass> {
    if timed_out {
        return Some(VerificationFailureClass::Timeout);
    }
    if status.is_some_and(|status| status.success()) {
        return None;
    }

    let output = String::from_utf8_lossy(combined_output).to_lowercase();
    if output.contains("error[e")
        || output.contains("mismatched types")
        || output.contains("cannot find")
        || output.contains("unresolved import")
        || output.contains("compilation failed")
        || output.contains("compile error")
        || output.contains("syntax error")
        || output.contains("ts")
            && output.contains("error")
            && (output.contains("is not assignable") || output.contains("cannot find name"))
    {
        return Some(VerificationFailureClass::Compile);
    }

    if output.contains("assertion failed")
        || output.contains("assert_eq")
        || output.contains("assert_ne")
        || output.contains("panicked at")
        || output.contains("expected")
            && output.contains("actual")
            && (output.contains("test") || output.contains("failed"))
        || output.contains("test result: failed")
        || output.contains("failures:")
    {
        return Some(VerificationFailureClass::Assertion);
    }

    if output.contains("connection refused")
        || output.contains("connection reset")
        || output.contains("service unavailable")
        || output.contains("timed out")
        || output.contains("timeout")
        || output.contains("econnrefused")
        || output.contains("enotfound")
        || output.contains("network")
        || output.contains("database")
            && (output.contains("unavailable")
                || output.contains("refused")
                || output.contains("could not connect"))
        || output.contains("permission denied")
        || output.contains("access is denied")
    {
        return Some(VerificationFailureClass::Environment);
    }

    if output.contains("no tests ran")
        || output.contains("0 tests")
        || output.contains("collected 0 items")
        || output.contains("no test files found")
        || output.contains("coverage")
            && (output.contains("threshold") || output.contains("not met"))
    {
        return Some(VerificationFailureClass::CoverageGap);
    }

    Some(VerificationFailureClass::Unknown)
}

struct VerifierCommandOutput {
    status: Option<ExitStatus>,
    timed_out: bool,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

#[cfg(windows)]
fn run_verifier_command(
    workspace: &Path,
    verifier: &VerifierConfig,
) -> Result<VerifierCommandOutput, VerifyError> {
    let job = WindowsVerifierJob::create()?;
    let (stdin_read, stdin_write) = windows_pipe()?;
    let (stdout_read, stdout_write) = windows_pipe()?;
    let (stderr_read, stderr_write) = windows_pipe()?;
    windows_make_non_inheritable(stdin_write.raw())?;
    windows_make_non_inheritable(stdout_read.raw())?;
    windows_make_non_inheritable(stderr_read.raw())?;

    let process = WindowsVerifierProcess::spawn(
        workspace,
        &verifier.command,
        &job,
        stdin_read.raw(),
        stdout_write.raw(),
        stderr_write.raw(),
    )?;

    drop(stdin_read);
    drop(stdin_write);
    drop(stdout_write);
    drop(stderr_write);

    let stdout_reader = Some(spawn_output_reader(stdout_read.into_file()));
    let stderr_reader = Some(spawn_output_reader(stderr_read.into_file()));
    let timeout = Duration::from_secs(verifier.timeout_secs.max(1));
    let deadline = Instant::now() + timeout;
    let mut timed_out = false;
    let mut status = None;

    loop {
        if status.is_none() {
            status = process.try_wait()?;
        }

        if status.is_some() && output_readers_finished(&stdout_reader, &stderr_reader) {
            break;
        }

        if Instant::now() >= deadline {
            timed_out = true;
            job.terminate()?;
            if status.is_none() {
                status = Some(process.wait()?);
            }
            break;
        }

        thread::sleep(Duration::from_millis(25));
    }

    Ok(VerifierCommandOutput {
        status,
        timed_out,
        stdout: collect_output_reader(stdout_reader)?,
        stderr: collect_output_reader(stderr_reader)?,
    })
}

#[cfg(not(windows))]
fn run_verifier_command(
    workspace: &Path,
    verifier: &VerifierConfig,
) -> Result<VerifierCommandOutput, VerifyError> {
    let mut command = verifier_command(&verifier.command);
    command
        .current_dir(workspace)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = command.spawn().map_err(VerifyError::Spawn)?;
    let stdout_reader = child.stdout.take().map(spawn_output_reader);
    let stderr_reader = child.stderr.take().map(spawn_output_reader);
    let process_group = match VerifierProcessGroup::attach(&mut child) {
        Ok(process_group) => process_group,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
    };
    let timeout = Duration::from_secs(verifier.timeout_secs.max(1));
    let deadline = Instant::now() + timeout;
    let mut timed_out = false;
    let mut status = None;

    loop {
        if status.is_none() {
            status = child.try_wait().map_err(VerifyError::Wait)?;
        }

        if status.is_some() && output_readers_finished(&stdout_reader, &stderr_reader) {
            break;
        }

        if Instant::now() >= deadline {
            timed_out = true;
            process_group.kill(&mut child)?;
            if status.is_none() {
                status = Some(child.wait().map_err(VerifyError::Wait)?);
            }
            break;
        }

        thread::sleep(Duration::from_millis(25));
    }

    Ok(VerifierCommandOutput {
        status,
        timed_out,
        stdout: collect_output_reader(stdout_reader)?,
        stderr: collect_output_reader(stderr_reader)?,
    })
}

#[cfg(windows)]
struct WindowsHandle(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl WindowsHandle {
    fn new(handle: windows_sys::Win32::Foundation::HANDLE) -> Result<Self, VerifyError> {
        if handle.is_null() || handle == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
            Err(VerifyError::ProcessGroup(std::io::Error::last_os_error()))
        } else {
            Ok(Self(handle))
        }
    }

    fn raw(&self) -> windows_sys::Win32::Foundation::HANDLE {
        self.0
    }

    fn take(&mut self) -> windows_sys::Win32::Foundation::HANDLE {
        let handle = self.0;
        self.0 = std::ptr::null_mut();
        handle
    }

    fn into_file(mut self) -> std::fs::File {
        use std::os::windows::io::FromRawHandle;

        let handle = self.take();
        unsafe { std::fs::File::from_raw_handle(handle.cast()) }
    }
}

#[cfg(windows)]
impl Drop for WindowsHandle {
    fn drop(&mut self) {
        unsafe {
            if !self.0.is_null() && self.0 != windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
                windows_sys::Win32::Foundation::CloseHandle(self.0);
            }
        }
    }
}

#[cfg(windows)]
struct WindowsVerifierJob {
    handle: WindowsHandle,
}

#[cfg(windows)]
impl WindowsVerifierJob {
    fn create() -> Result<Self, VerifyError> {
        use std::ffi::c_void;
        use std::mem::{size_of, zeroed};
        use std::ptr::null;
        use windows_sys::Win32::System::JobObjects::{
            CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
            SetInformationJobObject,
        };

        let handle = WindowsHandle::new(unsafe { CreateJobObjectW(null(), null()) })?;
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let configured = unsafe {
            SetInformationJobObject(
                handle.raw(),
                JobObjectExtendedLimitInformation,
                &limits as *const _ as *const c_void,
                size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if configured == 0 {
            return Err(VerifyError::ProcessGroup(std::io::Error::last_os_error()));
        }

        Ok(Self { handle })
    }

    fn raw(&self) -> windows_sys::Win32::Foundation::HANDLE {
        self.handle.raw()
    }

    fn terminate(&self) -> Result<(), VerifyError> {
        use windows_sys::Win32::System::JobObjects::TerminateJobObject;

        if unsafe { TerminateJobObject(self.raw(), 1) } == 0 {
            Err(VerifyError::Kill(std::io::Error::last_os_error()))
        } else {
            Ok(())
        }
    }
}

#[cfg(windows)]
struct WindowsAttributeList {
    bytes: Vec<u8>,
    job_list: Box<[windows_sys::Win32::Foundation::HANDLE; 1]>,
}

#[cfg(windows)]
impl WindowsAttributeList {
    fn new(job: &WindowsVerifierJob) -> Result<Self, VerifyError> {
        use std::ffi::c_void;
        use std::ptr::{null, null_mut};
        use windows_sys::Win32::System::Threading::{
            InitializeProcThreadAttributeList, PROC_THREAD_ATTRIBUTE_JOB_LIST,
            UpdateProcThreadAttribute,
        };

        let mut size = 0usize;
        unsafe {
            InitializeProcThreadAttributeList(null_mut(), 1, 0, &mut size);
        }
        if size == 0 {
            return Err(VerifyError::ProcessGroup(std::io::Error::last_os_error()));
        }

        let mut bytes = vec![0u8; size];
        let attribute_list = bytes.as_mut_ptr()
            as windows_sys::Win32::System::Threading::LPPROC_THREAD_ATTRIBUTE_LIST;
        if unsafe { InitializeProcThreadAttributeList(attribute_list, 1, 0, &mut size) } == 0 {
            return Err(VerifyError::ProcessGroup(std::io::Error::last_os_error()));
        }

        let mut job_list = Box::new([job.raw()]);
        let updated = unsafe {
            UpdateProcThreadAttribute(
                attribute_list,
                0,
                PROC_THREAD_ATTRIBUTE_JOB_LIST as usize,
                job_list.as_mut_ptr() as *const c_void,
                std::mem::size_of::<windows_sys::Win32::Foundation::HANDLE>(),
                null_mut(),
                null(),
            )
        };
        if updated == 0 {
            unsafe {
                windows_sys::Win32::System::Threading::DeleteProcThreadAttributeList(
                    attribute_list,
                );
            }
            return Err(VerifyError::ProcessGroup(std::io::Error::last_os_error()));
        }

        Ok(Self { bytes, job_list })
    }

    fn raw(&mut self) -> windows_sys::Win32::System::Threading::LPPROC_THREAD_ATTRIBUTE_LIST {
        self.bytes.as_mut_ptr()
            as windows_sys::Win32::System::Threading::LPPROC_THREAD_ATTRIBUTE_LIST
    }
}

#[cfg(windows)]
impl Drop for WindowsAttributeList {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::System::Threading::DeleteProcThreadAttributeList(self.raw());
        }
        let _ = self.job_list[0];
    }
}

#[cfg(windows)]
struct WindowsVerifierProcess {
    process: WindowsHandle,
}

#[cfg(windows)]
impl WindowsVerifierProcess {
    fn spawn(
        workspace: &Path,
        command: &str,
        job: &WindowsVerifierJob,
        stdin: windows_sys::Win32::Foundation::HANDLE,
        stdout: windows_sys::Win32::Foundation::HANDLE,
        stderr: windows_sys::Win32::Foundation::HANDLE,
    ) -> Result<Self, VerifyError> {
        use std::ffi::c_void;
        use std::mem::{size_of, zeroed};
        use std::os::windows::ffi::OsStrExt;
        use std::ptr::null;
        use windows_sys::Win32::Foundation::TRUE;
        use windows_sys::Win32::System::Threading::{
            CREATE_NO_WINDOW, CreateProcessW, EXTENDED_STARTUPINFO_PRESENT, PROCESS_INFORMATION,
            STARTF_USESTDHANDLES, STARTUPINFOEXW,
        };

        let mut attributes = WindowsAttributeList::new(job)?;
        let mut startup: STARTUPINFOEXW = unsafe { zeroed() };
        startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
        startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
        startup.StartupInfo.hStdInput = stdin;
        startup.StartupInfo.hStdOutput = stdout;
        startup.StartupInfo.hStdError = stderr;
        startup.lpAttributeList = attributes.raw();

        let mut process_info: PROCESS_INFORMATION = unsafe { zeroed() };
        let mut command_line = windows_verifier_command_line(command);
        let current_directory = workspace
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect::<Vec<_>>();
        let created = unsafe {
            CreateProcessW(
                null(),
                command_line.as_mut_ptr(),
                null(),
                null(),
                TRUE,
                EXTENDED_STARTUPINFO_PRESENT | CREATE_NO_WINDOW,
                null::<c_void>(),
                current_directory.as_ptr(),
                &startup as *const STARTUPINFOEXW as *const _,
                &mut process_info,
            )
        };
        if created == 0 {
            return Err(VerifyError::Spawn(std::io::Error::last_os_error()));
        }

        let process = WindowsHandle::new(process_info.hProcess)?;
        drop(WindowsHandle::new(process_info.hThread)?);
        Ok(Self { process })
    }

    fn try_wait(&self) -> Result<Option<ExitStatus>, VerifyError> {
        use std::os::windows::process::ExitStatusExt;
        use windows_sys::Win32::Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT};
        use windows_sys::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};

        match unsafe { WaitForSingleObject(self.process.raw(), 0) } {
            WAIT_TIMEOUT => Ok(None),
            WAIT_OBJECT_0 => {
                let mut code = 0u32;
                if unsafe { GetExitCodeProcess(self.process.raw(), &mut code) } == 0 {
                    Err(VerifyError::Wait(std::io::Error::last_os_error()))
                } else {
                    Ok(Some(ExitStatus::from_raw(code)))
                }
            }
            _ => Err(VerifyError::Wait(std::io::Error::last_os_error())),
        }
    }

    fn wait(&self) -> Result<ExitStatus, VerifyError> {
        use std::os::windows::process::ExitStatusExt;
        use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
        use windows_sys::Win32::System::Threading::{
            GetExitCodeProcess, INFINITE, WaitForSingleObject,
        };

        if unsafe { WaitForSingleObject(self.process.raw(), INFINITE) } != WAIT_OBJECT_0 {
            return Err(VerifyError::Wait(std::io::Error::last_os_error()));
        }
        let mut code = 0u32;
        if unsafe { GetExitCodeProcess(self.process.raw(), &mut code) } == 0 {
            Err(VerifyError::Wait(std::io::Error::last_os_error()))
        } else {
            Ok(ExitStatus::from_raw(code))
        }
    }
}

#[cfg(windows)]
fn windows_pipe() -> Result<(WindowsHandle, WindowsHandle), VerifyError> {
    use std::mem::{size_of, zeroed};
    use std::ptr::null_mut;
    use windows_sys::Win32::Foundation::TRUE;
    use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
    use windows_sys::Win32::System::Pipes::CreatePipe;

    let mut security: SECURITY_ATTRIBUTES = unsafe { zeroed() };
    security.nLength = size_of::<SECURITY_ATTRIBUTES>() as u32;
    security.lpSecurityDescriptor = null_mut();
    security.bInheritHandle = TRUE;

    let mut read = null_mut();
    let mut write = null_mut();
    if unsafe { CreatePipe(&mut read, &mut write, &security, 0) } == 0 {
        return Err(VerifyError::Spawn(std::io::Error::last_os_error()));
    }
    Ok((WindowsHandle::new(read)?, WindowsHandle::new(write)?))
}

#[cfg(windows)]
fn windows_make_non_inheritable(
    handle: windows_sys::Win32::Foundation::HANDLE,
) -> Result<(), VerifyError> {
    use windows_sys::Win32::Foundation::{HANDLE_FLAG_INHERIT, SetHandleInformation};

    if unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0) } == 0 {
        Err(VerifyError::Spawn(std::io::Error::last_os_error()))
    } else {
        Ok(())
    }
}

#[cfg(windows)]
fn windows_verifier_command_line(command: &str) -> Vec<u16> {
    [
        "powershell.exe",
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy",
        "Bypass",
        "-Command",
        command,
    ]
    .iter()
    .map(|arg| quote_windows_arg(arg))
    .collect::<Vec<_>>()
    .join(" ")
    .encode_utf16()
    .chain(std::iter::once(0))
    .collect()
}

#[cfg(windows)]
fn quote_windows_arg(arg: &str) -> String {
    if !arg.is_empty() && !arg.chars().any(|ch| ch == ' ' || ch == '\t' || ch == '"') {
        return arg.into();
    }

    let mut quoted = String::from("\"");
    let mut backslashes = 0usize;
    for ch in arg.chars() {
        match ch {
            '\\' => backslashes += 1,
            '"' => {
                quoted.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
                quoted.push('"');
                backslashes = 0;
            }
            _ => {
                quoted.extend(std::iter::repeat_n('\\', backslashes));
                backslashes = 0;
                quoted.push(ch);
            }
        }
    }
    quoted.extend(std::iter::repeat_n('\\', backslashes * 2));
    quoted.push('"');
    quoted
}

#[cfg(unix)]
struct VerifierProcessGroup {
    process_group_id: i32,
}

#[cfg(unix)]
impl VerifierProcessGroup {
    fn attach(child: &mut Child) -> Result<Self, VerifyError> {
        Ok(Self {
            process_group_id: child.id() as i32,
        })
    }

    fn kill(&self, child: &mut Child) -> Result<(), VerifyError> {
        if self.kill_process_group() {
            return Ok(());
        }

        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ESRCH) {
            Ok(())
        } else {
            child.kill().map_err(VerifyError::Kill)
        }
    }

    fn kill_process_group(&self) -> bool {
        (unsafe { libc::kill(-self.process_group_id, libc::SIGKILL) }) == 0
    }
}

#[cfg(unix)]
impl Drop for VerifierProcessGroup {
    fn drop(&mut self) {
        let _ = self.kill_process_group();
    }
}

#[cfg(all(not(windows), not(unix)))]
struct VerifierProcessGroup;

#[cfg(all(not(windows), not(unix)))]
impl VerifierProcessGroup {
    fn attach(_child: &mut Child) -> Result<Self, VerifyError> {
        Ok(Self)
    }

    fn kill(&self, child: &mut Child) -> Result<(), VerifyError> {
        match child.kill() {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::InvalidInput => Ok(()),
            Err(error) => Err(VerifyError::Kill(error)),
        }
    }
}

fn spawn_output_reader<R>(mut reader: R) -> thread::JoinHandle<std::io::Result<Vec<u8>>>
where
    R: Read + Send + 'static,
{
    thread::spawn(move || {
        let mut output = Vec::new();
        reader.read_to_end(&mut output)?;
        Ok(output)
    })
}

fn collect_output_reader(
    reader: Option<thread::JoinHandle<std::io::Result<Vec<u8>>>>,
) -> Result<Vec<u8>, VerifyError> {
    match reader {
        Some(reader) => reader
            .join()
            .map_err(|_| VerifyError::OutputReaderPanicked)?
            .map_err(VerifyError::ReadOutput),
        None => Ok(Vec::new()),
    }
}

fn output_readers_finished(
    stdout_reader: &Option<thread::JoinHandle<std::io::Result<Vec<u8>>>>,
    stderr_reader: &Option<thread::JoinHandle<std::io::Result<Vec<u8>>>>,
) -> bool {
    stdout_reader
        .as_ref()
        .is_none_or(thread::JoinHandle::is_finished)
        && stderr_reader
            .as_ref()
            .is_none_or(thread::JoinHandle::is_finished)
}

#[cfg(unix)]
fn verifier_command(command: &str) -> Command {
    use std::os::unix::process::CommandExt;

    let mut shell = Command::new("sh");
    shell.args(["-c", command]);
    shell.process_group(0);
    shell
}

#[cfg(all(not(windows), not(unix)))]
fn verifier_command(command: &str) -> Command {
    let mut shell = Command::new("sh");
    shell.args(["-c", command]);
    shell
}
