use std::path::{Path, PathBuf};
use std::process;

use personal_rns::config::{discover, parse_and_plan_named, ConfigDiagnostic, DaemonPlan};

use crate::cli::BootstrapProfile;

mod bootstrap;

pub(crate) use bootstrap::{
    format_archive_size, materialize_nnpages_settings, prepare_nnpages_layout, refresh_source_page,
    refresh_staged_bundled_source, seed_coming_from_rns_page, seed_default_page, seed_source_page,
    stage_bundled_source, stage_source_archive, BundledSourceRefresh, ManagedPageSeed,
    ServerBootstrapError, SourcePageRefresh, SourcePageSeed, SourcePageState,
};

pub(crate) const DEFAULT_CONFIG: &str = "[reticulum]\n\
    enable_transport = Yes\n\
    share_instance = Yes\n\
    [interfaces]\n\
      [[Default Interface]]\n\
        type = AutoInterface\n\
        interface_enabled = Yes\n\
      [[USB Auto]]\n\
        type = PrnsUsbAuto\n\
        interface_enabled = Yes\n\
      [[Bluetooth Auto]]\n\
        type = PrnsBluetoothAuto\n\
        interface_enabled = Yes\n";

pub(super) struct LoadedConfiguration {
    pub(super) directory: PathBuf,
    pub(super) path: Option<PathBuf>,
    pub(super) plan: DaemonPlan,
    pub(super) warnings: Vec<ConfigDiagnostic>,
}

pub(super) fn load_or_exit(
    config_dir: Option<&Path>,
    bootstrap_profile: Option<BootstrapProfile>,
) -> LoadedConfiguration {
    if let Some(BootstrapProfile::Server) = bootstrap_profile {
        match bootstrap::create_server_config_if_missing(config_dir) {
            Ok(Some(receipt)) => {
                eprintln!(
                    "prnsd: created cloud server configuration {}",
                    receipt.config_path.display()
                );
                if let Some(path) = receipt.seeded_page {
                    eprintln!("prnsd: seeded operator NNPages index {}", path.display());
                }
            }
            Ok(None) => {}
            Err(error) => {
                eprintln!("prnsd: server configuration bootstrap failed: {error}");
                process::exit(1);
            }
        }
    }
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
            eprintln!(
                "prnsd: run `prnsd interfaces repair --config {}` to inspect safe repairs",
                discovered.dir.display()
            );
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

#[cfg(test)]
mod tests {
    use super::*;
    use personal_rns::config::PlannedMedium;

    #[test]
    fn built_in_config_enables_each_auto_interface() {
        let plan = parse_and_plan_named("<built-in config>", DEFAULT_CONFIG)
            .expect("built-in configuration is valid")
            .value;

        assert_eq!(
            plan.interfaces
                .iter()
                .map(|interface| interface.name.as_str())
                .collect::<Vec<_>>(),
            ["Default Interface", "USB Auto", "Bluetooth Auto"]
        );
        assert!(matches!(
            plan.interfaces[0].medium,
            PlannedMedium::AutoWifi(_)
        ));
        assert!(matches!(
            plan.interfaces[1].medium,
            PlannedMedium::PrnsUsbAuto
        ));
        assert!(matches!(
            plan.interfaces[2].medium,
            PlannedMedium::PrnsBluetoothAuto { .. }
        ));
    }
}
