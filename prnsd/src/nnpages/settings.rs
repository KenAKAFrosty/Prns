use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;

pub(crate) const SETTINGS_FILE_NAME: &str = "settings.toml";
pub(crate) const DEFAULT_ANNOUNCE_INTERVAL_MINUTES: u64 = 6 * 60;
pub(crate) const DEFAULT_SETTINGS_DOCUMENT: &str =
    "announce = true\nannounce_interval_minutes = 360\n";
const SECONDS_PER_MINUTE: u64 = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct NnPagesSettings {
    announce: bool,
    announce_interval: Duration,
}

impl NnPagesSettings {
    pub(crate) fn new(
        announce: bool,
        announce_interval_minutes: u64,
    ) -> Result<Self, NnPagesSettingsError> {
        let seconds = announce_interval_minutes
            .checked_mul(SECONDS_PER_MINUTE)
            .filter(|seconds| *seconds != 0)
            .ok_or(NnPagesSettingsError::InvalidAnnounceInterval {
                minutes: announce_interval_minutes,
            })?;
        Ok(Self {
            announce,
            announce_interval: Duration::from_secs(seconds),
        })
    }

    pub(crate) const fn announce(self) -> bool {
        self.announce
    }

    pub(crate) const fn announce_interval(self) -> Duration {
        self.announce_interval
    }

    pub(crate) const fn announce_interval_minutes(self) -> u64 {
        self.announce_interval.as_secs() / SECONDS_PER_MINUTE
    }
}

impl Default for NnPagesSettings {
    fn default() -> Self {
        Self {
            announce: true,
            announce_interval: Duration::from_secs(
                DEFAULT_ANNOUNCE_INTERVAL_MINUTES * SECONDS_PER_MINUTE,
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NnPagesSettingsError {
    Read {
        path: PathBuf,
        kind: io::ErrorKind,
        message: String,
    },
    Parse {
        path: PathBuf,
        message: String,
    },
    InvalidTarget {
        path: PathBuf,
    },
    InvalidAnnounceInterval {
        minutes: u64,
    },
}

impl core::fmt::Display for NnPagesSettingsError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Read {
                path,
                kind,
                message,
            } => {
                write!(
                    formatter,
                    "could not read {} ({kind:?}): {message}",
                    path.display()
                )
            }
            Self::Parse { path, message } => {
                write!(formatter, "could not parse {}: {message}", path.display())
            }
            Self::InvalidTarget { path } => {
                write!(formatter, "{} is not a regular settings file", path.display())
            }
            Self::InvalidAnnounceInterval { minutes } => write!(
                formatter,
                "announce_interval_minutes must be a positive whole number representable as a duration, got {minutes}"
            ),
        }
    }
}

impl std::error::Error for NnPagesSettingsError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NnPagesSettingsStatus {
    Loaded,
    MissingDefaults,
    InvalidDefaults,
}

impl NnPagesSettingsStatus {
    pub(crate) const fn as_control_value(self) -> &'static str {
        match self {
            Self::Loaded => "loaded",
            Self::MissingDefaults => "missing-defaults",
            Self::InvalidDefaults => "invalid-defaults",
        }
    }

    pub(crate) fn from_control_value(value: &str) -> Option<Self> {
        match value {
            "loaded" => Some(Self::Loaded),
            "missing-defaults" => Some(Self::MissingDefaults),
            "invalid-defaults" => Some(Self::InvalidDefaults),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NnPagesSettingsSnapshot {
    effective: NnPagesSettings,
    source: NnPagesSettingsSource,
}

impl NnPagesSettingsSnapshot {
    pub(crate) const fn effective(&self) -> NnPagesSettings {
        self.effective
    }

    pub(crate) const fn status(&self) -> NnPagesSettingsStatus {
        match self.source {
            NnPagesSettingsSource::Loaded => NnPagesSettingsStatus::Loaded,
            NnPagesSettingsSource::Missing => NnPagesSettingsStatus::MissingDefaults,
            NnPagesSettingsSource::Invalid(_) => NnPagesSettingsStatus::InvalidDefaults,
        }
    }

    pub(crate) fn diagnostic(&self) -> Option<&NnPagesSettingsError> {
        match &self.source {
            NnPagesSettingsSource::Invalid(error) => Some(error),
            NnPagesSettingsSource::Loaded | NnPagesSettingsSource::Missing => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum NnPagesSettingsSource {
    Loaded,
    Missing,
    Invalid(NnPagesSettingsError),
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct NnPagesSettingsDocument {
    announce: bool,
    announce_interval_minutes: u64,
}

impl Default for NnPagesSettingsDocument {
    fn default() -> Self {
        Self {
            announce: true,
            announce_interval_minutes: DEFAULT_ANNOUNCE_INTERVAL_MINUTES,
        }
    }
}

pub(crate) fn load(root: &Path) -> NnPagesSettingsSnapshot {
    let path = root.join(SETTINGS_FILE_NAME);
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.file_type().is_file() => {}
        Ok(_) => {
            return invalid_snapshot(NnPagesSettingsError::InvalidTarget { path });
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return NnPagesSettingsSnapshot {
                effective: NnPagesSettings::default(),
                source: NnPagesSettingsSource::Missing,
            };
        }
        Err(error) => {
            return invalid_snapshot(NnPagesSettingsError::Read {
                path,
                kind: error.kind(),
                message: error.to_string(),
            });
        }
    }
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) => {
            return invalid_snapshot(NnPagesSettingsError::Read {
                path,
                kind: error.kind(),
                message: error.to_string(),
            });
        }
    };
    let document = match toml::from_str::<NnPagesSettingsDocument>(&text) {
        Ok(document) => document,
        Err(error) => {
            return invalid_snapshot(NnPagesSettingsError::Parse {
                path,
                message: error.to_string(),
            });
        }
    };
    match NnPagesSettings::new(document.announce, document.announce_interval_minutes) {
        Ok(effective) => NnPagesSettingsSnapshot {
            effective,
            source: NnPagesSettingsSource::Loaded,
        },
        Err(error) => invalid_snapshot(error),
    }
}

fn invalid_snapshot(error: NnPagesSettingsError) -> NnPagesSettingsSnapshot {
    NnPagesSettingsSnapshot {
        effective: NnPagesSettings::default(),
        source: NnPagesSettingsSource::Invalid(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_and_partial_documents_use_defaults() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let missing = load(directory.path());
        assert_eq!(missing.status(), NnPagesSettingsStatus::MissingDefaults);
        assert_eq!(missing.effective(), NnPagesSettings::default());

        fs::write(
            directory.path().join(SETTINGS_FILE_NAME),
            "# operator policy\nannounce = false\n",
        )
        .expect("settings");
        let partial = load(directory.path());
        assert_eq!(partial.status(), NnPagesSettingsStatus::Loaded);
        assert!(!partial.effective().announce());
        assert_eq!(
            partial.effective().announce_interval_minutes(),
            DEFAULT_ANNOUNCE_INTERVAL_MINUTES
        );
    }

    #[test]
    fn complete_document_uses_native_toml_types() {
        let directory = tempfile::tempdir().expect("temporary directory");
        fs::write(
            directory.path().join(SETTINGS_FILE_NAME),
            "announce = true\nannounce_interval_minutes = 45\n",
        )
        .expect("settings");
        let snapshot = load(directory.path());
        assert_eq!(snapshot.status(), NnPagesSettingsStatus::Loaded);
        assert!(snapshot.effective().announce());
        assert_eq!(snapshot.effective().announce_interval_minutes(), 45);
    }

    #[test]
    fn malformed_unknown_duplicate_and_zero_values_use_complete_defaults() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let path = directory.path().join(SETTINGS_FILE_NAME);
        for text in [
            "announce = \"yes\"\n",
            "annouce = false\n",
            "announce = true\nannounce = false\n",
            "announce = false\nannounce_interval_minutes = 0\n",
        ] {
            fs::write(&path, text).expect("settings");
            let snapshot = load(directory.path());
            assert_eq!(snapshot.status(), NnPagesSettingsStatus::InvalidDefaults);
            assert_eq!(snapshot.effective(), NnPagesSettings::default());
            assert!(snapshot.diagnostic().is_some());
        }
    }

    #[test]
    fn unreadable_settings_target_uses_complete_defaults() {
        let directory = tempfile::tempdir().expect("temporary directory");
        fs::create_dir(directory.path().join(SETTINGS_FILE_NAME)).expect("settings directory");
        let snapshot = load(directory.path());
        assert_eq!(snapshot.status(), NnPagesSettingsStatus::InvalidDefaults);
        assert_eq!(snapshot.effective(), NnPagesSettings::default());
        assert!(matches!(
            snapshot.diagnostic(),
            Some(NnPagesSettingsError::InvalidTarget { .. })
        ));
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_settings_are_not_followed() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().expect("temporary directory");
        let external = directory.path().join("external.toml");
        fs::write(&external, "announce = false\n").expect("external settings");
        let root = directory.path().join("nnpages");
        fs::create_dir(&root).expect("NNPages root");
        symlink(&external, root.join(SETTINGS_FILE_NAME)).expect("settings symlink");

        let snapshot = load(&root);
        assert_eq!(snapshot.status(), NnPagesSettingsStatus::InvalidDefaults);
        assert_eq!(snapshot.effective(), NnPagesSettings::default());
        assert!(matches!(
            snapshot.diagnostic(),
            Some(NnPagesSettingsError::InvalidTarget { .. })
        ));
    }
}
