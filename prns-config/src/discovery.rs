use std::fmt;
use std::path::{Path, PathBuf};

const REFERENCE_FILE: &str = "config";
const TOML_FILE: &str = "config.toml";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredConfigs {
    pub dir: PathBuf,
    pub reference: Option<PathBuf>,
    pub toml: Option<PathBuf>,
}

impl DiscoveredConfigs {
    pub fn is_empty(&self) -> bool {
        self.reference.is_none() && self.toml.is_none()
    }

    pub fn has_both(&self) -> bool {
        self.reference.is_some() && self.toml.is_some()
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
    exists(&dir.join(REFERENCE_FILE)) || exists(&dir.join(TOML_FILE))
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

fn probe(dir: PathBuf, exists: &impl Fn(&Path) -> bool) -> DiscoveredConfigs {
    let reference = dir.join(REFERENCE_FILE);
    let toml = dir.join(TOML_FILE);
    DiscoveredConfigs {
        reference: exists(&reference).then_some(reference),
        toml: exists(&toml).then_some(toml),
        dir,
    }
}

pub fn discover(override_dir: Option<&Path>) -> Result<DiscoveredConfigs, DiscoveryError> {
    let exists = |path: &Path| path.is_file();
    let dir = resolve_dir(override_dir, system_config_dir(), dirs::home_dir(), &exists)?;
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
    fn a_lone_toml_makes_a_dir_count_as_a_config_home() {
        let dir = resolve_dir(
            None,
            Some(PathBuf::from("/etc/reticulum")),
            Some(PathBuf::from("/home/op")),
            &world(&["/home/op/.config/reticulum/config.toml"]),
        )
        .unwrap();
        assert_eq!(dir, PathBuf::from("/home/op/.config/reticulum"));
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
    fn probe_reports_exactly_which_files_exist() {
        let configs = probe(
            PathBuf::from("/home/op/.reticulum"),
            &world(&["/home/op/.reticulum/config.toml"]),
        );
        assert_eq!(configs.reference, None);
        assert_eq!(
            configs.toml,
            Some(PathBuf::from("/home/op/.reticulum/config.toml"))
        );
        assert!(!configs.is_empty());
        assert!(!configs.has_both());
    }

    #[test]
    fn both_files_present_is_the_divergeable_case() {
        let configs = probe(
            PathBuf::from("/etc/reticulum"),
            &world(&["/etc/reticulum/config", "/etc/reticulum/config.toml"]),
        );
        assert!(configs.has_both());
    }
}
