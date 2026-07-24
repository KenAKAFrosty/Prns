use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
#[cfg(windows)]
use std::os::windows::process::CommandExt;

static SCRIPT_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TrayAction {
    OpenTerminal,
    ShowStatus,
    ManageInterfaces,
    OpenConfigDirectory,
}

impl TrayAction {
    pub(crate) const fn event_name(self) -> &'static str {
        match self {
            Self::OpenTerminal => "open_terminal",
            Self::ShowStatus => "show_status",
            Self::ManageInterfaces => "manage_interfaces",
            Self::OpenConfigDirectory => "open_config_directory",
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct TrayActionContext {
    binary: PathBuf,
    config_dir: PathBuf,
    managed_state_dir: Option<PathBuf>,
}

impl TrayActionContext {
    pub(crate) fn discover(
        config_dir: PathBuf,
        managed_state_dir: Option<PathBuf>,
    ) -> Result<Self, TrayActionError> {
        let binary = std::env::current_exe().map_err(TrayActionError::CurrentExecutable)?;
        Ok(Self {
            binary,
            config_dir,
            managed_state_dir,
        })
    }

    #[cfg(test)]
    fn new_for_test(
        binary: impl Into<PathBuf>,
        config_dir: impl Into<PathBuf>,
        managed_state_dir: Option<PathBuf>,
    ) -> Self {
        Self {
            binary: binary.into(),
            config_dir: config_dir.into(),
            managed_state_dir,
        }
    }

    pub(crate) const fn can_attach_terminal(&self) -> bool {
        self.managed_state_dir.is_some()
    }

    pub(crate) fn perform(&self, action: TrayAction) -> Result<(), TrayActionError> {
        match action {
            TrayAction::OpenTerminal => {
                if !self.can_attach_terminal() {
                    return Err(TrayActionError::ForegroundSession);
                }
                self.open_terminal(TerminalCommand::Attach)
            }
            TrayAction::ShowStatus => self.open_terminal(TerminalCommand::Status),
            TrayAction::ManageInterfaces => self.open_terminal(TerminalCommand::Interfaces),
            TrayAction::OpenConfigDirectory => open_directory(&self.config_dir),
        }
    }

    fn command_arguments(&self, command: TerminalCommand) -> Vec<OsString> {
        match command {
            TerminalCommand::Attach => Vec::new(),
            TerminalCommand::Status => vec![
                OsString::from("status"),
                OsString::from("--config"),
                self.config_dir.as_os_str().to_owned(),
            ],
            TerminalCommand::Interfaces => vec![
                OsString::from("interfaces"),
                OsString::from("--config"),
                self.config_dir.as_os_str().to_owned(),
            ],
        }
    }

    fn open_terminal(&self, command: TerminalCommand) -> Result<(), TrayActionError> {
        let script = write_terminal_script(self, command)?;
        match launch_terminal(&script) {
            Ok(()) => Ok(()),
            Err(error) => {
                let _ = fs::remove_file(&script);
                Err(error)
            }
        }
    }
}

#[derive(Debug)]
pub(crate) enum TrayActionError {
    CurrentExecutable(io::Error),
    ForegroundSession,
    TemporaryScript(io::Error),
    ScriptWrite(io::Error),
    ScriptPermissions(io::Error),
    Launch {
        program: &'static str,
        source: io::Error,
    },
    #[cfg(target_os = "macos")]
    LaunchExit {
        program: &'static str,
        status: std::process::ExitStatus,
    },
    #[cfg(all(unix, not(target_os = "macos")))]
    NoTerminal,
}

impl fmt::Display for TrayActionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::CurrentExecutable(error) => {
                write!(
                    formatter,
                    "could not locate the running prnsd executable: {error}"
                )
            }
            Self::ForegroundSession => {
                formatter.write_str("terminal attachment is unavailable for a foreground session")
            }
            Self::TemporaryScript(error) => {
                write!(
                    formatter,
                    "could not create a private terminal launcher: {error}"
                )
            }
            Self::ScriptWrite(error) => {
                write!(formatter, "could not write the terminal launcher: {error}")
            }
            Self::ScriptPermissions(error) => {
                write!(
                    formatter,
                    "could not make the terminal launcher executable: {error}"
                )
            }
            Self::Launch { program, source } => {
                write!(formatter, "could not launch {program}: {source}")
            }
            #[cfg(target_os = "macos")]
            Self::LaunchExit { program, status } => {
                write!(formatter, "{program} exited with {status}")
            }
            #[cfg(all(unix, not(target_os = "macos")))]
            Self::NoTerminal => {
                formatter.write_str("no supported desktop terminal emulator was found")
            }
        }
    }
}

impl std::error::Error for TrayActionError {}

#[derive(Debug, Clone, Copy)]
enum TerminalCommand {
    Attach,
    Status,
    Interfaces,
}

impl TerminalCommand {
    const fn introduction(self) -> &'static str {
        match self {
            Self::Attach => {
                "Attaching to prnsd. Press Ctrl-C to detach while leaving the daemon running."
            }
            Self::Status => "Reading live Reticulum network status from prnsd.",
            Self::Interfaces => "Opening the guided Reticulum interface editor.",
        }
    }

    const fn slug(self) -> &'static str {
        match self {
            Self::Attach => "attach",
            Self::Status => "status",
            Self::Interfaces => "interfaces",
        }
    }
}

fn unique_script_path(extension: &str, command: TerminalCommand) -> PathBuf {
    let id = SCRIPT_ID.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "prnsd-tray-{}-{id}-{}.{}",
        std::process::id(),
        command.slug(),
        extension
    ))
}

#[cfg(unix)]
fn write_terminal_script(
    context: &TrayActionContext,
    command: TerminalCommand,
) -> Result<PathBuf, TrayActionError> {
    let path = unique_script_path("command", command);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o700)
        .open(&path)
        .map_err(TrayActionError::TemporaryScript)?;
    let script = posix_terminal_script(context, command);
    file.write_all(&script)
        .map_err(TrayActionError::ScriptWrite)?;
    file.flush().map_err(TrayActionError::ScriptWrite)?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700))
        .map_err(TrayActionError::ScriptPermissions)?;
    Ok(path)
}

#[cfg(unix)]
fn posix_terminal_script(context: &TrayActionContext, command: TerminalCommand) -> Vec<u8> {
    let mut script = b"#!/bin/sh\nclear\nprintf '%s\\n\\n' ".to_vec();
    script.extend_from_slice(&posix_quote(OsStr::new(command.introduction())));
    script.push(b'\n');
    if let Some(state_dir) = &context.managed_state_dir {
        script.extend_from_slice(b"export PRNSD_STATE_DIR=");
        script.extend_from_slice(&posix_quote(state_dir.as_os_str()));
        script.push(b'\n');
    }
    script.extend_from_slice(b"cd ");
    script.extend_from_slice(&posix_quote(context.config_dir.as_os_str()));
    script.extend_from_slice(b" || exit 1\n");
    script.extend_from_slice(&posix_quote(context.binary.as_os_str()));
    for argument in context.command_arguments(command) {
        script.push(b' ');
        script.extend_from_slice(&posix_quote(&argument));
    }
    script.extend_from_slice(
        b"\ncommand_status=$?\n\
          rm -f -- \"$0\"\n\
          printf '\\nprnsd command exited with status %s.\\n' \"$command_status\"\n\
          exec \"${SHELL:-/bin/sh}\" -l\n",
    );
    script
}

#[cfg(unix)]
fn posix_quote(value: &OsStr) -> Vec<u8> {
    let mut quoted = Vec::with_capacity(value.as_bytes().len().saturating_add(2));
    quoted.push(b'\'');
    for byte in value.as_bytes() {
        if *byte == b'\'' {
            quoted.extend_from_slice(b"'\"'\"'");
        } else {
            quoted.push(*byte);
        }
    }
    quoted.push(b'\'');
    quoted
}

#[cfg(windows)]
fn write_terminal_script(
    context: &TrayActionContext,
    command: TerminalCommand,
) -> Result<PathBuf, TrayActionError> {
    let path = unique_script_path("cmd", command);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(TrayActionError::TemporaryScript)?;
    let script = windows_terminal_script(context, command);
    file.write_all(script.as_bytes())
        .map_err(TrayActionError::ScriptWrite)?;
    file.flush().map_err(TrayActionError::ScriptWrite)?;
    Ok(path)
}

#[cfg(windows)]
fn windows_terminal_script(context: &TrayActionContext, command: TerminalCommand) -> String {
    let mut script = String::from("@echo off\r\n");
    script.push_str("title Prns\r\n");
    script.push_str("echo ");
    script.push_str(command.introduction());
    script.push_str("\r\necho.\r\n");
    if let Some(state_dir) = &context.managed_state_dir {
        script.push_str("set \"PRNSD_STATE_DIR=");
        script.push_str(&windows_batch_value(state_dir.as_os_str()));
        script.push_str("\"\r\n");
    }
    script.push_str("cd /d \"");
    script.push_str(&windows_batch_value(context.config_dir.as_os_str()));
    script.push_str("\"\r\n\"");
    script.push_str(&windows_batch_value(context.binary.as_os_str()));
    script.push('"');
    for argument in context.command_arguments(command) {
        script.push_str(" \"");
        script.push_str(&windows_batch_value(&argument));
        script.push('"');
    }
    script.push_str("\r\nset \"PRNSD_TRAY_EXIT=%ERRORLEVEL%\"\r\n");
    script.push_str("echo.\r\necho prnsd command exited with status %PRNSD_TRAY_EXIT%.\r\n");
    script.push_str("(goto) 2>nul & del /q \"%~f0\"\r\n");
    script
}

#[cfg(windows)]
fn windows_batch_value(value: &OsStr) -> String {
    value.to_string_lossy().replace('%', "%%")
}

#[cfg(target_os = "macos")]
fn launch_terminal(script: &Path) -> Result<(), TrayActionError> {
    let status = Command::new("/usr/bin/open")
        .args(["-a", "Terminal"])
        .arg(script)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|source| TrayActionError::Launch {
            program: "Terminal",
            source,
        })?;
    if status.success() {
        Ok(())
    } else {
        Err(TrayActionError::LaunchExit {
            program: "Terminal",
            status,
        })
    }
}

#[cfg(windows)]
fn launch_terminal(script: &Path) -> Result<(), TrayActionError> {
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    Command::new("cmd.exe")
        .args(["/D", "/C", "start"])
        .raw_arg("\"Prns\"")
        .args(["cmd.exe", "/D", "/V:OFF", "/K"])
        .arg(script)
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|source| TrayActionError::Launch {
            program: "Command Prompt",
            source,
        })
}

#[cfg(all(unix, not(target_os = "macos")))]
fn launch_terminal(script: &Path) -> Result<(), TrayActionError> {
    if let Some(terminal) = std::env::var_os("TERMINAL").filter(|value| !value.is_empty()) {
        match spawn_terminal(Command::new(terminal), &["-e"], script) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(TrayActionError::Launch {
                    program: "$TERMINAL",
                    source,
                });
            }
        }
    }

    for (program, arguments) in [
        ("xdg-terminal-exec", &[][..]),
        ("x-terminal-emulator", &["-e"][..]),
        ("gnome-terminal", &["--"][..]),
        ("konsole", &["-e"][..]),
        ("kitty", &[][..]),
        ("alacritty", &["-e"][..]),
    ] {
        match spawn_terminal(Command::new(program), arguments, script) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(TrayActionError::Launch { program, source });
            }
        }
    }
    Err(TrayActionError::NoTerminal)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn spawn_terminal(
    mut command: Command,
    arguments: &[&str],
    script: &Path,
) -> Result<(), io::Error> {
    command
        .args(arguments)
        .arg(script)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
}

#[cfg(target_os = "macos")]
fn open_directory(path: &Path) -> Result<(), TrayActionError> {
    spawn_directory_opener("/usr/bin/open", Command::new("/usr/bin/open"), path)
}

#[cfg(windows)]
fn open_directory(path: &Path) -> Result<(), TrayActionError> {
    spawn_directory_opener("File Explorer", Command::new("explorer.exe"), path)
}

#[cfg(all(unix, not(target_os = "macos")))]
fn open_directory(path: &Path) -> Result<(), TrayActionError> {
    spawn_directory_opener("xdg-open", Command::new("xdg-open"), path)
}

fn spawn_directory_opener(
    program: &'static str,
    mut command: Command,
    path: &Path,
) -> Result<(), TrayActionError> {
    command
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|source| TrayActionError::Launch { program, source })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn terminal_script_targets_the_exact_binary_session_and_config() {
        let context = TrayActionContext::new_for_test(
            "/Applications/Prns' Tools/prnsd",
            "/Users/operator/Radio Config",
            Some(PathBuf::from(
                "/Users/operator/Library/Application Support/prnsd",
            )),
        );
        let script = posix_terminal_script(&context, TerminalCommand::Interfaces);
        let script = String::from_utf8_lossy(&script);

        assert!(script.contains(
            "export PRNSD_STATE_DIR='/Users/operator/Library/Application Support/prnsd'"
        ));
        assert!(script.contains("cd '/Users/operator/Radio Config' || exit 1"));
        assert!(script.contains(
            "'/Applications/Prns'\"'\"' Tools/prnsd' 'interfaces' '--config' '/Users/operator/Radio Config'"
        ));
        assert!(!script.contains("Press Ctrl-C"));
        assert!(script.contains("rm -f -- \"$0\""));
    }

    #[cfg(unix)]
    #[test]
    fn terminal_script_quoting_preserves_non_utf8_path_bytes() {
        use std::os::unix::ffi::OsStringExt;

        let path = OsString::from_vec(vec![b'/', b't', b'm', b'p', b'/', 0xff, b'\'', b'x']);

        assert_eq!(
            posix_quote(&path),
            vec![
                b'\'', b'/', b't', b'm', b'p', b'/', 0xff, b'\'', b'"', b'\'', b'"', b'\'', b'x',
                b'\'',
            ]
        );
    }

    #[test]
    fn only_managed_sessions_offer_log_attachment() {
        let managed =
            TrayActionContext::new_for_test("/opt/prnsd", "/config", Some(PathBuf::from("/state")));
        let foreground = TrayActionContext::new_for_test("/opt/prnsd", "/config", None);

        assert!(managed.can_attach_terminal());
        assert!(!foreground.can_attach_terminal());
    }
}
