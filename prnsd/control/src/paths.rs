use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub struct StateDirectoryError;

impl fmt::Display for StateDirectoryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .write_str("could not determine a per-user prnsd state directory; set PRNSD_STATE_DIR")
    }
}

impl std::error::Error for StateDirectoryError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServicePaths {
    pub state_dir: PathBuf,
    pub record: PathBuf,
    pub control_lock: PathBuf,
    pub runtime_lock: PathBuf,
    pub ready: PathBuf,
    pub stop: PathBuf,
    pub human_log: PathBuf,
    pub human_previous_log: PathBuf,
    pub json_log: PathBuf,
    pub json_previous_log: PathBuf,
}

impl ServicePaths {
    pub fn discover() -> Result<Self, StateDirectoryError> {
        resolve_state_dir(
            std::env::var_os("PRNSD_STATE_DIR"),
            dirs::state_dir().or_else(dirs::data_local_dir),
        )
        .map(Self::in_dir)
    }

    pub fn in_dir(path: impl AsRef<Path>) -> Self {
        let state_dir = path.as_ref().to_path_buf();
        Self {
            record: state_dir.join("session"),
            control_lock: state_dir.join("control.lock"),
            runtime_lock: state_dir.join("runtime.lock"),
            ready: state_dir.join("ready"),
            stop: state_dir.join("stop"),
            human_log: state_dir.join("prnsd.log"),
            human_previous_log: state_dir.join("prnsd.previous.log"),
            json_log: state_dir.join("prnsd.jsonl"),
            json_previous_log: state_dir.join("prnsd.previous.jsonl"),
            state_dir,
        }
    }

    pub fn reload_request(&self, generation: u128) -> PathBuf {
        self.state_dir
            .join(format!("reload-request-{generation:032x}"))
    }

    pub fn reload_result(&self, generation: u128, request_id: u128) -> PathBuf {
        self.state_dir.join(format!(
            "reload-result-{generation:032x}-{request_id:032x}"
        ))
    }
}

fn resolve_state_dir(
    override_dir: Option<OsString>,
    platform_base: Option<PathBuf>,
) -> Result<PathBuf, StateDirectoryError> {
    match override_dir {
        Some(path) if !path.is_empty() => Ok(PathBuf::from(path)),
        _ => platform_base
            .map(|base| base.join("prnsd"))
            .ok_or(StateDirectoryError),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_share_one_state_directory() {
        let paths = ServicePaths::in_dir("/state/prnsd");
        assert_eq!(paths.state_dir, Path::new("/state/prnsd"));
        assert_eq!(paths.record, Path::new("/state/prnsd/session"));
        assert_eq!(paths.human_log, Path::new("/state/prnsd/prnsd.log"));
        assert_eq!(paths.json_log, Path::new("/state/prnsd/prnsd.jsonl"));
    }

    #[test]
    fn override_directory_wins_over_the_platform_base() {
        assert_eq!(
            resolve_state_dir(
                Some(OsString::from("/isolated")),
                Some(PathBuf::from("/platform"))
            )
            .unwrap(),
            Path::new("/isolated")
        );
    }

    #[test]
    fn platform_base_is_used_when_there_is_no_override() {
        assert_eq!(
            resolve_state_dir(None, Some(PathBuf::from("/platform"))).unwrap(),
            Path::new("/platform/prnsd")
        );
        assert!(resolve_state_dir(None, None).is_err());
    }
}
