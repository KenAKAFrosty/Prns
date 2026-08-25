use std::fs;
use std::io;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use crate::process::{running, ControlLock, POLL_INTERVAL};
use crate::state::{atomic_write, remove_if_present};
use crate::{ServiceError, ServicePaths};

const REQUEST_VERSION: &str = "prnsd-reload-request-v1";
const RESULT_VERSION: &str = "prnsd-reload-result-v1";
const RELOAD_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReloadRequest {
    generation: u128,
    request_id: u128,
    digest: [u8; 32],
}

impl ReloadRequest {
    pub const fn generation(&self) -> u128 {
        self.generation
    }

    pub const fn request_id(&self) -> u128 {
        self.request_id
    }

    pub const fn digest(&self) -> [u8; 32] {
        self.digest
    }

    pub(crate) fn read(
        paths: &ServicePaths,
        generation: u128,
    ) -> Result<Option<Self>, ServiceError> {
        let path = paths.reload_request(generation);
        match fs::read_to_string(path) {
            Ok(text) => Self::decode(&text)
                .filter(|request| request.generation == generation)
                .map(Some)
                .ok_or(ServiceError::InvalidRecord),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(source) => Err(ServiceError::Io {
                operation: "could not read the prnsd interface apply request",
                source,
            }),
        }
    }

    fn encode(&self) -> String {
        format!(
            "{REQUEST_VERSION}\n{:032x}\n{:032x}\n{}\n",
            self.generation,
            self.request_id,
            encode_digest(self.digest),
        )
    }

    fn decode(text: &str) -> Option<Self> {
        let mut lines = text.lines();
        if lines.next()? != REQUEST_VERSION {
            return None;
        }
        let generation = u128::from_str_radix(lines.next()?, 16).ok()?;
        let request_id = u128::from_str_radix(lines.next()?, 16).ok()?;
        let digest = decode_digest(lines.next()?)?;
        if lines.next().is_some() {
            return None;
        }
        Some(Self {
            generation,
            request_id,
            digest,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReloadResult {
    Applied,
    Unchanged,
    RestartRequired,
    NotInterfaceOwner,
    Rejected,
    RolledBack { rollback_failed: bool },
}

impl ReloadResult {
    pub(crate) fn write(
        self,
        paths: &ServicePaths,
        request: &ReloadRequest,
    ) -> Result<(), ServiceError> {
        let result_path = paths.reload_result(request.generation, request.request_id);
        atomic_write(
            &result_path,
            self.encode(request).as_bytes(),
            "could not write the prnsd interface apply result",
        )?;
        remove_if_present(
            &paths.reload_request(request.generation),
            "could not remove the completed prnsd interface apply request",
        )
    }

    fn encode(self, request: &ReloadRequest) -> String {
        let (name, rollback_failed) = match self {
            Self::Applied => ("applied", false),
            Self::Unchanged => ("unchanged", false),
            Self::RestartRequired => ("restart-required", false),
            Self::NotInterfaceOwner => ("not-interface-owner", false),
            Self::Rejected => ("rejected", false),
            Self::RolledBack { rollback_failed } => ("rolled-back", rollback_failed),
        };
        format!(
            "{RESULT_VERSION}\n{:032x}\n{:032x}\n{name}\n{}\n",
            request.generation,
            request.request_id,
            u8::from(rollback_failed),
        )
    }

    fn decode(text: &str, request: &ReloadRequest) -> Option<Self> {
        let mut lines = text.lines();
        if lines.next()? != RESULT_VERSION {
            return None;
        }
        if u128::from_str_radix(lines.next()?, 16).ok()? != request.generation {
            return None;
        }
        if u128::from_str_radix(lines.next()?, 16).ok()? != request.request_id {
            return None;
        }
        let name = lines.next()?;
        let rollback_failed = match lines.next()? {
            "0" => false,
            "1" => true,
            _ => return None,
        };
        if lines.next().is_some() {
            return None;
        }
        match name {
            "applied" if !rollback_failed => Some(Self::Applied),
            "unchanged" if !rollback_failed => Some(Self::Unchanged),
            "restart-required" if !rollback_failed => Some(Self::RestartRequired),
            "not-interface-owner" if !rollback_failed => Some(Self::NotInterfaceOwner),
            "rejected" if !rollback_failed => Some(Self::Rejected),
            "rolled-back" => Some(Self::RolledBack { rollback_failed }),
            _ => None,
        }
    }
}

pub fn config_digest(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

pub fn request_reload(
    paths: &ServicePaths,
    digest: [u8; 32],
) -> Result<Option<ReloadResult>, ServiceError> {
    let _control = ControlLock::acquire(paths)?;
    let Some(record) = running(paths)? else {
        return Ok(None);
    };
    let request = ReloadRequest {
        generation: record.generation,
        request_id: request_id(),
        digest,
    };
    let request_path = paths.reload_request(record.generation);
    let result_path = paths.reload_result(record.generation, request.request_id);
    remove_if_present(
        &result_path,
        "could not clear the previous prnsd interface apply result",
    )?;
    atomic_write(
        &request_path,
        request.encode().as_bytes(),
        "could not write the prnsd interface apply request",
    )?;
    let started = Instant::now();
    loop {
        match fs::read_to_string(&result_path) {
            Ok(text) => {
                let Some(result) = ReloadResult::decode(&text, &request) else {
                    let _ = remove_if_present(
                        &request_path,
                        "could not remove the invalid prnsd interface apply request",
                    );
                    let _ = remove_if_present(
                        &result_path,
                        "could not remove the invalid prnsd interface apply result",
                    );
                    return Err(ServiceError::InvalidRecord);
                };
                remove_if_present(
                    &result_path,
                    "could not remove the prnsd interface apply result",
                )?;
                return Ok(Some(result));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(source) => {
                let _ = remove_if_present(
                    &request_path,
                    "could not remove the failed prnsd interface apply request",
                );
                return Err(ServiceError::Io {
                    operation: "could not read the prnsd interface apply result",
                    source,
                });
            }
        }
        if running(paths)?.is_none_or(|current| current.generation != request.generation) {
            let _ = remove_if_present(
                &request_path,
                "could not remove the abandoned prnsd interface apply request",
            );
            return Ok(None);
        }
        if started.elapsed() >= RELOAD_TIMEOUT {
            let _ = remove_if_present(
                &request_path,
                "could not remove the timed-out prnsd interface apply request",
            );
            return Err(ServiceError::ReloadTimedOut { pid: record.pid });
        }
        thread::sleep(POLL_INTERVAL);
    }
}

fn encode_digest(digest: [u8; 32]) -> String {
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn decode_digest(text: &str) -> Option<[u8; 32]> {
    if text.len() != 64 {
        return None;
    }
    let mut digest = [0; 32];
    for (index, pair) in text.as_bytes().as_chunks::<2>().0.iter().enumerate() {
        let pair = std::str::from_utf8(pair).ok()?;
        digest[index] = u8::from_str_radix(pair, 16).ok()?;
    }
    Some(digest)
}

fn request_id() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        ^ (u128::from(std::process::id()) << 64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_and_results_round_trip_without_configuration_contents() {
        let request = ReloadRequest {
            generation: 41,
            request_id: 99,
            digest: config_digest(b"private configuration"),
        };
        let encoded = request.encode();
        assert_eq!(ReloadRequest::decode(&encoded), Some(request.clone()));
        assert!(!encoded.contains("private configuration"));

        for result in [
            ReloadResult::Applied,
            ReloadResult::Unchanged,
            ReloadResult::RestartRequired,
            ReloadResult::NotInterfaceOwner,
            ReloadResult::Rejected,
            ReloadResult::RolledBack {
                rollback_failed: false,
            },
            ReloadResult::RolledBack {
                rollback_failed: true,
            },
        ] {
            assert_eq!(
                ReloadResult::decode(&result.encode(&request), &request),
                Some(result)
            );
        }
    }
}
