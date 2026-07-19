use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

static STAGING_SEQUENCE: AtomicU32 = AtomicU32::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverwritePolicy {
    Refuse,
    Replace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputSensitivity {
    Ordinary,
    Private,
}

pub enum OutputSink {
    Stdout(io::Stdout),
    File(AtomicOutput),
}

pub struct AtomicOutput {
    final_path: PathBuf,
    staging_path: PathBuf,
    file: Option<File>,
    overwrite: OverwritePolicy,
}

#[derive(Debug)]
pub enum IdentityIoError {
    HomeUnavailable(PathBuf),
    Read { path: PathBuf, source: io::Error },
    Write { path: PathBuf, source: io::Error },
    AlreadyExists(PathBuf),
    InvalidOutputPath(PathBuf),
}

impl OutputSink {
    pub fn stdout() -> Self {
        Self::Stdout(io::stdout())
    }

    pub fn file(
        path: &Path,
        overwrite: OverwritePolicy,
        sensitivity: OutputSensitivity,
    ) -> Result<Self, IdentityIoError> {
        AtomicOutput::create(path, overwrite, sensitivity).map(Self::File)
    }

    pub fn finish(self) -> Result<(), IdentityIoError> {
        match self {
            Self::Stdout(mut stdout) => stdout.flush().map_err(|source| IdentityIoError::Write {
                path: PathBuf::from("<stdout>"),
                source,
            }),
            Self::File(output) => output.commit(),
        }
    }
}

impl Write for OutputSink {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        match self {
            Self::Stdout(stdout) => stdout.write(bytes),
            Self::File(output) => output
                .file
                .as_mut()
                .ok_or_else(|| io::Error::other("output is already committed"))?
                .write(bytes),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Stdout(stdout) => stdout.flush(),
            Self::File(output) => output
                .file
                .as_mut()
                .ok_or_else(|| io::Error::other("output is already committed"))?
                .flush(),
        }
    }
}

impl AtomicOutput {
    fn create(
        path: &Path,
        overwrite: OverwritePolicy,
        sensitivity: OutputSensitivity,
    ) -> Result<Self, IdentityIoError> {
        let final_path = expand_user_path(path)?;
        if overwrite == OverwritePolicy::Refuse && final_path.exists() {
            return Err(IdentityIoError::AlreadyExists(final_path));
        }
        let parent = final_path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let file_name = final_path
            .file_name()
            .ok_or_else(|| IdentityIoError::InvalidOutputPath(final_path.clone()))?;
        let (staging_path, file) = loop {
            let sequence = STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let staging_path = parent.join(format!(
                ".{}.{}.{}.staging",
                file_name.to_string_lossy(),
                std::process::id(),
                sequence
            ));
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(match sensitivity {
                    OutputSensitivity::Ordinary => 0o644,
                    OutputSensitivity::Private => 0o600,
                });
            }
            match options.open(&staging_path) {
                Ok(file) => break (staging_path, file),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(source) => {
                    return Err(IdentityIoError::Write {
                        path: staging_path,
                        source,
                    });
                }
            }
        };
        Ok(Self {
            final_path,
            staging_path,
            file: Some(file),
            overwrite,
        })
    }

    fn commit(mut self) -> Result<(), IdentityIoError> {
        let mut file = self.file.take().ok_or_else(|| IdentityIoError::Write {
            path: self.staging_path.clone(),
            source: io::Error::other("output is already committed"),
        })?;
        file.flush().map_err(|source| IdentityIoError::Write {
            path: self.staging_path.clone(),
            source,
        })?;
        file.sync_all().map_err(|source| IdentityIoError::Write {
            path: self.staging_path.clone(),
            source,
        })?;
        drop(file);
        let published = match self.overwrite {
            OverwritePolicy::Refuse => fs::hard_link(&self.staging_path, &self.final_path),
            OverwritePolicy::Replace => fs::rename(&self.staging_path, &self.final_path),
        };
        if let Err(source) = published {
            let _ = fs::remove_file(&self.staging_path);
            if source.kind() == io::ErrorKind::AlreadyExists {
                return Err(IdentityIoError::AlreadyExists(self.final_path.clone()));
            }
            return Err(IdentityIoError::Write {
                path: self.final_path.clone(),
                source,
            });
        }
        if self.overwrite == OverwritePolicy::Refuse {
            fs::remove_file(&self.staging_path).map_err(|source| IdentityIoError::Write {
                path: self.staging_path.clone(),
                source,
            })?;
        }
        Ok(())
    }
}

impl Drop for AtomicOutput {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.staging_path);
    }
}

pub fn read_file(path: &Path) -> Result<Vec<u8>, IdentityIoError> {
    let path = expand_user_path(path)?;
    fs::read(&path).map_err(|source| IdentityIoError::Read { path, source })
}

pub fn read_stdin() -> Result<Vec<u8>, IdentityIoError> {
    let mut bytes = Vec::new();
    io::stdin()
        .read_to_end(&mut bytes)
        .map_err(|source| IdentityIoError::Read {
            path: PathBuf::from("<stdin>"),
            source,
        })?;
    Ok(bytes)
}

pub fn expand_user_path(path: &Path) -> Result<PathBuf, IdentityIoError> {
    let Some(path_text) = path.to_str() else {
        return Ok(path.to_owned());
    };
    let remainder = path_text
        .strip_prefix("~/")
        .or_else(|| path_text.strip_prefix("~\\"));
    if path_text != "~" && remainder.is_none() {
        return Ok(path.to_owned());
    }
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"));
    let Some(home) = home.filter(|home| !home.is_empty()) else {
        return Err(IdentityIoError::HomeUnavailable(path.to_owned()));
    };
    Ok(remainder.map_or_else(
        || PathBuf::from(&home),
        |suffix| PathBuf::from(&home).join(suffix),
    ))
}

impl std::fmt::Display for IdentityIoError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::HomeUnavailable(path) => write!(
                formatter,
                "path {} requires a home directory, but neither HOME nor USERPROFILE is available",
                path.display()
            ),
            Self::Read { path, source } => {
                write!(formatter, "could not read {}: {source}", path.display())
            }
            Self::Write { path, source } => {
                write!(formatter, "could not write {}: {source}", path.display())
            }
            Self::AlreadyExists(path) => write!(
                formatter,
                "file {} already exists, not overwriting",
                path.display()
            ),
            Self::InvalidOutputPath(path) => {
                write!(formatter, "{} is not a valid output path", path.display())
            }
        }
    }
}

impl std::error::Error for IdentityIoError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Read { source, .. } | Self::Write { source, .. } => Some(source),
            Self::HomeUnavailable(_) | Self::AlreadyExists(_) | Self::InvalidOutputPath(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_directory(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "prnsd-rnid-{name}-{}-{}",
            std::process::id(),
            STAGING_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ))
    }

    #[test]
    fn no_clobber_publish_preserves_a_racing_destination_and_cleans_staging() {
        let directory = test_directory("no-clobber");
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("identity.pub");
        let mut output =
            OutputSink::file(&path, OverwritePolicy::Refuse, OutputSensitivity::Ordinary).unwrap();
        output.write_all(b"new").unwrap();
        fs::write(&path, b"existing").unwrap();

        assert!(matches!(
            output.finish(),
            Err(IdentityIoError::AlreadyExists(existing)) if existing == path
        ));
        assert_eq!(fs::read(&path).unwrap(), b"existing");
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 1);

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn abandoned_output_leaves_neither_destination_nor_staging_file() {
        let directory = test_directory("abandoned");
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("opened");
        let mut output =
            OutputSink::file(&path, OverwritePolicy::Refuse, OutputSensitivity::Ordinary).unwrap();
        output.write_all(b"partial").unwrap();
        drop(output);

        assert!(!path.exists());
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 0);

        fs::remove_dir_all(directory).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn private_outputs_are_created_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let directory = test_directory("private-mode");
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("identity.rid");
        let mut output =
            OutputSink::file(&path, OverwritePolicy::Refuse, OutputSensitivity::Private).unwrap();
        output.write_all(b"secret").unwrap();
        output.finish().unwrap();

        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );

        fs::remove_dir_all(directory).unwrap();
    }
}
