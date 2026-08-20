use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;

pub const LOCAL_RPC_KEY_FILE: &str = "local_rpc_key";

#[derive(Debug)]
pub enum LocalRpcKeyError {
    Read(std::io::Error),
    Write(std::io::Error),
    Length { expected: usize, actual: usize },
    Random,
}

impl core::fmt::Display for LocalRpcKeyError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Read(error) => write!(formatter, "could not read local RPC key: {error}"),
            Self::Write(error) => write!(formatter, "could not write local RPC key: {error}"),
            Self::Length { expected, actual } => write!(
                formatter,
                "local RPC key is {actual} bytes; expected {expected} bytes"
            ),
            Self::Random => formatter.write_str("local RPC key entropy unavailable"),
        }
    }
}

pub fn load_or_create_local_rpc_key(storage_dir: &Path) -> Result<[u8; 32], LocalRpcKeyError> {
    let path = storage_dir.join(LOCAL_RPC_KEY_FILE);
    match read_key(&path) {
        Ok(key) => Ok(key),
        Err(LocalRpcKeyError::Read(error)) if error.kind() == std::io::ErrorKind::NotFound => {
            create_key(storage_dir, &path)
        }
        Err(error) => Err(error),
    }
}

fn read_key(path: &Path) -> Result<[u8; 32], LocalRpcKeyError> {
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(LocalRpcKeyError::Read)?
        .read_to_end(&mut bytes)
        .map_err(LocalRpcKeyError::Read)?;
    bytes
        .as_slice()
        .try_into()
        .map_err(|_| LocalRpcKeyError::Length {
            expected: 32,
            actual: bytes.len(),
        })
}

fn create_key(storage_dir: &Path, path: &Path) -> Result<[u8; 32], LocalRpcKeyError> {
    std::fs::create_dir_all(storage_dir).map_err(LocalRpcKeyError::Write)?;
    let mut key = [0u8; 32];
    getrandom::getrandom(&mut key).map_err(|_| LocalRpcKeyError::Random)?;
    let mut file = match OpenOptions::new().write(true).create_new(true).open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => return read_key(path),
        Err(error) => return Err(LocalRpcKeyError::Write(error)),
    };
    file.write_all(&key).map_err(LocalRpcKeyError::Write)?;
    file.sync_all().map_err(LocalRpcKeyError::Write)?;
    Ok(key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process;

    #[test]
    fn local_rpc_key_is_created_once_and_reloaded() {
        let storage_dir =
            std::env::temp_dir().join(format!("personal-hopspot-local-rpc-{}", process::id()));
        let _ = std::fs::remove_dir_all(&storage_dir);

        let first = load_or_create_local_rpc_key(&storage_dir).unwrap();
        let second = load_or_create_local_rpc_key(&storage_dir).unwrap();
        assert_eq!(first, second);
        assert_ne!(first, [0u8; 32]);

        let _ = std::fs::remove_dir_all(storage_dir);
    }
}
