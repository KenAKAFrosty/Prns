use std::fmt;
use std::path::{Path, PathBuf};

const REFERENCE_FILE: &str = "config";

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

fn system_config_dir() -> Option<PathBuf> {
    #[cfg(unix)]
    {
        Some(PathBuf::from("/etc/reticulum"))
    }
    #[cfg(not(unix))]
    {
        None
    }
}

fn holds_config(dir: &Path, exists: &impl Fn(&Path) -> bool) -> bool {
    exists(&dir.join(REFERENCE_FILE))
}

fn resolve_dir(
    override_dir: Option<&Path>,
    system: Option<PathBuf>,
    home: Option<PathBuf>,
    exists: &impl Fn(&Path) -> bool,
) -> Result<PathBuf, DiscoveryError> {
    if let Some(dir) = override_dir {
        return Ok(dir.to_path_buf());
    }
    if let Some(system) = system {
        if holds_config(&system, exists) {
            return Ok(system);
        }
    }
    let home = home.ok_or(DiscoveryError::HomeDirectoryUnavailable)?;
    let xdg = home.join(".config/reticulum");
    if holds_config(&xdg, exists) {
        return Ok(xdg);
    }
    Ok(home.join(".reticulum"))
}

fn probe(dir: PathBuf, exists: &impl Fn(&Path) -> bool) -> DiscoveredConfig {
    let config = dir.join(REFERENCE_FILE);
    DiscoveredConfig {
        config: exists(&config).then_some(config),
        dir,
    }
}

pub fn discover(override_dir: Option<&Path>) -> Result<DiscoveredConfig, DiscoveryError> {
    let exists = |path: &Path| path.is_file();
    let dir = resolve_dir(override_dir, system_config_dir(), home::home_dir(), &exists)?;
    Ok(probe(dir, &exists))
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
        let dir = resolve_dir(
            Some(Path::new("/opt/custom")),
            Some(PathBuf::from("/etc/reticulum")),
            Some(PathBuf::from("/home/op")),
            &world(&["/etc/reticulum/config"]),
        )
        .unwrap();
        assert_eq!(dir, PathBuf::from("/opt/custom"));
    }

    #[test]
    fn etc_outranks_home_when_it_holds_config() {
        let dir = resolve_dir(
            None,
            Some(PathBuf::from("/etc/reticulum")),
            Some(PathBuf::from("/home/op")),
            &world(&["/etc/reticulum/config"]),
        )
        .unwrap();
        assert_eq!(dir, PathBuf::from("/etc/reticulum"));
    }

    #[test]
    fn a_lone_toml_file_is_not_a_config_home() {
        let dir = resolve_dir(
            None,
            Some(PathBuf::from("/etc/reticulum")),
            Some(PathBuf::from("/home/op")),
            &world(&["/home/op/.config/reticulum/config.toml"]),
        )
        .unwrap();
        assert_eq!(dir, PathBuf::from("/home/op/.reticulum"));
    }

    #[test]
    fn the_default_home_dir_is_returned_even_with_nothing_in_it() {
        let dir = resolve_dir(
            None,
            Some(PathBuf::from("/etc/reticulum")),
            Some(PathBuf::from("/home/op")),
            &world(&[]),
        )
        .unwrap();
        assert_eq!(dir, PathBuf::from("/home/op/.reticulum"));
    }

    #[test]
    fn a_missing_home_is_an_error_without_an_override() {
        assert!(matches!(
            resolve_dir(None, None, None, &world(&[])),
            Err(DiscoveryError::HomeDirectoryUnavailable)
        ));
    }

    #[test]
    fn non_unix_discovery_does_not_invent_an_etc_directory() {
        let dir = resolve_dir(
            None,
            None,
            Some(PathBuf::from("C:/Users/op")),
            &world(&["/etc/reticulum/config"]),
        )
        .unwrap();
        assert_eq!(dir, PathBuf::from("C:/Users/op/.reticulum"));
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
