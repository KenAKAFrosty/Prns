//! Host-side identity bootstrap: the OS-entropy and filesystem sides of minting the node's
//! X25519 ‖ Ed25519 identity secret, which the sans-io engine only ever takes as bytes.

use std::fs;
use std::io::{Read, Write};
use std::path::Path;

use crate::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};

/// A fresh identity secret from the OS CSPRNG.
#[must_use]
#[expect(
    clippy::expect_used,
    reason = "a host without a functioning CSPRNG cannot mint identities; failing loud beats weak keys"
)]
pub fn generate_identity_secret() -> Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]> {
    let mut key = Zeroizing::new([0u8; IDENTITY_SECRET_KEY_LEN]);
    getrandom::getrandom(&mut *key).expect("OS CSPRNG must provide identity key material");
    key
}

/// Load the identity secret at `path`, minting and persisting a fresh one when the file is
/// absent (parent directories created, unix mode `0o600`). A malformed file is refused,
/// never overwritten.
pub fn load_or_create_identity_secret(
    path: &Path,
) -> Result<Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>, IdentitySecretFileError> {
    let mut file = match fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return create_identity_secret(path)
        }
        Err(error) => return Err(IdentitySecretFileError::Io(error)),
    };
    let len = file.metadata().map_err(IdentitySecretFileError::Io)?.len();
    if len != IDENTITY_SECRET_KEY_LEN as u64 {
        return Err(IdentitySecretFileError::Malformed { len });
    }
    let mut key = Zeroizing::new([0u8; IDENTITY_SECRET_KEY_LEN]);
    file.read_exact(&mut *key)
        .map_err(IdentitySecretFileError::Io)?;
    Ok(key)
}

fn create_identity_secret(
    path: &Path,
) -> Result<Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>, IdentitySecretFileError> {
    let key = generate_identity_secret();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(IdentitySecretFileError::Io)?;
    }
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).map_err(IdentitySecretFileError::Io)?;
    file.write_all(&key[..])
        .map_err(IdentitySecretFileError::Io)?;
    Ok(key)
}

/// Why [`load_or_create_identity_secret`] produced no identity.
#[derive(Debug)]
pub enum IdentitySecretFileError {
    Io(std::io::Error),
    /// The file exists but is not exactly [`IDENTITY_SECRET_KEY_LEN`] bytes.
    Malformed {
        len: u64,
    },
}

impl core::fmt::Display for IdentitySecretFileError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            IdentitySecretFileError::Io(error) => write!(f, "identity secret file: {error}"),
            IdentitySecretFileError::Malformed { len } => write!(
                f,
                "identity secret file holds {len} bytes, not the {IDENTITY_SECRET_KEY_LEN} of an X25519 ‖ Ed25519 secret"
            ),
        }
    }
}

impl std::error::Error for IdentitySecretFileError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            IdentitySecretFileError::Io(error) => Some(error),
            IdentitySecretFileError::Malformed { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_path_mints_persists_and_reloads_the_same_secret() {
        let dir =
            std::env::temp_dir().join(format!("prns-identity-bootstrap-{}", std::process::id()));
        let path = dir.join("deeper").join("transport_identity");
        let _ = fs::remove_dir_all(&dir);

        let minted = load_or_create_identity_secret(&path).unwrap();
        let reloaded = load_or_create_identity_secret(&path).unwrap();
        assert_eq!(&minted[..], &reloaded[..]);
        assert_ne!(&minted[..], &[0u8; IDENTITY_SECRET_KEY_LEN][..]);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_malformed_file_is_refused_not_overwritten() {
        let dir = std::env::temp_dir().join(format!(
            "prns-identity-bootstrap-malformed-{}",
            std::process::id()
        ));
        let path = dir.join("transport_identity");
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(&path, b"short").unwrap();

        assert!(matches!(
            load_or_create_identity_secret(&path),
            Err(IdentitySecretFileError::Malformed { len: 5 })
        ));
        assert_eq!(fs::read(&path).unwrap(), b"short");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn generated_secrets_differ() {
        assert_ne!(
            &generate_identity_secret()[..],
            &generate_identity_secret()[..],
        );
    }
}
