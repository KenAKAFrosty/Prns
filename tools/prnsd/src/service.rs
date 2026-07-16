use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::Duration;

#[cfg(unix)]
use std::os::unix::process::CommandExt;

const ATTACH_BACKLOG_BYTES: u64 = 64 * 1024;
const POLL_INTERVAL: Duration = Duration::from_millis(100);
const FOLLOW_LIVENESS_POLLS: usize = 10;
const START_OBSERVATION_POLLS: usize = 10;
const STOP_POLLS: usize = 100;
const LOCK_POLLS: usize = 300;

#[derive(Debug)]
pub enum ServiceError {
    Io {
        operation: &'static str,
        source: io::Error,
    },
    #[cfg(not(unix))]
    UnsupportedPlatform,
    StartInProgress,
    ProcessExited {
        log: PathBuf,
    },
    SignalFailed {
        pid: u32,
    },
    StopTimedOut {
        pid: u32,
    },
}

impl fmt::Display for ServiceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { operation, source } => write!(formatter, "{operation}: {source}"),
            #[cfg(not(unix))]
            Self::UnsupportedPlatform => formatter.write_str(
                "managed cargo prnsd services are currently supported on macOS and Linux",
            ),
            Self::StartInProgress => {
                formatter.write_str("another cargo prnsd invocation is still starting the service")
            }
            Self::ProcessExited { log } => write!(
                formatter,
                "prnsd exited during startup; inspect {}",
                log.display()
            ),
            Self::SignalFailed { pid } => {
                write!(formatter, "could not send SIGTERM to prnsd process {pid}")
            }
            Self::StopTimedOut { pid } => {
                write!(
                    formatter,
                    "prnsd process {pid} did not stop within 10 seconds"
                )
            }
        }
    }
}

impl std::error::Error for ServiceError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServiceRecord {
    pub pid: u32,
    pub binary: PathBuf,
    pub log: PathBuf,
    pub signature: u64,
}

#[derive(Debug)]
pub struct ServicePaths {
    pub state_dir: PathBuf,
    pub record: PathBuf,
    pub human_log: PathBuf,
    pub json_log: PathBuf,
    lock: PathBuf,
}

impl ServicePaths {
    pub fn new(repo_root: &Path) -> Self {
        let state_dir = repo_root.join("prnsd/.run");
        Self {
            record: state_dir.join("service"),
            lock: state_dir.join("start.lock"),
            human_log: state_dir.join("prnsd.log"),
            json_log: repo_root.join("prnsd/observability/data/prnsd.jsonl"),
            state_dir,
        }
    }
}

pub enum StartOutcome {
    Started(ServiceRecord),
    AlreadyRunning(ServiceRecord),
}

struct StartLock {
    path: PathBuf,
}

impl StartLock {
    fn acquire(paths: &ServicePaths) -> Result<Self, ServiceError> {
        create_dir_all(&paths.state_dir, "could not create prnsd runtime directory")?;
        for _ in 0..LOCK_POLLS {
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&paths.lock)
            {
                Ok(mut file) => {
                    writeln!(file, "{}", std::process::id()).map_err(|source| {
                        ServiceError::Io {
                            operation: "could not write prnsd start lock",
                            source,
                        }
                    })?;
                    return Ok(Self {
                        path: paths.lock.clone(),
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    if !lock_owner_is_running(&paths.lock)? {
                        remove_if_present(&paths.lock, "could not remove stale prnsd start lock")?;
                        continue;
                    }
                    thread::sleep(POLL_INTERVAL);
                }
                Err(source) => {
                    return Err(ServiceError::Io {
                        operation: "could not create prnsd start lock",
                        source,
                    });
                }
            }
        }
        Err(ServiceError::StartInProgress)
    }
}

impl Drop for StartLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

pub fn running(paths: &ServicePaths) -> Result<Option<ServiceRecord>, ServiceError> {
    let Some(record) = read_record(paths)? else {
        return Ok(None);
    };
    if record_is_running(&record)? {
        return Ok(Some(record));
    }
    remove_if_present(&paths.record, "could not remove stale prnsd service record")?;
    Ok(None)
}

#[cfg(unix)]
pub fn start(
    paths: &ServicePaths,
    binary: &Path,
    daemon_args: &[std::ffi::OsString],
    working_dir: &Path,
    log: &Path,
    signature: u64,
) -> Result<StartOutcome, ServiceError> {
    let _lock = StartLock::acquire(paths)?;
    if let Some(record) = running(paths)? {
        return Ok(StartOutcome::AlreadyRunning(record));
    }

    let parent = log.parent().ok_or_else(|| ServiceError::Io {
        operation: "prnsd log path has no parent directory",
        source: io::Error::new(io::ErrorKind::InvalidInput, log.display().to_string()),
    })?;
    create_dir_all(parent, "could not create prnsd log directory")?;
    rotate_log(log)?;
    let stdout = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(log)
        .map_err(|source| ServiceError::Io {
            operation: "could not open prnsd log",
            source,
        })?;
    let stderr = stdout.try_clone().map_err(|source| ServiceError::Io {
        operation: "could not duplicate prnsd log handle",
        source,
    })?;

    let mut command = Command::new("nohup");
    command
        .arg(binary)
        .arg("--managed")
        .args(daemon_args)
        .current_dir(working_dir)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    command.process_group(0);
    let child = command.spawn().map_err(|source| ServiceError::Io {
        operation: "could not launch prnsd with nohup",
        source,
    })?;
    let record = ServiceRecord {
        pid: child.id(),
        binary: binary.to_path_buf(),
        log: log.to_path_buf(),
        signature,
    };
    if let Err(error) = write_record(paths, &record) {
        let _ = send_terminate(record.pid);
        return Err(error);
    }

    for _ in 0..START_OBSERVATION_POLLS {
        thread::sleep(POLL_INTERVAL);
        if !record_is_running(&record)? {
            remove_if_present(
                &paths.record,
                "could not remove failed prnsd service record",
            )?;
            return Err(ServiceError::ProcessExited {
                log: log.to_path_buf(),
            });
        }
    }
    Ok(StartOutcome::Started(record))
}

#[cfg(not(unix))]
pub fn start(
    _paths: &ServicePaths,
    _binary: &Path,
    _daemon_args: &[std::ffi::OsString],
    _working_dir: &Path,
    _log: &Path,
    _signature: u64,
) -> Result<StartOutcome, ServiceError> {
    Err(ServiceError::UnsupportedPlatform)
}

#[cfg(unix)]
pub fn stop(paths: &ServicePaths) -> Result<bool, ServiceError> {
    let Some(record) = running(paths)? else {
        return Ok(false);
    };
    send_terminate(record.pid)?;
    for _ in 0..STOP_POLLS {
        thread::sleep(POLL_INTERVAL);
        if !record_is_running(&record)? {
            remove_if_present(
                &paths.record,
                "could not remove stopped prnsd service record",
            )?;
            return Ok(true);
        }
    }
    Err(ServiceError::StopTimedOut { pid: record.pid })
}

#[cfg(unix)]
fn send_terminate(pid: u32) -> Result<(), ServiceError> {
    let status = Command::new("kill")
        .arg("-TERM")
        .arg(pid.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map_err(|source| ServiceError::Io {
            operation: "could not invoke kill for prnsd",
            source,
        })?;
    if !status.success() {
        return Err(ServiceError::SignalFailed { pid });
    }
    Ok(())
}

#[cfg(not(unix))]
pub fn stop(_paths: &ServicePaths) -> Result<bool, ServiceError> {
    Err(ServiceError::UnsupportedPlatform)
}

#[cfg(unix)]
pub fn stop_and_follow(paths: &ServicePaths, record: &ServiceRecord) -> Result<(), ServiceError> {
    let mut file = File::open(&record.log).map_err(|source| ServiceError::Io {
        operation: "could not open prnsd log for shutdown attachment",
        source,
    })?;
    seek_to_backlog(&mut file)?;
    send_terminate(record.pid)?;

    let stdout = io::stdout();
    let mut stdout = stdout.lock();
    for _ in 0..STOP_POLLS {
        copy_available(&mut file, &mut stdout)?;
        if !record_is_running(record)? {
            copy_available(&mut file, &mut stdout)?;
            remove_if_present(
                &paths.record,
                "could not remove stopped prnsd service record",
            )?;
            return Ok(());
        }
        follow_truncation(&mut file)?;
        thread::sleep(POLL_INTERVAL);
    }
    Err(ServiceError::StopTimedOut { pid: record.pid })
}

#[cfg(not(unix))]
pub fn stop_and_follow(_paths: &ServicePaths, _record: &ServiceRecord) -> Result<(), ServiceError> {
    Err(ServiceError::UnsupportedPlatform)
}

pub fn follow(record: &ServiceRecord) -> Result<(), ServiceError> {
    let mut file = File::open(&record.log).map_err(|source| ServiceError::Io {
        operation: "could not open prnsd log for attachment",
        source,
    })?;
    seek_to_backlog(&mut file)?;
    let stdout = io::stdout();
    let mut stdout = stdout.lock();
    let mut liveness_polls = 0;
    loop {
        if copy_available(&mut file, &mut stdout)? {
            liveness_polls = 0;
            continue;
        }
        liveness_polls += 1;
        if liveness_polls >= FOLLOW_LIVENESS_POLLS {
            if !record_is_running(record)? {
                return Ok(());
            }
            liveness_polls = 0;
        }
        follow_truncation(&mut file)?;
        thread::sleep(POLL_INTERVAL);
    }
}

fn copy_available(file: &mut File, output: &mut impl Write) -> Result<bool, ServiceError> {
    let mut copied = false;
    let mut buffer = [0_u8; 8 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|source| ServiceError::Io {
            operation: "could not read prnsd log",
            source,
        })?;
        if read == 0 {
            break;
        }
        output
            .write_all(&buffer[..read])
            .map_err(|source| ServiceError::Io {
                operation: "could not write attached prnsd log",
                source,
            })?;
        copied = true;
    }
    if copied {
        output.flush().map_err(|source| ServiceError::Io {
            operation: "could not flush attached prnsd log",
            source,
        })?;
    }
    Ok(copied)
}

fn follow_truncation(file: &mut File) -> Result<(), ServiceError> {
    let position = file.stream_position().map_err(|source| ServiceError::Io {
        operation: "could not inspect prnsd log position",
        source,
    })?;
    let length = file
        .metadata()
        .map_err(|source| ServiceError::Io {
            operation: "could not inspect prnsd log",
            source,
        })?
        .len();
    if length < position {
        file.seek(SeekFrom::Start(0))
            .map_err(|source| ServiceError::Io {
                operation: "could not follow truncated prnsd log",
                source,
            })?;
    }
    Ok(())
}

pub fn print_recent_log(path: &Path) -> Result<(), ServiceError> {
    if !path.exists() {
        return Ok(());
    }
    let mut file = File::open(path).map_err(|source| ServiceError::Io {
        operation: "could not open prnsd log",
        source,
    })?;
    seek_to_backlog(&mut file)?;
    let mut output = io::stdout().lock();
    io::copy(&mut file, &mut output).map_err(|source| ServiceError::Io {
        operation: "could not print prnsd log",
        source,
    })?;
    Ok(())
}

fn seek_to_backlog(file: &mut File) -> Result<(), ServiceError> {
    let length = file
        .metadata()
        .map_err(|source| ServiceError::Io {
            operation: "could not inspect prnsd log",
            source,
        })?
        .len();
    let offset = length.saturating_sub(ATTACH_BACKLOG_BYTES);
    file.seek(SeekFrom::Start(offset))
        .map_err(|source| ServiceError::Io {
            operation: "could not seek in prnsd log",
            source,
        })?;
    if offset > 0 {
        let mut byte = [0_u8; 1];
        while file.read(&mut byte).map_err(|source| ServiceError::Io {
            operation: "could not align prnsd log output",
            source,
        })? == 1
            && byte[0] != b'\n'
        {}
    }
    Ok(())
}

fn rotate_log(path: &Path) -> Result<(), ServiceError> {
    let previous = path.with_extension("previous");
    remove_if_present(&previous, "could not remove previous prnsd log")?;
    match fs::rename(path, previous) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(ServiceError::Io {
            operation: "could not rotate prnsd log",
            source,
        }),
    }
}

fn read_record(paths: &ServicePaths) -> Result<Option<ServiceRecord>, ServiceError> {
    let text = match fs::read_to_string(&paths.record) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(source) => {
            return Err(ServiceError::Io {
                operation: "could not read prnsd service record",
                source,
            });
        }
    };
    let mut lines = text.lines();
    let record = lines
        .next()
        .and_then(|pid| pid.parse().ok())
        .zip(lines.next())
        .zip(lines.next())
        .map(|((pid, binary), log)| ServiceRecord {
            pid,
            binary: PathBuf::from(binary),
            log: PathBuf::from(log),
            signature: lines
                .next()
                .and_then(|value| value.parse().ok())
                .unwrap_or(0),
        });
    if record.is_none() {
        remove_if_present(
            &paths.record,
            "could not remove invalid prnsd service record",
        )?;
    }
    Ok(record)
}

fn write_record(paths: &ServicePaths, record: &ServiceRecord) -> Result<(), ServiceError> {
    create_dir_all(&paths.state_dir, "could not create prnsd runtime directory")?;
    fs::write(
        &paths.record,
        format!(
            "{}\n{}\n{}\n{}\n",
            record.pid,
            record.binary.display(),
            record.log.display(),
            record.signature,
        ),
    )
    .map_err(|source| ServiceError::Io {
        operation: "could not write prnsd service record",
        source,
    })
}

fn lock_owner_is_running(path: &Path) -> Result<bool, ServiceError> {
    let pid = match fs::read_to_string(path) {
        Ok(text) => text.trim().parse().ok(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(source) => {
            return Err(ServiceError::Io {
                operation: "could not read prnsd start lock",
                source,
            });
        }
    };
    match pid {
        Some(pid) => pid_is_running(pid),
        None => Ok(false),
    }
}

#[cfg(unix)]
fn record_is_running(record: &ServiceRecord) -> Result<bool, ServiceError> {
    let output = Command::new("ps")
        .arg("-p")
        .arg(record.pid.to_string())
        .arg("-o")
        .arg("command=")
        .output()
        .map_err(|source| ServiceError::Io {
            operation: "could not inspect prnsd process",
            source,
        })?;
    if !output.status.success() {
        return Ok(false);
    }
    Ok(String::from_utf8_lossy(&output.stdout).contains(&*record.binary.to_string_lossy()))
}

#[cfg(not(unix))]
fn record_is_running(_record: &ServiceRecord) -> Result<bool, ServiceError> {
    Err(ServiceError::UnsupportedPlatform)
}

#[cfg(unix)]
fn pid_is_running(pid: u32) -> Result<bool, ServiceError> {
    Command::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .map_err(|source| ServiceError::Io {
            operation: "could not inspect process state with kill",
            source,
        })
}

#[cfg(not(unix))]
fn pid_is_running(_pid: u32) -> Result<bool, ServiceError> {
    Err(ServiceError::UnsupportedPlatform)
}

fn create_dir_all(path: &Path, operation: &'static str) -> Result<(), ServiceError> {
    fs::create_dir_all(path).map_err(|source| ServiceError::Io { operation, source })
}

fn remove_if_present(path: &Path, operation: &'static str) -> Result<(), ServiceError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(ServiceError::Io { operation, source }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_attachment_reads_new_bytes_without_replaying_old_bytes() {
        let path =
            std::env::temp_dir().join(format!("prnsd-command-follow-{}", std::process::id()));
        fs::write(&path, b"old\n").unwrap();
        let mut file = File::open(&path).unwrap();
        file.seek(SeekFrom::End(0)).unwrap();
        OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"new\n")
            .unwrap();

        let mut output = Vec::new();
        assert!(copy_available(&mut file, &mut output).unwrap());
        assert_eq!(output, b"new\n");
        assert!(!copy_available(&mut file, &mut output).unwrap());

        fs::remove_file(path).unwrap();
    }

    #[test]
    fn service_paths_are_repo_local_except_for_the_grafana_log() {
        let paths = ServicePaths::new(Path::new("/repo"));
        assert_eq!(paths.state_dir, Path::new("/repo/prnsd/.run"));
        assert_eq!(paths.human_log, Path::new("/repo/prnsd/.run/prnsd.log"));
        assert_eq!(
            paths.json_log,
            Path::new("/repo/prnsd/observability/data/prnsd.jsonl")
        );
    }
}
