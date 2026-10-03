use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::BuildError;

pub(super) fn validate(
    repository: &Path,
    cargo: &Command,
    adapter_target: &str,
) -> Result<(), BuildError> {
    validate_inherited_variables(std::env::vars_os().map(|(name, _)| name), adapter_target)?;
    let current_dir = cargo
        .get_current_dir()
        .ok_or(BuildError::MissingCargoWorkingDirectory)?;
    validate_cargo_configurations(repository, current_dir, cargo_home(current_dir).as_deref())
}

fn validate_inherited_variables(
    names: impl IntoIterator<Item = OsString>,
    adapter_target: &str,
) -> Result<(), BuildError> {
    if let Some(variable) = names
        .into_iter()
        .filter(|name| semantic_override(name, adapter_target))
        .min()
    {
        Err(BuildError::SemanticEnvironmentOverride { variable })
    } else {
        Ok(())
    }
}

fn semantic_override(name: &OsStr, adapter_target: &str) -> bool {
    let name = name.to_string_lossy();
    let adapter_target = adapter_target.replace('-', "_").to_ascii_uppercase();
    let adapter_rustflags = format!("CARGO_TARGET_{adapter_target}_RUSTFLAGS");
    let adapter_linker = format!("CARGO_TARGET_{adapter_target}_LINKER");
    let unowned_target_setting = name.starts_with("CARGO_TARGET_")
        && (name.ends_with("_RUSTFLAGS") || name.ends_with("_LINKER"))
        && name != adapter_rustflags
        && name != adapter_linker;
    name.starts_with("CARGO_PROFILE_")
        || name.starts_with("CARGO_UNSTABLE_")
        || name.starts_with("ESP_")
        || name.starts_with("HOPSPOT_")
        || name.starts_with("PRNS_BUILD_")
        || name.starts_with("PRNS_SOURCE_")
        || name.starts_with("TROUBLE_HOST_")
        || unowned_target_setting
        || matches!(
            name.as_ref(),
            "CARGO_ENCODED_RUSTFLAGS"
                | "CARGO_BUILD_INCREMENTAL"
                | "CARGO_BUILD_RUSTFLAGS"
                | "CARGO_BUILD_RUSTC"
                | "CARGO_BUILD_RUSTC_WRAPPER"
                | "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER"
                | "CARGO_BUILD_TARGET"
                | "CARGO_CONFIG"
                | "CARGO_INCREMENTAL"
                | "PRNS_FLASH_VERSION"
                | "RUSTC"
                | "RUSTC_WRAPPER"
                | "RUSTC_WORKSPACE_WRAPPER"
                | "SOURCE_DATE_EPOCH"
        )
}

fn validate_cargo_configurations(
    repository: &Path,
    current_dir: &Path,
    cargo_home: Option<&Path>,
) -> Result<(), BuildError> {
    let repository = canonicalize(repository)?;
    let mut configurations = current_dir
        .ancestors()
        .filter_map(|directory| active_cargo_configuration(&directory.join(".cargo")))
        .collect::<Vec<_>>();
    if let Some(cargo_home) = cargo_home.and_then(active_cargo_configuration) {
        configurations.push(cargo_home);
    }
    configurations.sort();
    configurations.dedup();
    for path in configurations {
        let canonical_path = canonicalize(&path)?;
        if !canonical_path.starts_with(&repository) {
            validate_external_cargo_configuration(&canonical_path)?;
        }
    }
    Ok(())
}

fn cargo_home(current_dir: &Path) -> Option<PathBuf> {
    let path = std::env::var_os("CARGO_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .or_else(|| std::env::var_os("USERPROFILE"))
                .map(PathBuf::from)
                .map(|home| home.join(".cargo"))
        })?;
    Some(if path.is_absolute() {
        path
    } else {
        current_dir.join(path)
    })
}

fn active_cargo_configuration(directory: &Path) -> Option<PathBuf> {
    let legacy = directory.join("config");
    if legacy.is_file() {
        return Some(legacy);
    }
    let current = directory.join("config.toml");
    current.is_file().then_some(current)
}

fn validate_external_cargo_configuration(path: &Path) -> Result<(), BuildError> {
    let contents =
        std::fs::read_to_string(path).map_err(|source| BuildError::CargoConfigurationIo {
            path: path.to_path_buf(),
            source,
        })?;
    let configuration = toml::from_str::<toml::Table>(&contents).map_err(|error| {
        BuildError::CargoConfigurationParse {
            path: path.to_path_buf(),
            reason: error.to_string(),
        }
    })?;
    if let Some(key) = configuration_requires_custody(&configuration) {
        return Err(BuildError::ExternalCargoConfiguration {
            path: path.to_path_buf(),
            key,
        });
    }
    Ok(())
}

fn configuration_requires_custody(configuration: &toml::Table) -> Option<String> {
    configuration
        .iter()
        .filter_map(|(key, value)| match key.as_str() {
            "build" => restricted_table_key(
                key,
                value,
                &[
                    "build-dir",
                    "dep-info-basedir",
                    "jobs",
                    "pipelining",
                    "rustdocflags",
                    "target-dir",
                ],
            ),
            "env" => restricted_table_key(key, value, &["RUST_MIN_STACK"]),
            "target" => restricted_target_key(value),
            "alias"
            | "cache"
            | "cargo-new"
            | "credential-alias"
            | "doc"
            | "future-incompat-report"
            | "http"
            | "install"
            | "net"
            | "registries"
            | "registry"
            | "source"
            | "term" => None,
            _ => Some(key.clone()),
        })
        .min()
}

fn restricted_table_key(prefix: &str, value: &toml::Value, allowed: &[&str]) -> Option<String> {
    let Some(table) = value.as_table() else {
        return Some(prefix.to_string());
    };
    table
        .keys()
        .filter(|key| !allowed.contains(&key.as_str()))
        .map(|key| format!("{prefix}.{key}"))
        .min()
}

fn restricted_target_key(value: &toml::Value) -> Option<String> {
    let Some(targets) = value.as_table() else {
        return Some("target".to_string());
    };
    targets
        .iter()
        .filter_map(|(target, value)| {
            let Some(settings) = value.as_table() else {
                return Some(format!("target.{target}"));
            };
            settings
                .keys()
                .filter(|key| !matches!(key.as_str(), "runner" | "rustdocflags"))
                .map(|key| format!("target.{target}.{key}"))
                .min()
        })
        .min()
}

fn canonicalize(path: &Path) -> Result<PathBuf, BuildError> {
    std::fs::canonicalize(path).map_err(|source| BuildError::CargoConfigurationIo {
        path: path.to_path_buf(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inherited_build_inputs_are_rejected() {
        for variable in [
            "CARGO_PROFILE_RELEASE_LTO",
            "CARGO_PROFILE_RELEASE_PACKAGE_EXAMPLE_OPT_LEVEL",
            "CARGO_UNSTABLE_BUILD_STD_FEATURES",
            "CARGO_ENCODED_RUSTFLAGS",
            "CARGO_BUILD_INCREMENTAL",
            "CARGO_BUILD_RUSTFLAGS",
            "CARGO_BUILD_RUSTC",
            "CARGO_BUILD_RUSTC_WRAPPER",
            "CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER",
            "CARGO_BUILD_TARGET",
            "CARGO_CONFIG",
            "CARGO_INCREMENTAL",
            "CARGO_TARGET_AARCH64_APPLE_DARWIN_RUSTFLAGS",
            "ESP_HAL_CONFIG_STACK_GUARD_OFFSET",
            "ESP_LOG",
            "HOPSPOT_TCP_TARGET",
            "HOPSPOT_WIFI_PASSWORD",
            "HOPSPOT_WIFI_SECURITY_PROBE_MODE",
            "HOPSPOT_WIFI_SECURITY_STATION_PROBE",
            "HOPSPOT_WIFI_SSID",
            "PRNS_BUILD_COMMIT",
            "PRNS_BUILD_COMMIT_SHORT",
            "PRNS_BUILD_CHANNEL",
            "PRNS_BUILD_SOURCE_DIGEST",
            "PRNS_FLASH_VERSION",
            "PRNS_SOURCE_ARCHIVE",
            "PRNS_SOURCE_COMMIT",
            "PRNS_SOURCE_SHA256",
            "PRNS_SOURCE_SIZE",
            "PRNS_SOURCE_VERSION",
            "RUSTC",
            "RUSTC_WRAPPER",
            "RUSTC_WORKSPACE_WRAPPER",
            "SOURCE_DATE_EPOCH",
            "TROUBLE_HOST_GATT_CLIENT_NOTIFICATION_MAX_SUBSCRIBERS",
            "TROUBLE_HOST_GATT_CLIENT_NOTIFICATION_QUEUE_SIZE",
        ] {
            assert!(matches!(
                validate_inherited_variables(
                    [OsString::from(variable)],
                    "thumbv7em-none-eabihf"
                ),
                Err(BuildError::SemanticEnvironmentOverride { variable: actual })
                    if actual == OsStr::new(variable)
            ));
        }
    }

    #[test]
    fn adapter_owned_and_output_independent_environment_is_allowed() -> Result<(), BuildError> {
        validate_inherited_variables(
            [
                "RUSTFLAGS",
                "CARGO_TARGET_THUMBV7EM_NONE_EABIHF_RUSTFLAGS",
                "CARGO_TARGET_THUMBV7EM_NONE_EABIHF_LINKER",
                "CARGO_HOME",
                "PATH",
            ]
            .map(OsString::from),
            "thumbv7em-none-eabihf",
        )
    }

    #[test]
    fn multiple_overrides_report_the_lexically_first_variable() {
        assert!(matches!(
            validate_inherited_variables(
                ["RUSTC", "CARGO_INCREMENTAL", "PRNS_BUILD_COMMIT"].map(OsString::from),
                "thumbv7em-none-eabihf",
            ),
            Err(BuildError::SemanticEnvironmentOverride { variable })
                if variable == OsStr::new("CARGO_INCREMENTAL")
        ));
    }

    #[test]
    fn external_output_configuration_is_rejected() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("config.toml");
        std::fs::write(&path, "[profile.release]\nlto = \"thin\"\n")?;
        assert!(matches!(
            validate_external_cargo_configuration(&path),
            Err(BuildError::ExternalCargoConfiguration { path: actual, key })
                if actual == path && key == "profile"
        ));
        Ok(())
    }

    #[test]
    fn external_output_independent_configuration_is_allowed(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("config.toml");
        std::fs::write(
            &path,
            "[alias]\ncheck-all = \"check --all\"\n[net]\noffline = true\n[build]\njobs = 4\n[env]\nRUST_MIN_STACK = \"8388608\"\n[target.thumbv7em-none-eabihf]\nrunner = \"probe-rs run\"\n",
        )?;
        validate_external_cargo_configuration(&path)?;
        Ok(())
    }

    #[test]
    fn external_target_rustflags_are_rejected() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("config.toml");
        std::fs::write(
            &path,
            "[target.thumbv7em-none-eabihf]\nrustflags = [\"-C\", \"opt-level=0\"]\n",
        )?;
        assert!(matches!(
            validate_external_cargo_configuration(&path),
            Err(BuildError::ExternalCargoConfiguration { key, .. })
                if key == "target.thumbv7em-none-eabihf.rustflags"
        ));
        Ok(())
    }

    #[test]
    fn extensionless_configuration_has_cargo_compatibility_precedence(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let legacy = directory.path().join("config");
        std::fs::write(&legacy, "[alias]\nlegacy = \"check\"\n")?;
        std::fs::write(
            directory.path().join("config.toml"),
            "[profile.release]\nlto = \"thin\"\n",
        )?;
        assert_eq!(active_cargo_configuration(directory.path()), Some(legacy));
        Ok(())
    }

    #[test]
    fn repository_owned_configuration_remains_source_custodied(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let repository = directory.path().join("repository");
        let current_dir = repository.join("firmware");
        let cargo = repository.join(".cargo");
        std::fs::create_dir_all(&current_dir)?;
        std::fs::create_dir_all(&cargo)?;
        std::fs::write(
            cargo.join("config.toml"),
            "[profile.release]\nlto = \"thin\"\n",
        )?;
        validate_cargo_configurations(&repository, &current_dir, None)?;
        Ok(())
    }

    #[test]
    fn semantic_ancestor_configuration_is_rejected() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let parent = directory.path().join("parent");
        let repository = parent.join("repository");
        let current_dir = repository.join("firmware");
        let cargo = parent.join(".cargo");
        std::fs::create_dir_all(&current_dir)?;
        std::fs::create_dir_all(&cargo)?;
        std::fs::write(cargo.join("config.toml"), "[build]\ntarget = \"host\"\n")?;
        assert!(matches!(
            validate_cargo_configurations(&repository, &current_dir, None),
            Err(BuildError::ExternalCargoConfiguration { key, .. }) if key == "build.target"
        ));
        Ok(())
    }

    #[test]
    fn semantic_cargo_home_configuration_is_rejected() -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let repository = directory.path().join("repository");
        let current_dir = repository.join("firmware");
        let cargo_home = directory.path().join("cargo-home");
        std::fs::create_dir_all(&current_dir)?;
        std::fs::create_dir_all(&cargo_home)?;
        std::fs::write(cargo_home.join("config.toml"), "[env]\nMODE = \"other\"\n")?;
        assert!(matches!(
            validate_cargo_configurations(&repository, &current_dir, Some(&cargo_home)),
            Err(BuildError::ExternalCargoConfiguration { key, .. }) if key == "env.MODE"
        ));
        Ok(())
    }

    #[test]
    fn malformed_external_configuration_has_a_typed_error() -> Result<(), Box<dyn std::error::Error>>
    {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("config.toml");
        std::fs::write(&path, "[profile.release\n")?;
        assert!(matches!(
            validate_external_cargo_configuration(&path),
            Err(BuildError::CargoConfigurationParse { path: actual, .. }) if actual == path
        ));
        Ok(())
    }
}
