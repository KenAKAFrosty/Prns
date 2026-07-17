use std::ffi::{OsStr, OsString};
use std::fmt;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::os::unix::process::CommandExt;
#[cfg(windows)]
use std::os::windows::process::CommandExt;

use crate::record::{LogLane, ServiceRecord, ServiceState};
use crate::ServicePaths;

const ATTACH_BACKLOG_BYTES: u64 = 64 * 1024;
const POLL_INTERVAL: Duration = Duration::from_millis(100);
const LIVENESS_INTERVAL: Duration = Duration::from_secs(1);
const RECORD_WAIT_TIMEOUT: Duration = Duration::from_secs(1);
const CONTROL_TIMEOUT: Duration = Duration::from_secs(30);
const START_TIMEOUT: Duration = Duration::from_secs(30);
const STOP_TIMEOUT: Duration = Duration::from_secs(30);
const MANAGED_STATE_DIR: &str = "PRNSD_INTERNAL_STATE_DIR";
const MANAGED_GENERATION: &str = "PRNSD_INTERNAL_GENERATION";
const MANAGED_SIGNATURE: &str = "PRNSD_INTERNAL_SIGNATURE";
const MANAGED_LOG_LANE: &str = "PRNSD_INTERNAL_LOG_LANE";
const MANAGED_VERSION: &str = "PRNSD_INTERNAL_VERSION";

#[derive(Debug)]
pub enum ServiceError {
    Io {
        operation: &'static str,
        source: io::Error,
    },
    ControlBusy,
    InvalidRecord,
    IncompleteRecord,
    InvalidManagedEnvironment,
    ManagedInstanceAlreadyRunning,
    ProcessExited {
        log: PathBuf,
    },
    StartupTimedOut {
        pid: u32,
        log: PathBuf,
    },
    StopTimedOut {
        pid: u32,
    },
}

impl fmt::Display for ServiceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { operation, source } => write!(formatter, "{operation}: {source}"),
            Self::ControlBusy => {
                formatter.write_str("another prnsd lifecycle command is still in progress")
            }
            Self::InvalidRecord => formatter.write_str("the prnsd session record is invalid"),
            Self::IncompleteRecord => {
                formatter.write_str("prnsd is running without a complete session record")
            }
            Self::InvalidManagedEnvironment => {
                formatter.write_str("the internal prnsd managed environment is invalid")
            }
            Self::ManagedInstanceAlreadyRunning => {
                formatter.write_str("another managed prnsd instance already owns the session")
            }
            Self::ProcessExited { log } => write!(
                formatter,
                "prnsd exited during startup; inspect {}",
                log.display()
            ),
            Self::StartupTimedOut { pid, log } => write!(
                formatter,
                "prnsd process {pid} is still starting after 30 seconds; inspect {}",
                log.display()
            ),
            Self::StopTimedOut { pid } => write!(
                formatter,
                "prnsd process {pid} did not stop within 30 seconds"
            ),
        }
    }
}

impl std::error::Error for ServiceError {}

pub struct LaunchSpec<'a> {
    pub binary: &'a Path,
    pub managed_binary: Option<&'a Path>,
    pub args: &'a [OsString],
    pub working_dir: &'a Path,
    pub log_lane: LogLane,
    pub signature: u64,
    pub version: &'a str,
}

#[derive(Debug)]
pub enum StartOutcome {
    Started(ServiceRecord),
    AlreadyRunning(ServiceRecord),
}

struct ControlLock {
    file: File,
}

impl ControlLock {
    fn acquire(paths: &ServicePaths) -> Result<Self, ServiceError> {
        prepare_state_dir(paths)?;
        let file = open_lock(&paths.control_lock, "could not open prnsd control lock")?;
        let started = Instant::now();
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(Self { file }),
                Err(TryLockError::WouldBlock) if started.elapsed() < CONTROL_TIMEOUT => {
                    thread::sleep(POLL_INTERVAL);
                }
                Err(TryLockError::WouldBlock) => return Err(ServiceError::ControlBusy),
                Err(TryLockError::Error(source)) => {
                    return Err(ServiceError::Io {
                        operation: "could not lock prnsd lifecycle control",
                        source,
                    });
                }
            }
        }
    }
}

impl Drop for ControlLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

pub struct ManagedProcess {
    paths: ServicePaths,
    generation: u128,
    runtime_lock: File,
}

impl ManagedProcess {
    pub fn from_environment() -> Result<Option<Self>, ServiceError> {
        let Some(state_dir) = std::env::var_os(MANAGED_STATE_DIR) else {
            return Ok(None);
        };
        let generation = managed_value(MANAGED_GENERATION)?
            .parse()
            .map_err(|_| ServiceError::InvalidManagedEnvironment)?;
        let signature = managed_value(MANAGED_SIGNATURE)?
            .parse()
            .map_err(|_| ServiceError::InvalidManagedEnvironment)?;
        let log_lane = LogLane::parse(&managed_value(MANAGED_LOG_LANE)?)
            .ok_or(ServiceError::InvalidManagedEnvironment)?;
        let version = managed_value(MANAGED_VERSION)?;
        let paths = ServicePaths::in_dir(state_dir);
        prepare_state_dir(&paths)?;
        let runtime_lock = open_lock(&paths.runtime_lock, "could not open prnsd runtime lock")?;
        match runtime_lock.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => {
                return Err(ServiceError::ManagedInstanceAlreadyRunning);
            }
            Err(TryLockError::Error(source)) => {
                return Err(ServiceError::Io {
                    operation: "could not lock the prnsd managed session",
                    source,
                });
            }
        }
        let record = ServiceRecord {
            generation,
            pid: std::process::id(),
            signature,
            log_lane,
            binary: std::env::current_exe().map_err(|source| ServiceError::Io {
                operation: "could not locate the running prnsd executable",
                source,
            })?,
            version,
            state: ServiceState::Starting,
        };
        write_record(&paths, &record)?;
        Ok(Some(Self {
            paths,
            generation,
            runtime_lock,
        }))
    }

    pub fn mark_ready(&self) -> Result<(), ServiceError> {
        write_generation(
            &self.paths.ready,
            self.generation,
            "could not mark prnsd ready",
        )
    }

    pub fn stop_requested(&self) -> Result<bool, ServiceError> {
        read_generation(&self.paths.stop, "could not read prnsd stop request")
            .map(|generation| generation == Some(self.generation))
    }

    pub fn hold_runtime_lock_until_process_exit(self) {
        std::mem::forget(self);
    }
}

impl Drop for ManagedProcess {
    fn drop(&mut self) {
        let _ = self.runtime_lock.unlock();
        remove_generation_if_matching(&self.paths.ready, self.generation);
        remove_generation_if_matching(&self.paths.stop, self.generation);
        if read_record(&self.paths)
            .ok()
            .flatten()
            .is_some_and(|record| record.generation == self.generation)
        {
            let _ = fs::remove_file(&self.paths.record);
        }
    }
}

pub fn start(paths: &ServicePaths, launch: LaunchSpec<'_>) -> Result<StartOutcome, ServiceError> {
    let _control = ControlLock::acquire(paths)?;
    if let Some(record) = running(paths)? {
        return Ok(StartOutcome::AlreadyRunning(record));
    }
    cleanup_stale(paths)?;
    let binary = match launch.managed_binary {
        Some(path) => stage_binary(launch.binary, path)?,
        None => launch.binary.to_path_buf(),
    };
    let generation = generation();
    let log = launch.log_lane.path(paths);
    rotate_log(
        log,
        launch.log_lane.previous_path(paths),
        "could not rotate the prnsd log",
    )?;
    let stdout = open_log(log)?;
    let stderr = stdout.try_clone().map_err(|source| ServiceError::Io {
        operation: "could not duplicate the prnsd log handle",
        source,
    })?;
    let mut command = Command::new(binary);
    command
        .args(launch.args)
        .current_dir(launch.working_dir)
        .env_remove(MANAGED_STATE_DIR)
        .env_remove(MANAGED_GENERATION)
        .env_remove(MANAGED_SIGNATURE)
        .env_remove(MANAGED_LOG_LANE)
        .env_remove(MANAGED_VERSION)
        .env(MANAGED_STATE_DIR, &paths.state_dir)
        .env(MANAGED_GENERATION, generation.to_string())
        .env(MANAGED_SIGNATURE, launch.signature.to_string())
        .env(MANAGED_LOG_LANE, launch.log_lane.as_str())
        .env(MANAGED_VERSION, launch.version)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    #[cfg(unix)]
    command.process_group(0);
    #[cfg(windows)]
    command.creation_flags(windows_sys::Win32::System::Threading::DETACHED_PROCESS);
    let mut child = command.spawn().map_err(|source| ServiceError::Io {
        operation: "could not launch the managed prnsd process",
        source,
    })?;
    let started = Instant::now();
    loop {
        if let Some(mut record) = read_record(paths)? {
            if record.generation == generation && ready_generation(paths)? == Some(generation) {
                record.state = ServiceState::Running;
                return Ok(StartOutcome::Started(record));
            }
        }
        if child
            .try_wait()
            .map_err(|source| ServiceError::Io {
                operation: "could not inspect the starting prnsd process",
                source,
            })?
            .is_some()
        {
            cleanup_stale(paths)?;
            return Err(ServiceError::ProcessExited {
                log: log.to_path_buf(),
            });
        }
        if started.elapsed() >= START_TIMEOUT {
            return Err(ServiceError::StartupTimedOut {
                pid: child.id(),
                log: log.to_path_buf(),
            });
        }
        thread::sleep(POLL_INTERVAL);
    }
}

pub fn running(paths: &ServicePaths) -> Result<Option<ServiceRecord>, ServiceError> {
    prepare_state_dir(paths)?;
    let runtime_lock = open_lock(&paths.runtime_lock, "could not open prnsd runtime lock")?;
    match runtime_lock.try_lock() {
        Ok(()) => {
            cleanup_stale(paths)?;
            runtime_lock.unlock().map_err(|source| ServiceError::Io {
                operation: "could not unlock the prnsd runtime probe",
                source,
            })?;
            return Ok(None);
        }
        Err(TryLockError::WouldBlock) => {}
        Err(TryLockError::Error(source)) => {
            return Err(ServiceError::Io {
                operation: "could not inspect the prnsd runtime lock",
                source,
            });
        }
    }
    let started = Instant::now();
    let mut record = loop {
        if let Some(record) = read_record(paths)? {
            break record;
        }
        if started.elapsed() >= RECORD_WAIT_TIMEOUT {
            return Err(ServiceError::IncompleteRecord);
        }
        thread::sleep(POLL_INTERVAL);
    };
    record.state = if ready_generation(paths)? == Some(record.generation) {
        ServiceState::Running
    } else {
        ServiceState::Starting
    };
    Ok(Some(record))
}

pub fn stop(paths: &ServicePaths) -> Result<bool, ServiceError> {
    let _control = ControlLock::acquire(paths)?;
    let Some(record) = running(paths)? else {
        return Ok(false);
    };
    request_stop(paths, &record)?;
    wait_for_stop(paths, &record)?;
    Ok(true)
}

pub fn wait_until_ready(
    paths: &ServicePaths,
    mut record: ServiceRecord,
) -> Result<ServiceRecord, ServiceError> {
    let started = Instant::now();
    loop {
        match running(paths)? {
            Some(current) => record = current,
            None => {
                return Err(ServiceError::ProcessExited {
                    log: record.log(paths).to_path_buf(),
                });
            }
        }
        if record.state == ServiceState::Running {
            return Ok(record);
        }
        if started.elapsed() >= START_TIMEOUT {
            return Err(ServiceError::StartupTimedOut {
                pid: record.pid,
                log: record.log(paths).to_path_buf(),
            });
        }
        thread::sleep(POLL_INTERVAL);
    }
}

pub fn stop_and_follow(paths: &ServicePaths, record: &ServiceRecord) -> Result<(), ServiceError> {
    let _control = ControlLock::acquire(paths)?;
    let Some(current) = running(paths)? else {
        return Ok(());
    };
    if current.generation != record.generation {
        return Err(ServiceError::InvalidRecord);
    }
    let mut file = File::open(record.log(paths)).map_err(|source| ServiceError::Io {
        operation: "could not open the prnsd log for shutdown attachment",
        source,
    })?;
    seek_to_backlog(&mut file)?;
    request_stop(paths, record)?;
    let started = Instant::now();
    let mut output = io::stdout().lock();
    loop {
        copy_available(&mut file, &mut output)?;
        if !runtime_is_locked(paths)? {
            copy_available(&mut file, &mut output)?;
            cleanup_stale(paths)?;
            return Ok(());
        }
        if started.elapsed() >= STOP_TIMEOUT {
            return Err(ServiceError::StopTimedOut { pid: record.pid });
        }
        follow_truncation(&mut file)?;
        thread::sleep(POLL_INTERVAL);
    }
}

pub fn follow(paths: &ServicePaths, record: &ServiceRecord) -> Result<(), ServiceError> {
    let mut file = File::open(record.log(paths)).map_err(|source| ServiceError::Io {
        operation: "could not open the prnsd log for attachment",
        source,
    })?;
    seek_to_backlog(&mut file)?;
    let mut output = io::stdout().lock();
    let mut last_liveness = Instant::now();
    loop {
        if copy_available(&mut file, &mut output)? {
            continue;
        }
        if last_liveness.elapsed() >= LIVENESS_INTERVAL {
            if !runtime_is_locked(paths)? {
                copy_available(&mut file, &mut output)?;
                return Ok(());
            }
            last_liveness = Instant::now();
        }
        follow_truncation(&mut file)?;
        thread::sleep(POLL_INTERVAL);
    }
}

pub fn print_recent_log(path: &Path) -> Result<(), ServiceError> {
    if !path.exists() {
        return Ok(());
    }
    let mut file = File::open(path).map_err(|source| ServiceError::Io {
        operation: "could not open the prnsd log",
        source,
    })?;
    seek_to_backlog(&mut file)?;
    io::copy(&mut file, &mut io::stdout().lock()).map_err(|source| ServiceError::Io {
        operation: "could not print the prnsd log",
        source,
    })?;
    Ok(())
}

pub fn launch_signature(
    values: impl IntoIterator<Item = OsString>,
    environment: impl IntoIterator<Item = (OsString, OsString)>,
) -> u64 {
    let mut hash = 0xcbf29ce484222325_u64;
    for value in values {
        hash_value(&mut hash, &value);
    }
    hash ^= u64::MAX;
    hash = hash.wrapping_mul(0x100000001b3);
    let mut environment: Vec<_> = environment
        .into_iter()
        .filter(|(name, _)| {
            name == "RUST_LOG" || name.to_str().is_some_and(|name| name.starts_with("OTEL_"))
        })
        .collect();
    environment.sort_by(|left, right| left.0.cmp(&right.0));
    for (name, value) in environment {
        hash_value(&mut hash, &name);
        hash_value(&mut hash, &value);
    }
    hash
}

fn hash_value(hash: &mut u64, value: &OsStr) {
    let value = value.to_string_lossy();
    for byte in (value.len() as u64)
        .to_le_bytes()
        .into_iter()
        .chain(value.bytes())
    {
        *hash ^= u64::from(byte);
        *hash = hash.wrapping_mul(0x100000001b3);
    }
}

fn managed_value(name: &str) -> Result<String, ServiceError> {
    std::env::var(name).map_err(|_| ServiceError::InvalidManagedEnvironment)
}

fn generation() -> u128 {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    elapsed ^ (u128::from(std::process::id()) << 64)
}

fn request_stop(paths: &ServicePaths, record: &ServiceRecord) -> Result<(), ServiceError> {
    write_generation(
        &paths.stop,
        record.generation,
        "could not request prnsd shutdown",
    )
}

fn wait_for_stop(paths: &ServicePaths, record: &ServiceRecord) -> Result<(), ServiceError> {
    let started = Instant::now();
    loop {
        if !runtime_is_locked(paths)? {
            cleanup_stale(paths)?;
            return Ok(());
        }
        if started.elapsed() >= STOP_TIMEOUT {
            return Err(ServiceError::StopTimedOut { pid: record.pid });
        }
        thread::sleep(POLL_INTERVAL);
    }
}

fn runtime_is_locked(paths: &ServicePaths) -> Result<bool, ServiceError> {
    let file = open_lock(&paths.runtime_lock, "could not open prnsd runtime lock")?;
    match file.try_lock() {
        Ok(()) => {
            file.unlock().map_err(|source| ServiceError::Io {
                operation: "could not unlock the prnsd runtime probe",
                source,
            })?;
            Ok(false)
        }
        Err(TryLockError::WouldBlock) => Ok(true),
        Err(TryLockError::Error(source)) => Err(ServiceError::Io {
            operation: "could not inspect the prnsd runtime lock",
            source,
        }),
    }
}

fn ready_generation(paths: &ServicePaths) -> Result<Option<u128>, ServiceError> {
    read_generation(&paths.ready, "could not read prnsd readiness marker")
}

fn read_generation(path: &Path, operation: &'static str) -> Result<Option<u128>, ServiceError> {
    match fs::read_to_string(path) {
        Ok(text) => text
            .trim()
            .parse()
            .map(Some)
            .map_err(|_| ServiceError::InvalidRecord),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(ServiceError::Io { operation, source }),
    }
}

fn write_generation(
    path: &Path,
    generation: u128,
    operation: &'static str,
) -> Result<(), ServiceError> {
    atomic_write(path, format!("{generation}\n").as_bytes(), operation)
}

fn read_record(paths: &ServicePaths) -> Result<Option<ServiceRecord>, ServiceError> {
    match fs::read_to_string(&paths.record) {
        Ok(text) => ServiceRecord::decode(&text)
            .map(Some)
            .map_err(|_| ServiceError::InvalidRecord),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(ServiceError::Io {
            operation: "could not read the prnsd session record",
            source,
        }),
    }
}

fn write_record(paths: &ServicePaths, record: &ServiceRecord) -> Result<(), ServiceError> {
    atomic_write(
        &paths.record,
        record.encode().as_bytes(),
        "could not write the prnsd session record",
    )
}

fn atomic_write(path: &Path, bytes: &[u8], operation: &'static str) -> Result<(), ServiceError> {
    let parent = path.parent().ok_or_else(|| ServiceError::Io {
        operation,
        source: io::Error::new(io::ErrorKind::InvalidInput, "state path has no parent"),
    })?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)
        .map_err(|source| ServiceError::Io { operation, source })?;
    temporary
        .write_all(bytes)
        .and_then(|()| temporary.as_file().sync_all())
        .map_err(|source| ServiceError::Io { operation, source })?;
    temporary
        .persist(path)
        .map(|_| ())
        .map_err(|error| ServiceError::Io {
            operation,
            source: error.error,
        })
}

fn cleanup_stale(paths: &ServicePaths) -> Result<(), ServiceError> {
    remove_if_present(&paths.record, "could not remove stale prnsd session record")?;
    remove_if_present(
        &paths.ready,
        "could not remove stale prnsd readiness marker",
    )?;
    remove_if_present(&paths.stop, "could not remove stale prnsd stop request")
}

fn remove_generation_if_matching(path: &Path, generation: u128) {
    if fs::read_to_string(path)
        .ok()
        .and_then(|text| text.trim().parse().ok())
        == Some(generation)
    {
        let _ = fs::remove_file(path);
    }
}

fn rotate_log(path: &Path, previous: &Path, operation: &'static str) -> Result<(), ServiceError> {
    remove_if_present(previous, operation)?;
    match fs::rename(path, previous) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(ServiceError::Io { operation, source }),
    }
}

fn open_log(path: &Path) -> Result<File, ServiceError> {
    open_secure(path, true, true, "could not open the prnsd log")
}

fn stage_binary(source: &Path, destination: &Path) -> Result<PathBuf, ServiceError> {
    let mut source = File::open(source).map_err(|source| ServiceError::Io {
        operation: "could not open the built prnsd executable",
        source,
    })?;
    let parent = destination.parent().ok_or_else(|| ServiceError::Io {
        operation: "managed prnsd executable path has no parent",
        source: io::Error::new(
            io::ErrorKind::InvalidInput,
            destination.display().to_string(),
        ),
    })?;
    let mut temporary =
        tempfile::NamedTempFile::new_in(parent).map_err(|source| ServiceError::Io {
            operation: "could not stage the managed prnsd executable",
            source,
        })?;
    io::copy(&mut source, &mut temporary)
        .and_then(|_| temporary.as_file().sync_all())
        .map_err(|source| ServiceError::Io {
            operation: "could not stage the managed prnsd executable",
            source,
        })?;
    temporary
        .persist(destination)
        .map_err(|error| ServiceError::Io {
            operation: "could not install the managed prnsd executable",
            source: error.error,
        })?;
    Ok(destination.to_path_buf())
}

fn open_lock(path: &Path, operation: &'static str) -> Result<File, ServiceError> {
    open_secure(path, false, true, operation)
}

fn open_secure(
    path: &Path,
    truncate: bool,
    create: bool,
    operation: &'static str,
) -> Result<File, ServiceError> {
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(true)
        .truncate(truncate)
        .create(create);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(path)
        .map_err(|source| ServiceError::Io { operation, source })
}

fn prepare_state_dir(paths: &ServicePaths) -> Result<(), ServiceError> {
    fs::create_dir_all(&paths.state_dir).map_err(|source| ServiceError::Io {
        operation: "could not create the prnsd state directory",
        source,
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&paths.state_dir, fs::Permissions::from_mode(0o700)).map_err(
            |source| ServiceError::Io {
                operation: "could not protect the prnsd state directory",
                source,
            },
        )?;
    }
    Ok(())
}

fn remove_if_present(path: &Path, operation: &'static str) -> Result<(), ServiceError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(ServiceError::Io { operation, source }),
    }
}

fn copy_available(file: &mut File, output: &mut impl Write) -> Result<bool, ServiceError> {
    let mut copied = false;
    let mut buffer = [0_u8; 8 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|source| ServiceError::Io {
            operation: "could not read the prnsd log",
            source,
        })?;
        if read == 0 {
            break;
        }
        output
            .write_all(&buffer[..read])
            .map_err(|source| ServiceError::Io {
                operation: "could not write attached prnsd output",
                source,
            })?;
        copied = true;
    }
    if copied {
        output.flush().map_err(|source| ServiceError::Io {
            operation: "could not flush attached prnsd output",
            source,
        })?;
    }
    Ok(copied)
}

fn follow_truncation(file: &mut File) -> Result<(), ServiceError> {
    let position = file.stream_position().map_err(|source| ServiceError::Io {
        operation: "could not inspect the prnsd log position",
        source,
    })?;
    let length = file
        .metadata()
        .map_err(|source| ServiceError::Io {
            operation: "could not inspect the prnsd log",
            source,
        })?
        .len();
    if length < position {
        file.seek(SeekFrom::Start(0))
            .map_err(|source| ServiceError::Io {
                operation: "could not follow the rotated prnsd log",
                source,
            })?;
    }
    Ok(())
}

fn seek_to_backlog(file: &mut File) -> Result<(), ServiceError> {
    let length = file
        .metadata()
        .map_err(|source| ServiceError::Io {
            operation: "could not inspect the prnsd log",
            source,
        })?
        .len();
    let offset = length.saturating_sub(ATTACH_BACKLOG_BYTES);
    file.seek(SeekFrom::Start(offset))
        .map_err(|source| ServiceError::Io {
            operation: "could not seek in the prnsd log",
            source,
        })?;
    if offset > 0 {
        let mut byte = [0_u8; 1];
        while file.read(&mut byte).map_err(|source| ServiceError::Io {
            operation: "could not align attached prnsd output",
            source,
        })? == 1
            && byte[0] != b'\n'
        {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};

    fn test_paths(name: &str) -> ServicePaths {
        ServicePaths::in_dir(
            std::env::temp_dir().join(format!("prnsd-control-{name}-{}", std::process::id())),
        )
    }

    #[test]
    fn runtime_lock_is_the_liveness_source() {
        let paths = test_paths("liveness");
        prepare_state_dir(&paths).unwrap();
        assert!(!runtime_is_locked(&paths).unwrap());
        let lock = open_lock(&paths.runtime_lock, "test lock").unwrap();
        lock.try_lock().unwrap();
        assert!(runtime_is_locked(&paths).unwrap());
        lock.unlock().unwrap();
        fs::remove_dir_all(paths.state_dir).unwrap();
    }

    #[test]
    fn readiness_marker_distinguishes_starting_from_running() {
        let paths = test_paths("readiness");
        prepare_state_dir(&paths).unwrap();
        let lock = open_lock(&paths.runtime_lock, "test lock").unwrap();
        lock.try_lock().unwrap();
        let record = ServiceRecord {
            generation: 41,
            pid: 17,
            signature: 9,
            log_lane: LogLane::Human,
            binary: PathBuf::from("/test/prnsd"),
            version: "test".to_string(),
            state: ServiceState::Starting,
        };
        write_record(&paths, &record).unwrap();
        assert_eq!(
            running(&paths).unwrap().unwrap().state,
            ServiceState::Starting
        );
        write_generation(&paths.ready, record.generation, "test ready").unwrap();
        assert_eq!(
            running(&paths).unwrap().unwrap().state,
            ServiceState::Running
        );
        lock.unlock().unwrap();
        assert!(running(&paths).unwrap().is_none());
        fs::remove_dir_all(paths.state_dir).unwrap();
    }

    #[test]
    fn stale_records_and_markers_are_cleaned_only_when_unlocked() {
        let paths = test_paths("stale");
        prepare_state_dir(&paths).unwrap();
        fs::write(&paths.record, "stale").unwrap();
        fs::write(&paths.ready, "1\n").unwrap();
        fs::write(&paths.stop, "1\n").unwrap();
        assert!(running(&paths).unwrap().is_none());
        assert!(!paths.record.exists());
        assert!(!paths.ready.exists());
        assert!(!paths.stop.exists());
        fs::remove_dir_all(paths.state_dir).unwrap();
    }

    #[test]
    fn stop_requests_are_generation_scoped() {
        let paths = test_paths("generation");
        prepare_state_dir(&paths).unwrap();
        write_generation(&paths.stop, 41, "test stop").unwrap();
        assert_eq!(read_generation(&paths.stop, "test stop").unwrap(), Some(41));
        remove_generation_if_matching(&paths.stop, 42);
        assert!(paths.stop.exists());
        remove_generation_if_matching(&paths.stop, 41);
        assert!(!paths.stop.exists());
        fs::remove_dir_all(paths.state_dir).unwrap();
    }

    #[test]
    fn attachment_reads_appended_bytes_once() {
        let paths = test_paths("follow");
        prepare_state_dir(&paths).unwrap();
        fs::write(&paths.human_log, b"old\n").unwrap();
        let mut file = File::open(&paths.human_log).unwrap();
        file.seek(SeekFrom::End(0)).unwrap();
        OpenOptions::new()
            .append(true)
            .open(&paths.human_log)
            .unwrap()
            .write_all(b"new\n")
            .unwrap();
        let mut output = Vec::new();
        assert!(copy_available(&mut file, &mut output).unwrap());
        assert_eq!(output, b"new\n");
        assert!(!copy_available(&mut file, &mut output).unwrap());
        fs::remove_dir_all(paths.state_dir).unwrap();
    }

    #[test]
    fn rotation_keeps_one_predecessor() {
        let paths = test_paths("rotation");
        prepare_state_dir(&paths).unwrap();
        fs::write(&paths.human_log, b"first\n").unwrap();
        rotate_log(&paths.human_log, &paths.human_previous_log, "test rotation").unwrap();
        assert_eq!(fs::read(&paths.human_previous_log).unwrap(), b"first\n");
        fs::write(&paths.human_log, b"second\n").unwrap();
        rotate_log(&paths.human_log, &paths.human_previous_log, "test rotation").unwrap();
        assert_eq!(fs::read(&paths.human_previous_log).unwrap(), b"second\n");
        fs::remove_dir_all(paths.state_dir).unwrap();
    }

    #[test]
    fn staged_binary_atomically_replaces_its_predecessor() {
        let paths = test_paths("staged-binary");
        prepare_state_dir(&paths).unwrap();
        let source = paths.state_dir.join("source");
        let destination = paths.state_dir.join("managed");
        fs::write(&source, b"new executable").unwrap();
        fs::write(&destination, b"old executable").unwrap();
        assert_eq!(stage_binary(&source, &destination).unwrap(), destination);
        assert_eq!(fs::read(&destination).unwrap(), b"new executable");
        fs::remove_dir_all(paths.state_dir).unwrap();
    }

    #[test]
    fn launch_signature_tracks_args_and_observability_environment() {
        let values = vec![OsString::from("run"), OsString::from("--config=/one")];
        let signature = launch_signature(
            values.clone(),
            vec![
                (OsString::from("RUST_LOG"), OsString::from("info")),
                (OsString::from("OTHER"), OsString::from("ignored")),
            ],
        );
        assert_eq!(
            signature,
            launch_signature(
                values.clone(),
                vec![
                    (OsString::from("OTHER"), OsString::from("different")),
                    (OsString::from("RUST_LOG"), OsString::from("info")),
                ]
            )
        );
        assert_ne!(
            signature,
            launch_signature(
                values,
                vec![(OsString::from("RUST_LOG"), OsString::from("debug"))]
            )
        );
    }

    #[test]
    fn managed_helper_process() {
        if std::env::var_os(MANAGED_STATE_DIR).is_none() {
            return;
        }
        let managed = ManagedProcess::from_environment().unwrap().unwrap();
        managed.mark_ready().unwrap();
        while !managed.stop_requested().unwrap() {
            thread::sleep(POLL_INTERVAL);
        }
        managed.hold_runtime_lock_until_process_exit();
    }

    #[test]
    fn concurrent_starts_share_one_ready_process_and_stop_cleanly() {
        let paths = test_paths("concurrent");
        let binary = std::env::current_exe().unwrap();
        let working_dir = std::env::current_dir().unwrap();
        let barrier = Arc::new(Barrier::new(2));
        let handles: Vec<_> = (0..2)
            .map(|_| {
                let paths = paths.clone();
                let binary = binary.clone();
                let working_dir = working_dir.clone();
                let barrier = Arc::clone(&barrier);
                thread::spawn(move || {
                    let args = [
                        OsString::from("--exact"),
                        OsString::from("service::tests::managed_helper_process"),
                        OsString::from("--nocapture"),
                    ];
                    barrier.wait();
                    start(
                        &paths,
                        LaunchSpec {
                            binary: &binary,
                            managed_binary: None,
                            args: &args,
                            working_dir: &working_dir,
                            log_lane: LogLane::Human,
                            signature: 7,
                            version: "test",
                        },
                    )
                    .unwrap()
                })
            })
            .collect();
        let outcomes: Vec<_> = handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect();
        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| matches!(outcome, StartOutcome::Started(_)))
                .count(),
            1
        );
        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| matches!(outcome, StartOutcome::AlreadyRunning(_)))
                .count(),
            1
        );
        assert_eq!(
            running(&paths).unwrap().unwrap().state,
            ServiceState::Running
        );
        assert!(stop(&paths).unwrap());
        assert!(running(&paths).unwrap().is_none());
        fs::remove_dir_all(paths.state_dir).unwrap();
    }
}
