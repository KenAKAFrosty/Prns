use std::io::Write;
use std::path::{Path, PathBuf};

use prns_core::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use prns_core::interfaces::shared_instance::rns_rpc::RpcAuthenticationKey;

const TRANSPORT_IDENTITY_FILE_NAME: &str = "transport_identity";

#[derive(Debug)]
pub enum RnsRpcKeyStorageError {
    ReadTransportIdentity {
        path: PathBuf,
        source: std::io::Error,
    },
    CreateStorageDirectory {
        path: PathBuf,
        source: std::io::Error,
    },
    CreateTransportIdentity {
        path: PathBuf,
        source: std::io::Error,
    },
    WriteTransportIdentity {
        path: PathBuf,
        source: std::io::Error,
    },
    SyncTransportIdentity {
        path: PathBuf,
        source: std::io::Error,
    },
    InvalidTransportIdentityLength {
        path: PathBuf,
        expected: usize,
        actual: usize,
    },
}

impl core::fmt::Display for RnsRpcKeyStorageError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::ReadTransportIdentity { path, source } => write!(
                formatter,
                "could not read RNS transport identity at {}: {source}",
                path.display()
            ),
            Self::CreateStorageDirectory { path, source } => write!(
                formatter,
                "could not create RNS storage directory {}: {source}",
                path.display()
            ),
            Self::CreateTransportIdentity { path, source } => write!(
                formatter,
                "could not create RNS transport identity at {}: {source}",
                path.display()
            ),
            Self::WriteTransportIdentity { path, source } => write!(
                formatter,
                "could not write RNS transport identity at {}: {source}",
                path.display()
            ),
            Self::SyncTransportIdentity { path, source } => write!(
                formatter,
                "could not durably store RNS transport identity at {}: {source}",
                path.display()
            ),
            Self::InvalidTransportIdentityLength {
                path,
                expected,
                actual,
            } => write!(
                formatter,
                "RNS transport identity at {} is {actual} bytes; expected {expected} bytes",
                path.display()
            ),
        }
    }
}

impl std::error::Error for RnsRpcKeyStorageError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::ReadTransportIdentity { source, .. }
            | Self::CreateStorageDirectory { source, .. }
            | Self::CreateTransportIdentity { source, .. }
            | Self::WriteTransportIdentity { source, .. }
            | Self::SyncTransportIdentity { source, .. } => Some(source),
            Self::InvalidTransportIdentityLength { .. } => None,
        }
    }
}

pub fn load_or_seed_rns_rpc_key(
    storage_dir: &Path,
    seed_if_absent: &[u8; IDENTITY_SECRET_KEY_LEN],
) -> Result<RpcAuthenticationKey, RnsRpcKeyStorageError> {
    let path = storage_dir.join(TRANSPORT_IDENTITY_FILE_NAME);
    match read_rpc_key(&path) {
        Ok(rpc_key) => Ok(rpc_key),
        Err(RnsRpcKeyStorageError::ReadTransportIdentity { source, .. })
            if source.kind() == std::io::ErrorKind::NotFound =>
        {
            seed_rpc_key(storage_dir, &path, seed_if_absent)
        }
        Err(error) => Err(error),
    }
}

fn read_rpc_key(path: &Path) -> Result<RpcAuthenticationKey, RnsRpcKeyStorageError> {
    let bytes = Zeroizing::new(std::fs::read(path).map_err(|source| {
        RnsRpcKeyStorageError::ReadTransportIdentity {
            path: path.to_path_buf(),
            source,
        }
    })?);
    let secret: [u8; IDENTITY_SECRET_KEY_LEN] = bytes.as_slice().try_into().map_err(|_| {
        RnsRpcKeyStorageError::InvalidTransportIdentityLength {
            path: path.to_path_buf(),
            expected: IDENTITY_SECRET_KEY_LEN,
            actual: bytes.len(),
        }
    })?;
    let secret = Zeroizing::new(secret);
    Ok(RpcAuthenticationKey::from_rns_transport_identity_secret(
        &secret,
    ))
}

fn seed_rpc_key(
    storage_dir: &Path,
    path: &Path,
    seed: &[u8; IDENTITY_SECRET_KEY_LEN],
) -> Result<RpcAuthenticationKey, RnsRpcKeyStorageError> {
    std::fs::create_dir_all(storage_dir).map_err(|source| {
        RnsRpcKeyStorageError::CreateStorageDirectory {
            path: storage_dir.to_path_buf(),
            source,
        }
    })?;
    let mut file = match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
    {
        Ok(file) => file,
        Err(source) if source.kind() == std::io::ErrorKind::AlreadyExists => {
            return read_rpc_key(path);
        }
        Err(source) => {
            return Err(RnsRpcKeyStorageError::CreateTransportIdentity {
                path: path.to_path_buf(),
                source,
            });
        }
    };
    if let Err(source) = file.write_all(seed) {
        drop(file);
        let _ = std::fs::remove_file(path);
        return Err(RnsRpcKeyStorageError::WriteTransportIdentity {
            path: path.to_path_buf(),
            source,
        });
    }
    if let Err(source) = file.sync_all() {
        drop(file);
        let _ = std::fs::remove_file(path);
        return Err(RnsRpcKeyStorageError::SyncTransportIdentity {
            path: path.to_path_buf(),
            source,
        });
    }
    Ok(RpcAuthenticationKey::from_rns_transport_identity_secret(
        seed,
    ))
}

#[must_use]
pub fn reticulum_storage_dir() -> PathBuf {
    std::env::var_os("RETICULUM_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".reticulum")
        })
        .join("storage")
}
