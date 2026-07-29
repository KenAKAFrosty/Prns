use std::path::{Path, PathBuf};

pub const CONFIG_FILE_NAME: &str = "config";

fn system_directory() -> Option<PathBuf> {
    #[cfg(unix)]
    {
        Some(PathBuf::from("/etc/reticulum"))
    }
    #[cfg(not(unix))]
    {
        None
    }
}

fn holds_config(directory: &Path, exists: &impl Fn(&Path) -> bool) -> bool {
    exists(&directory.join(CONFIG_FILE_NAME))
}

fn resolve_from(
    system: Option<PathBuf>,
    home: Option<PathBuf>,
    exists: &impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    if let Some(system) = system {
        if holds_config(&system, exists) {
            return Some(system);
        }
    }
    let home = home?;
    let xdg = home.join(".config/reticulum");
    if holds_config(&xdg, exists) {
        return Some(xdg);
    }
    Some(home.join(".reticulum"))
}

pub fn resolve() -> Option<PathBuf> {
    resolve_from(system_directory(), std::env::home_dir(), &|path: &Path| {
        path.is_file()
    })
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
    fn etc_outranks_home_when_it_holds_config() {
        let dir = resolve_from(
            Some(PathBuf::from("/etc/reticulum")),
            Some(PathBuf::from("/home/op")),
            &world(&["/etc/reticulum/config"]),
        )
        .unwrap();
        assert_eq!(dir, PathBuf::from("/etc/reticulum"));
    }

    #[test]
    fn a_lone_toml_file_is_not_a_config_home() {
        let dir = resolve_from(
            Some(PathBuf::from("/etc/reticulum")),
            Some(PathBuf::from("/home/op")),
            &world(&["/home/op/.config/reticulum/config.toml"]),
        )
        .unwrap();
        assert_eq!(dir, PathBuf::from("/home/op/.reticulum"));
    }

    #[test]
    fn the_default_home_dir_is_returned_even_with_nothing_in_it() {
        let dir = resolve_from(
            Some(PathBuf::from("/etc/reticulum")),
            Some(PathBuf::from("/home/op")),
            &world(&[]),
        )
        .unwrap();
        assert_eq!(dir, PathBuf::from("/home/op/.reticulum"));
    }

    #[test]
    fn a_missing_home_resolves_to_none() {
        assert_eq!(resolve_from(None, None, &world(&[])), None);
    }

    #[test]
    fn non_unix_resolution_does_not_invent_an_etc_directory() {
        let dir = resolve_from(
            None,
            Some(PathBuf::from("C:/Users/op")),
            &world(&["/etc/reticulum/config"]),
        )
        .unwrap();
        assert_eq!(dir, PathBuf::from("C:/Users/op/.reticulum"));
    }
}
