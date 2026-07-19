use std::path::{Path, PathBuf};
use std::process;

use personal_rns::config::{discover, parse_and_plan_named, ConfigDiagnostic, DaemonPlan};

pub(crate) const DEFAULT_CONFIG: &str = "[reticulum]\n\
    enable_transport = Yes\n\
    share_instance = Yes\n\
    [interfaces]\n\
      [[Default Interface]]\n\
        type = AutoInterface\n\
        interface_enabled = Yes\n";

pub(super) struct LoadedConfiguration {
    pub(super) directory: PathBuf,
    pub(super) path: Option<PathBuf>,
    pub(super) plan: DaemonPlan,
    pub(super) warnings: Vec<ConfigDiagnostic>,
}

pub(super) fn load_or_exit(config_dir: Option<&Path>) -> LoadedConfiguration {
    let discovered = match discover(config_dir) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("prnsd: config discovery failed: {error}");
            process::exit(1);
        }
    };
    let (text, source) = match &discovered.config {
        Some(path) => match std::fs::read_to_string(path) {
            Ok(text) => (text, path.display().to_string()),
            Err(error) => {
                eprintln!("prnsd: could not read config {}: {error}", path.display());
                process::exit(1);
            }
        },
        None => (DEFAULT_CONFIG.to_string(), "<built-in config>".to_string()),
    };
    let report = match parse_and_plan_named(&source, &text) {
        Ok(report) => report,
        Err(errors) => {
            for diagnostic in errors.diagnostics() {
                eprintln!("{diagnostic}");
            }
            process::exit(1);
        }
    };
    LoadedConfiguration {
        directory: discovered.dir,
        path: discovered.config,
        plan: report.value,
        warnings: report.warnings,
    }
}
