use std::fs::{self, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use personal_rns::rncp::{parse_file_metadata, RncpCodecError};
use tokio::fs::File;

static STAGING_SEQUENCE: AtomicU32 = AtomicU32::new(0);

pub struct ReceiveTarget {
    directory: PathBuf,
    staging: PathBuf,
    pub file: File,
}

#[derive(Debug)]
pub enum CpIoError {
    HomeUnavailable(PathBuf),
    InvalidDirectory(PathBuf),
    OutsideJail { path: PathBuf, jail: PathBuf },
    NotAFile(PathBuf),
    InvalidMetadata(RncpCodecError),
    InvalidFilename,
    Io { path: PathBuf, source: io::Error },
}

pub fn expand_user_path(path: &Path) -> Result<PathBuf, CpIoError> {
    let Some(text) = path.to_str() else {
        return Ok(path.to_owned());
    };
    let suffix = text.strip_prefix("~/").or_else(|| text.strip_prefix("~\\"));
    if text != "~" && suffix.is_none() {
        return Ok(path.to_owned());
    }
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"));
    let Some(home) = home.filter(|value| !value.is_empty()) else {
        return Err(CpIoError::HomeUnavailable(path.to_owned()));
    };
    Ok(suffix.map_or_else(
        || PathBuf::from(&home),
        |suffix| PathBuf::from(&home).join(suffix),
    ))
}

pub fn canonical_directory(path: &Path) -> Result<PathBuf, CpIoError> {
    let expanded = expand_user_path(path)?;
    let canonical = fs::canonicalize(&expanded).map_err(|source| CpIoError::Io {
        path: expanded.clone(),
        source,
    })?;
    if !canonical.is_dir() {
        return Err(CpIoError::InvalidDirectory(canonical));
    }
    Ok(canonical)
}

pub fn resolve_fetch(path: &str, jail: Option<&Path>) -> Result<PathBuf, CpIoError> {
    let requested = expand_user_path(Path::new(path))?;
    let candidate = match jail {
        Some(jail) => {
            let relative = if requested.is_absolute() {
                requested.strip_prefix(jail).unwrap_or(&requested)
            } else {
                requested.as_path()
            };
            jail.join(relative)
        }
        None => requested,
    };
    let canonical = fs::canonicalize(&candidate).map_err(|source| CpIoError::Io {
        path: candidate.clone(),
        source,
    })?;
    if let Some(jail) = jail {
        if !canonical.starts_with(jail) || canonical == jail {
            return Err(CpIoError::OutsideJail {
                path: canonical,
                jail: jail.to_owned(),
            });
        }
    }
    if !canonical.is_file() {
        return Err(CpIoError::NotAFile(canonical));
    }
    Ok(canonical)
}

impl ReceiveTarget {
    pub fn create(directory: &Path) -> Result<Self, CpIoError> {
        loop {
            let sequence = STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let staging =
                directory.join(format!(".rncp.{}.{}.staging", std::process::id(), sequence));
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            match options.open(&staging) {
                Ok(file) => {
                    return Ok(Self {
                        directory: directory.to_owned(),
                        staging,
                        file: File::from_std(file),
                    })
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(source) => {
                    return Err(CpIoError::Io {
                        path: staging,
                        source,
                    })
                }
            }
        }
    }

    pub async fn publish(mut self, metadata: &[u8], overwrite: bool) -> Result<PathBuf, CpIoError> {
        use tokio::io::AsyncWriteExt;

        self.file.flush().await.map_err(|source| CpIoError::Io {
            path: self.staging.clone(),
            source,
        })?;
        self.file.sync_all().await.map_err(|source| CpIoError::Io {
            path: self.staging.clone(),
            source,
        })?;
        let name =
            safe_filename(parse_file_metadata(metadata).map_err(CpIoError::InvalidMetadata)?)?;
        let base = self.directory.join(name);
        if overwrite {
            fs::rename(&self.staging, &base).map_err(|source| CpIoError::Io {
                path: base.clone(),
                source,
            })?;
            return Ok(base);
        }
        let mut counter = 0u64;
        loop {
            let candidate = if counter == 0 {
                base.clone()
            } else {
                PathBuf::from(format!("{}.{}", base.display(), counter))
            };
            match fs::hard_link(&self.staging, &candidate) {
                Ok(()) => {
                    fs::remove_file(&self.staging).map_err(|source| CpIoError::Io {
                        path: self.staging.clone(),
                        source,
                    })?;
                    return Ok(candidate);
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    counter = counter.saturating_add(1);
                }
                Err(source) => {
                    return Err(CpIoError::Io {
                        path: candidate,
                        source,
                    })
                }
            }
        }
    }
}

impl Drop for ReceiveTarget {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.staging);
    }
}

fn safe_filename(bytes: &[u8]) -> Result<&str, CpIoError> {
    let name = std::str::from_utf8(bytes).map_err(|_| CpIoError::InvalidFilename)?;
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.as_bytes().contains(&b'/')
        || name.as_bytes().contains(&b'\\')
    {
        return Err(CpIoError::InvalidFilename);
    }
    Ok(name)
}

impl std::fmt::Display for CpIoError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::HomeUnavailable(path) => write!(
                formatter,
                "path {} requires a home directory, but none is available",
                path.display()
            ),
            Self::InvalidDirectory(path) => {
                write!(formatter, "{} is not a directory", path.display())
            }
            Self::OutsideJail { path, jail } => write!(
                formatter,
                "{} resolves outside fetch jail {}",
                path.display(),
                jail.display()
            ),
            Self::NotAFile(path) => write!(formatter, "{} is not a file", path.display()),
            Self::InvalidMetadata(source) => write!(formatter, "invalid RNCP metadata: {source:?}"),
            Self::InvalidFilename => formatter.write_str("invalid RNCP filename metadata"),
            Self::Io { path, source } => write!(formatter, "{}: {source}", path.display()),
        }
    }
}

impl std::error::Error for CpIoError {}

#[cfg(test)]
mod tests {
    use super::*;
    use personal_rns::rncp::write_file_metadata;

    fn directory(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "prnsd-rncp-{name}-{}-{}",
            std::process::id(),
            STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[tokio::test]
    async fn no_clobber_publish_chooses_an_atomic_postfix() {
        let directory = directory("publish");
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("case.bin"), b"existing").unwrap();
        let mut target = ReceiveTarget::create(&directory).unwrap();
        tokio::io::AsyncWriteExt::write_all(&mut target.file, b"new")
            .await
            .unwrap();
        let mut metadata = [0u8; 64];
        let len = write_file_metadata(b"case.bin", &mut metadata).unwrap();
        let published = target.publish(&metadata[..len], false).await.unwrap();
        assert_eq!(published, directory.join("case.bin.1"));
        assert_eq!(fs::read(directory.join("case.bin")).unwrap(), b"existing");
        assert_eq!(fs::read(published).unwrap(), b"new");
        fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn fetch_jail_follows_symlinks_then_enforces_the_canonical_boundary() {
        use std::os::unix::fs::symlink;

        let root = directory("jail");
        let jail = root.join("jail");
        let outside = root.join("outside");
        fs::create_dir_all(&jail).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(jail.join("inside"), b"ok").unwrap();
        fs::write(outside.join("secret"), b"no").unwrap();
        symlink(jail.join("inside"), jail.join("safe-link")).unwrap();
        symlink(outside.join("secret"), jail.join("escape-link")).unwrap();
        let jail = canonical_directory(&jail).unwrap();
        assert_eq!(
            resolve_fetch("safe-link", Some(&jail)).unwrap(),
            jail.join("inside")
        );
        assert!(matches!(
            resolve_fetch("escape-link", Some(&jail)),
            Err(CpIoError::OutsideJail { .. })
        ));
        fs::remove_dir_all(root).unwrap();
    }
}
