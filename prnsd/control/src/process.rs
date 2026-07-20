use std::ffi::{OsStr, OsString};
use std::fs::{self, File, TryLockError};
use std::io::{self};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[cfg(unix)]
use std::os::unix::process::CommandExt;
#[cfg(windows)]
use std::os::windows::process::CommandExt;

use crate::logs::{open_log, rotate_log};
use crate::record::{LogLane, ServiceRecord, ServiceState};
use crate::state::{
    cleanup_stale, open_lock, prepare_state_dir, read_generation, read_record, ready_generation,
    remove_generation_if_matching, runtime_is_locked, write_generation, write_record,
};
use crate::{ReloadRequest, ReloadResult, ServiceError, ServicePaths};

pub(crate) const POLL_INTERVAL: Duration = Duration::from_millis(100);
const RECORD_WAIT_TIMEOUT: Duration = Duration::from_secs(1);
const CONTROL_TIMEOUT: Duration = Duration::from_secs(30);
const START_TIMEOUT: Duration = Duration::from_secs(30);
pub(crate) const STOP_TIMEOUT: Duration = Duration::from_secs(30);
const MANAGED_STATE_DIR: &str = "PRNSD_INTERNAL_STATE_DIR";
const MANAGED_GENERATION: &str = "PRNSD_INTERNAL_GENERATION";
const MANAGED_SIGNATURE: &str = "PRNSD_INTERNAL_SIGNATURE";
const MANAGED_LOG_LANE: &str = "PRNSD_INTERNAL_LOG_LANE";
const MANAGED_VERSION: &str = "PRNSD_INTERNAL_VERSION";

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

pub(crate) struct ControlLock {
    file: File,
}

impl ControlLock {
    pub(crate) fn acquire(paths: &ServicePaths) -> Result<Self, ServiceError> {
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

    pub fn reload_request(&self) -> Result<Option<ReloadRequest>, ServiceError> {
        ReloadRequest::read(&self.paths, self.generation)
    }

    pub fn finish_reload(
        &self,
        request: &ReloadRequest,
        result: ReloadResult,
    ) -> Result<(), ServiceError> {
        result.write(&self.paths, request)
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

pub(crate) fn request_stop(
    paths: &ServicePaths,
    record: &ServiceRecord,
) -> Result<(), ServiceError> {
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
                        OsString::from("process::tests::managed_helper_process"),
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
