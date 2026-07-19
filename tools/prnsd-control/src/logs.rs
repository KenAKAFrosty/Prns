#[cfg(test)]
use std::fs::OpenOptions;
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use crate::process::{request_stop, running, ControlLock, POLL_INTERVAL, STOP_TIMEOUT};
#[cfg(test)]
use crate::state::prepare_state_dir;
use crate::state::{cleanup_stale, open_secure, remove_if_present, runtime_is_locked};
use crate::{ServiceError, ServicePaths, ServiceRecord};

const ATTACH_BACKLOG_BYTES: u64 = 64 * 1024;
const LIVENESS_INTERVAL: Duration = Duration::from_secs(1);

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

pub(crate) fn rotate_log(
    path: &Path,
    previous: &Path,
    operation: &'static str,
) -> Result<(), ServiceError> {
    remove_if_present(previous, operation)?;
    match fs::rename(path, previous) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(source) => Err(ServiceError::Io { operation, source }),
    }
}

pub(crate) fn open_log(path: &Path) -> Result<File, ServiceError> {
    open_secure(path, true, true, "could not open the prnsd log")
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

    fn test_paths(name: &str) -> ServicePaths {
        ServicePaths::in_dir(
            std::env::temp_dir().join(format!("prnsd-control-{name}-{}", std::process::id())),
        )
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
}
