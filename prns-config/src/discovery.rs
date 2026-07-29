use std::fmt;
use std::path::{Path, PathBuf};

use prns_core::persistence::reticulum_directory;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredConfig {
    pub dir: PathBuf,
    pub config: Option<PathBuf>,
}

impl DiscoveredConfig {
    pub fn is_empty(&self) -> bool {
        self.config.is_none()
    }
}

#[derive(Debug)]
pub enum DiscoveryError {
    HomeDirectoryUnavailable,
}

impl fmt::Display for DiscoveryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::HomeDirectoryUnavailable => formatter.write_str(
                "could not determine the Reticulum config directory; pass --config explicitly",
            ),
        }
    }
}

impl std::error::Error for DiscoveryError {}

fn probe(dir: PathBuf, exists: &impl Fn(&Path) -> bool) -> DiscoveredConfig {
    let config = dir.join(reticulum_directory::CONFIG_FILE_NAME);
    DiscoveredConfig {
        config: exists(&config).then_some(config),
        dir,
    }
}

pub fn discover(override_dir: Option<&Path>) -> Result<DiscoveredConfig, DiscoveryError> {
    let dir = match override_dir {
        Some(dir) => dir.to_path_buf(),
        None => reticulum_directory::resolve().ok_or(DiscoveryError::HomeDirectoryUnavailable)?,
    };
    Ok(probe(dir, &|path: &Path| path.is_file()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn world(files: &[&str]) -> impl Fn(&Path) -> bool {
        let present: HashSet<PathBuf> = files.iter().map(PathBuf::from).collect();
        move |path: &Path| present.contains(path)
    }

    #[test]
    fn an_override_wins_outright_even_when_empty() {
        let dir = tempfile::tempdir().unwrap();
        let discovered = discover(Some(dir.path())).unwrap();
        assert_eq!(discovered.dir, dir.path());
        assert!(discovered.is_empty());
    }

    #[test]
    fn probe_ignores_the_retired_toml_filename() {
        let configs = probe(
            PathBuf::from("/home/op/.reticulum"),
            &world(&["/home/op/.reticulum/config.toml"]),
        );
        assert_eq!(configs.config, None);
        assert!(configs.is_empty());
    }

    #[test]
    fn probe_returns_only_the_extensionless_config() {
        let configs = probe(
            PathBuf::from("/etc/reticulum"),
            &world(&["/etc/reticulum/config", "/etc/reticulum/config.toml"]),
        );
        assert_eq!(configs.config, Some(PathBuf::from("/etc/reticulum/config")));
    }
}
