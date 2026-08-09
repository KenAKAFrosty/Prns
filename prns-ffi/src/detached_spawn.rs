use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::Path;

use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{
    CloseHandle, SetHandleInformation, HANDLE, HANDLE_FLAG_INHERIT, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
use windows::Win32::System::Threading::{
    CreateProcessW, DeleteProcThreadAttributeList, GetExitCodeProcess,
    InitializeProcThreadAttributeList, UpdateProcThreadAttribute, WaitForSingleObject,
    CREATE_UNICODE_ENVIRONMENT, DETACHED_PROCESS, EXTENDED_STARTUPINFO_PRESENT,
    LPPROC_THREAD_ATTRIBUTE_LIST, PROCESS_INFORMATION, PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
    STARTF_USESTDHANDLES, STARTUPINFOEXW,
};

pub struct DetachedSpawn<'a> {
    pub binary: &'a Path,
    pub arguments: &'a [OsString],
    pub working_directory: &'a Path,
    pub environment: &'a [(OsString, OsString)],
    pub stdout: File,
    pub stderr: File,
}

pub struct DetachedChild {
    process: OwnedHandle,
    id: u32,
}

impl DetachedChild {
    pub fn id(&self) -> u32 {
        self.id
    }

    pub fn try_wait(&mut self) -> io::Result<Option<u32>> {
        let process = HANDLE(self.process.as_raw_handle());
        // SAFETY: `process` is the live process handle owned by `self.process` for the
        // lifetime of this child; a zero timeout makes this a non-blocking poll.
        let waited = unsafe { WaitForSingleObject(process, 0) };
        if waited == WAIT_TIMEOUT {
            return Ok(None);
        }
        if waited != WAIT_OBJECT_0 {
            return Err(io::Error::last_os_error());
        }
        let mut code = 0u32;
        // SAFETY: `process` is still owned and live, and `code` is a valid output slot for
        // the duration of this synchronous call.
        unsafe { GetExitCodeProcess(process, &mut code) }.map_err(io::Error::other)?;
        Ok(Some(code))
    }
}

pub fn spawn(specification: DetachedSpawn<'_>) -> io::Result<DetachedChild> {
    let stdin = File::open("NUL")?;
    let standard_handles = [
        HANDLE(stdin.as_raw_handle()),
        HANDLE(specification.stdout.as_raw_handle()),
        HANDLE(specification.stderr.as_raw_handle()),
    ];
    for handle in standard_handles {
        // SAFETY: each handle belongs to a `File` owned by this function or by
        // `specification`, all of which outlive the CreateProcessW call below; marking it
        // inheritable is required for the child to receive it.
        unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT.0, HANDLE_FLAG_INHERIT) }
            .map_err(io::Error::other)?;
    }

    let mut attribute_list_size = 0usize;
    // SAFETY: a null attribute list with a zeroed size is the documented sizing call; it
    // fails with ERROR_INSUFFICIENT_BUFFER by design and only writes `attribute_list_size`.
    let _ = unsafe {
        InitializeProcThreadAttributeList(
            LPPROC_THREAD_ATTRIBUTE_LIST(std::ptr::null_mut()),
            1,
            0,
            &mut attribute_list_size,
        )
    };
    if attribute_list_size == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut attribute_list_storage = vec![0u8; attribute_list_size];
    let attribute_list = LPPROC_THREAD_ATTRIBUTE_LIST(attribute_list_storage.as_mut_ptr().cast());
    // SAFETY: `attribute_list` points into `attribute_list_storage`, which is exactly the
    // size the sizing call reported and outlives every use of the list below.
    unsafe { InitializeProcThreadAttributeList(attribute_list, 1, 0, &mut attribute_list_size) }
        .map_err(io::Error::other)?;
    // SAFETY: the list was initialized for one attribute; `standard_handles` is a live array
    // that outlives the CreateProcessW call, and its byte length is passed alongside it.
    let updated = unsafe {
        UpdateProcThreadAttribute(
            attribute_list,
            0,
            PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
            Some(standard_handles.as_ptr().cast()),
            std::mem::size_of_val(&standard_handles),
            None,
            None,
        )
    };
    if let Err(error) = updated {
        // SAFETY: `attribute_list` was successfully initialized above and is deleted exactly
        // once on this early-exit path.
        unsafe { DeleteProcThreadAttributeList(attribute_list) };
        return Err(io::Error::other(error));
    }

    let application = wide_null(specification.binary.as_os_str());
    let mut command_line = build_command_line(specification.binary, specification.arguments);
    let environment_block = build_environment_block(specification.environment);
    let working_directory = wide_null(specification.working_directory.as_os_str());

    let mut startup = STARTUPINFOEXW::default();
    startup.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.StartupInfo.hStdInput = standard_handles[0];
    startup.StartupInfo.hStdOutput = standard_handles[1];
    startup.StartupInfo.hStdError = standard_handles[2];
    startup.lpAttributeList = attribute_list;

    let mut information = PROCESS_INFORMATION::default();
    // SAFETY: every pointer handed to CreateProcessW (application, mutable command line,
    // environment block, working directory, extended startup info, process information)
    // refers to a local that stays alive until the call returns, and the attribute list
    // restricts inheritance to `standard_handles`, which are inheritable and live.
    let created = unsafe {
        CreateProcessW(
            PCWSTR(application.as_ptr()),
            PWSTR(command_line.as_mut_ptr()),
            None,
            None,
            true,
            DETACHED_PROCESS | EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT,
            Some(environment_block.as_ptr().cast()),
            PCWSTR(working_directory.as_ptr()),
            &startup.StartupInfo,
            &mut information,
        )
    };
    // SAFETY: `attribute_list` was successfully initialized and is deleted exactly once now
    // that CreateProcessW no longer needs it.
    unsafe { DeleteProcThreadAttributeList(attribute_list) };
    created.map_err(io::Error::other)?;
    // SAFETY: CreateProcessW succeeded, so `hThread` is a live handle this process owns and
    // no longer needs.
    let _ = unsafe { CloseHandle(information.hThread) };
    // SAFETY: CreateProcessW succeeded, so `hProcess` is a live handle owned exclusively by
    // this process; OwnedHandle takes over closing it.
    let process = unsafe { OwnedHandle::from_raw_handle(information.hProcess.0) };
    Ok(DetachedChild {
        process,
        id: information.dwProcessId,
    })
}

fn wide_null(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain(std::iter::once(0)).collect()
}

fn build_command_line(binary: &Path, arguments: &[OsString]) -> Vec<u16> {
    let mut line = Vec::new();
    append_argument(&mut line, binary.as_os_str());
    for argument in arguments {
        line.push(u16::from(b' '));
        append_argument(&mut line, argument);
    }
    line.push(0);
    line
}

fn append_argument(line: &mut Vec<u16>, argument: &OsStr) {
    const QUOTE: u16 = b'"' as u16;
    const BACKSLASH: u16 = b'\\' as u16;
    let units: Vec<u16> = argument.encode_wide().collect();
    let quoting_needed = units.is_empty()
        || units
            .iter()
            .any(|&unit| unit == u16::from(b' ') || unit == u16::from(b'\t') || unit == QUOTE);
    if !quoting_needed {
        line.extend_from_slice(&units);
        return;
    }
    line.push(QUOTE);
    let mut backslashes = 0usize;
    for &unit in &units {
        if unit == BACKSLASH {
            backslashes += 1;
            continue;
        }
        if unit == QUOTE {
            line.extend(std::iter::repeat_n(BACKSLASH, backslashes * 2 + 1));
        } else {
            line.extend(std::iter::repeat_n(BACKSLASH, backslashes));
        }
        backslashes = 0;
        line.push(unit);
    }
    line.extend(std::iter::repeat_n(BACKSLASH, backslashes * 2));
    line.push(QUOTE);
}

fn build_environment_block(environment: &[(OsString, OsString)]) -> Vec<u16> {
    let mut sorted: Vec<&(OsString, OsString)> = environment.iter().collect();
    sorted.sort_by_key(|(key, _)| key.to_ascii_uppercase());
    let mut block = Vec::new();
    for (key, value) in sorted {
        block.extend(key.encode_wide());
        block.push(u16::from(b'='));
        block.extend(value.encode_wide());
        block.push(0);
    }
    block.push(0);
    if block.len() == 1 {
        block.push(0);
    }
    block
}

#[cfg(test)]
mod tests {
    use std::io::Read;
    use std::time::{Duration, Instant};

    use super::*;

    fn rendered(binary: &str, arguments: &[&str]) -> String {
        let arguments: Vec<OsString> = arguments.iter().map(OsString::from).collect();
        let mut line = build_command_line(Path::new(binary), &arguments);
        assert_eq!(line.pop(), Some(0));
        String::from_utf16(&line).expect("well-formed command line")
    }

    #[test]
    fn plain_arguments_pass_through_and_tricky_ones_are_quoted() {
        assert_eq!(rendered("prnsd.exe", &["run"]), "prnsd.exe run");
        assert_eq!(
            rendered(
                r"C:\Program Files\prnsd.exe",
                &["--config", r"C:\state dir"]
            ),
            r#""C:\Program Files\prnsd.exe" --config "C:\state dir""#
        );
        assert_eq!(rendered("bin", &[""]), r#"bin """#);
        assert_eq!(rendered("bin", &[r#"say "hi""#]), r#"bin "say \"hi\"""#);
        assert_eq!(
            rendered("bin", &[r"trailing\", "next"]),
            r#"bin trailing\ next"#
        );
        assert_eq!(rendered("bin", &[r"has space\"]), r#"bin "has space\\""#);
        assert_eq!(
            rendered("bin", &[r#"back\"quote"#]),
            r#"bin "back\\\"quote""#
        );
    }

    #[test]
    fn environment_blocks_are_sorted_and_double_terminated() {
        let environment = [
            (OsString::from("b"), OsString::from("2")),
            (OsString::from("A"), OsString::from("1")),
        ];
        let block = build_environment_block(&environment);
        let text = String::from_utf16(&block).expect("well-formed block");
        assert_eq!(text, "A=1\0b=2\0\0");
        assert_eq!(build_environment_block(&[]), vec![0, 0]);
    }

    fn scratch_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("prns-ffi-detached-{}-{name}", std::process::id()))
    }

    fn current_environment() -> Vec<(OsString, OsString)> {
        std::env::vars_os().collect()
    }

    fn comspec() -> OsString {
        std::env::var_os("ComSpec")
            .unwrap_or_else(|| OsString::from(r"C:\Windows\System32\cmd.exe"))
    }

    fn wait_for_exit(child: &mut DetachedChild, deadline: Duration) -> u32 {
        let started = Instant::now();
        loop {
            if let Some(code) = child.try_wait().expect("poll the child") {
                return code;
            }
            assert!(started.elapsed() < deadline, "child did not exit in time");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn a_detached_child_runs_with_redirected_output_and_reports_its_exit_code() {
        let stdout_path = scratch_path("stdout");
        let stderr_path = scratch_path("stderr");
        let arguments = [OsString::from("/c"), OsString::from("echo marker& exit 7")];
        let mut child = spawn(DetachedSpawn {
            binary: Path::new(&comspec()),
            arguments: &arguments,
            working_directory: &std::env::temp_dir(),
            environment: &current_environment(),
            stdout: File::create(&stdout_path).expect("stdout capture"),
            stderr: File::create(&stderr_path).expect("stderr capture"),
        })
        .expect("spawn cmd");
        assert_ne!(child.id(), 0);
        assert_eq!(wait_for_exit(&mut child, Duration::from_secs(10)), 7);
        let captured = std::fs::read_to_string(&stdout_path).expect("captured stdout");
        assert!(captured.contains("marker"));
        let _ = std::fs::remove_file(stdout_path);
        let _ = std::fs::remove_file(stderr_path);
    }

    #[test]
    fn stray_inheritable_handles_are_not_leaked_into_the_child() {
        let (mut reader, writer) = std::io::pipe().expect("anonymous pipe");
        let writer_handle = HANDLE(writer.as_raw_handle());
        // SAFETY: `writer_handle` is the live pipe writer owned by `writer` until it is
        // dropped below; marking it inheritable recreates the stray-handle hazard.
        unsafe { SetHandleInformation(writer_handle, HANDLE_FLAG_INHERIT.0, HANDLE_FLAG_INHERIT) }
            .expect("mark the pipe writer inheritable");

        let stdout_path = scratch_path("leak-stdout");
        let stderr_path = scratch_path("leak-stderr");
        let arguments = [
            OsString::from("/c"),
            OsString::from("ping -n 4 127.0.0.1 >nul"),
        ];
        let mut child = spawn(DetachedSpawn {
            binary: Path::new(&comspec()),
            arguments: &arguments,
            working_directory: &std::env::temp_dir(),
            environment: &current_environment(),
            stdout: File::create(&stdout_path).expect("stdout capture"),
            stderr: File::create(&stderr_path).expect("stderr capture"),
        })
        .expect("spawn a lingering child");

        drop(writer);
        let mut sink = Vec::new();
        let read_bytes = reader.read_to_end(&mut sink).expect("pipe drain");
        assert_eq!(
            read_bytes, 0,
            "the child must not hold the stray pipe writer"
        );
        assert!(
            child.try_wait().expect("poll the child").is_none(),
            "end-of-pipe must arrive while the child is still running"
        );

        wait_for_exit(&mut child, Duration::from_secs(15));
        let _ = std::fs::remove_file(stdout_path);
        let _ = std::fs::remove_file(stderr_path);
    }
}
