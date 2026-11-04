//! Private filesystem mechanics shared by native domain owners.
//!
//! Callers own the containing directory and domain recovery policy. These
//! checks reject symlinks at the supplied path, but do not protect against an
//! untrusted process replacing arbitrary ancestor directories concurrently.
//! Creation and confirmation are separate: an I/O error may follow a successful
//! create or publication. Preserve resulting state for domain reconciliation.

#[cfg(unix)]
use std::fs::{self, OpenOptions};
#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::{fmt, fs::File, path::Path};

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    UnsafePath,
    UnsafePermissions,
    UnsupportedPlatform,
}
impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}
impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => error.fmt(formatter),
            Self::UnsafePath => {
                formatter.write_str("path is not a private regular file or directory")
            }
            Self::UnsafePermissions => {
                formatter.write_str("path permissions or link count are unsafe")
            }
            Self::UnsupportedPlatform => {
                formatter.write_str("private durable files are unsupported on this platform")
            }
        }
    }
}
impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}
pub type Result<T> = std::result::Result<T, Error>;

/// Create exactly one new private directory, validate it, then confirm its
/// parent entry. Does not create ancestors or repair existing permissions.
pub fn create_private_directory(path: &Path) -> Result<()> {
    #[cfg(not(unix))]
    {
        let _ = path;
        Err(Error::UnsupportedPlatform)
    }
    #[cfg(unix)]
    {
        let mut builder = fs::DirBuilder::new();
        builder.mode(0o700);
        builder.create(path)?;
        check_private_directory(path)?;
        if let Some(parent) = path.parent() {
            sync_directory(parent)?;
        }
        Ok(())
    }
}

pub fn check_private_directory(path: &Path) -> Result<()> {
    #[cfg(not(unix))]
    {
        let _ = path;
        Err(Error::UnsupportedPlatform)
    }
    #[cfg(unix)]
    {
        let metadata = fs::symlink_metadata(path)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(Error::UnsafePath);
        }
        if metadata.permissions().mode() & 0o077 != 0 {
            return Err(Error::UnsafePermissions);
        }
        Ok(())
    }
}

/// Missing is an ordinary fact; a present non-private/non-regular path is an
/// error. Existing permissions and contents are never changed by validation.
pub fn check_private_file(path: &Path) -> Result<bool> {
    #[cfg(not(unix))]
    {
        let _ = path;
        Err(Error::UnsupportedPlatform)
    }
    #[cfg(unix)]
    {
        let metadata = match fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error.into()),
        };
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(Error::UnsafePath);
        }
        if metadata.permissions().mode() & 0o077 != 0 || metadata.nlink() != 1 {
            return Err(Error::UnsafePermissions);
        }
        Ok(true)
    }
}

/// Create a new private read/write file; never truncate or replace a path.
/// The caller writes its content and confirms the file and parent directory.
pub fn create_private_file(path: &Path) -> Result<File> {
    #[cfg(not(unix))]
    {
        let _ = path;
        Err(Error::UnsupportedPlatform)
    }
    #[cfg(unix)]
    {
        let mut options = OpenOptions::new();
        options.write(true).read(true).create_new(true).mode(0o600);
        let file = options.open(path)?;
        check_private_file(path)?;
        Ok(file)
    }
}

/// Confirm directory-entry durability. The domain decides whether publication
/// occurred before a failed confirmation and how to recover that uncertainty.
pub fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)?.sync_all()?;
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use std::{
        io::{Read, Write},
        path::PathBuf,
        sync::atomic::{AtomicUsize, Ordering},
        time::SystemTime,
    };
    struct Directory(PathBuf);
    impl Directory {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "prns-storage-files-{}-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            create_private_directory(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Directory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn creation_is_explicit_and_validation_preserves_existing_content() {
        let directory = Directory::new();
        let path = directory.0.join("state");
        assert!(!check_private_file(&path).unwrap());
        let mut file = create_private_file(&path).unwrap();
        file.write_all(b"retained").unwrap();
        file.sync_all().unwrap();
        sync_directory(&directory.0).unwrap();
        assert!(check_private_file(&path).unwrap());
        assert!(
            matches!(create_private_file(&path), Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::AlreadyExists)
        );
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(matches!(
            check_private_file(&path),
            Err(Error::UnsafePermissions)
        ));
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o644
        );
        let mut content = String::new();
        File::open(path)
            .unwrap()
            .read_to_string(&mut content)
            .unwrap();
        assert_eq!(content, "retained");
    }
    #[test]
    fn symlinks_hard_links_and_non_private_directories_are_rejected() {
        use std::os::unix::fs::symlink;
        let directory = Directory::new();
        let path = directory.0.join("state");
        create_private_file(&path).unwrap();
        let link = directory.0.join("link");
        symlink(&path, &link).unwrap();
        assert!(matches!(check_private_file(&link), Err(Error::UnsafePath)));
        assert!(
            matches!(create_private_file(&link), Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::AlreadyExists)
        );
        let hard = directory.0.join("hard");
        fs::hard_link(&path, &hard).unwrap();
        assert!(matches!(
            check_private_file(&path),
            Err(Error::UnsafePermissions)
        ));
        assert!(matches!(
            check_private_file(&directory.0),
            Err(Error::UnsafePath)
        ));
        let dir_link = directory.0.join("dir-link");
        symlink(&directory.0, &dir_link).unwrap();
        assert!(matches!(
            check_private_directory(&dir_link),
            Err(Error::UnsafePath)
        ));
        fs::set_permissions(&directory.0, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(matches!(
            check_private_directory(&directory.0),
            Err(Error::UnsafePermissions)
        ));
    }
}
