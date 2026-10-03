use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use personal_hopspot_builder::LtoMode;
use serde::Deserialize;
use thiserror::Error;

use super::model::{
    CargoLtoIdentity, DebugInfoIdentity, OptimizationLevelIdentity, PackageReleaseOverrideIdentity,
    PackageReleaseSettingsIdentity, PanicStrategyIdentity, ReleaseSettingsIdentity,
    SplitDebuginfoIdentity, StripIdentity,
};

pub(super) struct ResolvedBuildSettings {
    pub(super) effective_release: ReleaseSettingsIdentity,
    pub(super) package_overrides: Vec<PackageReleaseOverrideIdentity>,
    pub(super) build_override: PackageReleaseSettingsIdentity,
}

#[derive(Debug, Error)]
pub(crate) enum BuildSettingsError {
    #[error("could not read firmware manifest {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("could not parse firmware manifest {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    #[error("firmware manifest {path} has invalid {setting} value {value:?}")]
    InvalidSetting {
        path: PathBuf,
        setting: String,
        value: String,
    },
}

pub(super) fn resolve(
    repository: &Path,
    manifest: &str,
    lto_selection: LtoMode,
) -> Result<ResolvedBuildSettings, BuildSettingsError> {
    let path = repository.join(manifest);
    let source = std::fs::read_to_string(&path).map_err(|source| BuildSettingsError::Read {
        path: path.clone(),
        source,
    })?;
    let manifest =
        toml::from_str::<Manifest>(&source).map_err(|source| BuildSettingsError::Parse {
            path: path.clone(),
            source,
        })?;
    normalize(
        &path,
        manifest.profile.release.unwrap_or_default(),
        lto_selection,
    )
}

fn normalize(
    path: &Path,
    profile: RawReleaseProfile,
    lto_selection: LtoMode,
) -> Result<ResolvedBuildSettings, BuildSettingsError> {
    let mut effective_release = ReleaseSettingsIdentity {
        opt_level: optimization_level(
            path,
            "profile.release.opt-level",
            profile.opt_level,
            OptimizationLevelIdentity::Three,
        )?,
        debug: debug_info(
            path,
            "profile.release.debug",
            profile.debug,
            DebugInfoIdentity::None,
        )?,
        split_debuginfo: split_debuginfo(
            path,
            "profile.release.split-debuginfo",
            profile.split_debuginfo,
            SplitDebuginfoIdentity::ToolchainDefault,
        )?,
        strip: strip(
            path,
            "profile.release.strip",
            profile.strip,
            StripIdentity::None,
        )?,
        debug_assertions: profile.debug_assertions.unwrap_or(false),
        overflow_checks: profile.overflow_checks.unwrap_or(false),
        lto: lto(
            path,
            "profile.release.lto",
            profile.lto,
            CargoLtoIdentity::False,
        )?,
        panic: panic_strategy(
            path,
            "profile.release.panic",
            profile.panic,
            PanicStrategyIdentity::Unwind,
        )?,
        incremental: profile.incremental.unwrap_or(false),
        codegen_units: codegen_units(
            path,
            "profile.release.codegen-units",
            profile.codegen_units,
            16,
        )?,
        rpath: profile.rpath.unwrap_or(false),
    };
    effective_release.lto = match lto_selection {
        LtoMode::Configured => effective_release.lto,
        LtoMode::Fat => CargoLtoIdentity::Fat,
        LtoMode::Thin => CargoLtoIdentity::Thin,
    };

    let package_base = package_settings(&effective_release);
    let package_overrides = profile
        .package
        .into_iter()
        .map(|(package, raw)| {
            let setting = format!("profile.release.package.{package}");
            Ok(PackageReleaseOverrideIdentity {
                package,
                settings: merge_package_settings(path, &setting, &package_base, raw)?,
            })
        })
        .collect::<Result<Vec<_>, BuildSettingsError>>()?;
    let build_base = PackageReleaseSettingsIdentity {
        opt_level: OptimizationLevelIdentity::Zero,
        debug: DebugInfoIdentity::None,
        codegen_units: 256,
        ..package_base
    };
    let build_override = merge_package_settings(
        path,
        "profile.release.build-override",
        &build_base,
        profile.build_override,
    )?;
    Ok(ResolvedBuildSettings {
        effective_release,
        package_overrides,
        build_override,
    })
}

fn package_settings(release: &ReleaseSettingsIdentity) -> PackageReleaseSettingsIdentity {
    PackageReleaseSettingsIdentity {
        opt_level: release.opt_level,
        debug: release.debug,
        split_debuginfo: release.split_debuginfo,
        strip: release.strip,
        debug_assertions: release.debug_assertions,
        overflow_checks: release.overflow_checks,
        incremental: release.incremental,
        codegen_units: release.codegen_units,
    }
}

fn merge_package_settings(
    path: &Path,
    setting: &str,
    base: &PackageReleaseSettingsIdentity,
    raw: RawPackageProfile,
) -> Result<PackageReleaseSettingsIdentity, BuildSettingsError> {
    Ok(PackageReleaseSettingsIdentity {
        opt_level: optimization_level(
            path,
            &format!("{setting}.opt-level"),
            raw.opt_level,
            base.opt_level,
        )?,
        debug: debug_info(path, &format!("{setting}.debug"), raw.debug, base.debug)?,
        split_debuginfo: split_debuginfo(
            path,
            &format!("{setting}.split-debuginfo"),
            raw.split_debuginfo,
            base.split_debuginfo,
        )?,
        strip: strip(path, &format!("{setting}.strip"), raw.strip, base.strip)?,
        debug_assertions: raw.debug_assertions.unwrap_or(base.debug_assertions),
        overflow_checks: raw.overflow_checks.unwrap_or(base.overflow_checks),
        incremental: raw.incremental.unwrap_or(base.incremental),
        codegen_units: codegen_units(
            path,
            &format!("{setting}.codegen-units"),
            raw.codegen_units,
            base.codegen_units,
        )?,
    })
}

fn optimization_level(
    path: &Path,
    setting: &str,
    value: Option<RawOptimizationLevel>,
    default: OptimizationLevelIdentity,
) -> Result<OptimizationLevelIdentity, BuildSettingsError> {
    let Some(value) = value else {
        return Ok(default);
    };
    match value {
        RawOptimizationLevel::Integer(0) => Ok(OptimizationLevelIdentity::Zero),
        RawOptimizationLevel::Integer(1) => Ok(OptimizationLevelIdentity::One),
        RawOptimizationLevel::Integer(2) => Ok(OptimizationLevelIdentity::Two),
        RawOptimizationLevel::Integer(3) => Ok(OptimizationLevelIdentity::Three),
        RawOptimizationLevel::String(value) if value == "s" => Ok(OptimizationLevelIdentity::Size),
        RawOptimizationLevel::String(value) if value == "z" => {
            Ok(OptimizationLevelIdentity::SizeMin)
        }
        value => invalid(path, setting, format!("{value:?}")),
    }
}

fn debug_info(
    path: &Path,
    setting: &str,
    value: Option<RawDebugInfo>,
    default: DebugInfoIdentity,
) -> Result<DebugInfoIdentity, BuildSettingsError> {
    let Some(value) = value else {
        return Ok(default);
    };
    match value {
        RawDebugInfo::Boolean(false) | RawDebugInfo::Integer(0) => Ok(DebugInfoIdentity::None),
        RawDebugInfo::Integer(1) => Ok(DebugInfoIdentity::Limited),
        RawDebugInfo::Boolean(true) | RawDebugInfo::Integer(2) => Ok(DebugInfoIdentity::Full),
        RawDebugInfo::String(value) if value == "none" => Ok(DebugInfoIdentity::None),
        RawDebugInfo::String(value) if value == "limited" => Ok(DebugInfoIdentity::Limited),
        RawDebugInfo::String(value) if value == "full" => Ok(DebugInfoIdentity::Full),
        RawDebugInfo::String(value) if value == "line-tables-only" => {
            Ok(DebugInfoIdentity::LineTablesOnly)
        }
        value => invalid(path, setting, format!("{value:?}")),
    }
}

fn split_debuginfo(
    path: &Path,
    setting: &str,
    value: Option<String>,
    default: SplitDebuginfoIdentity,
) -> Result<SplitDebuginfoIdentity, BuildSettingsError> {
    match value.as_deref() {
        None => Ok(default),
        Some("off") => Ok(SplitDebuginfoIdentity::Off),
        Some("packed") => Ok(SplitDebuginfoIdentity::Packed),
        Some("unpacked") => Ok(SplitDebuginfoIdentity::Unpacked),
        Some(value) => invalid(path, setting, value.to_string()),
    }
}

fn strip(
    path: &Path,
    setting: &str,
    value: Option<RawStrip>,
    default: StripIdentity,
) -> Result<StripIdentity, BuildSettingsError> {
    match value {
        None => Ok(default),
        Some(RawStrip::Boolean(false)) => Ok(StripIdentity::None),
        Some(RawStrip::Boolean(true)) => Ok(StripIdentity::Symbols),
        Some(RawStrip::String(value)) if value == "none" => Ok(StripIdentity::None),
        Some(RawStrip::String(value)) if value == "debuginfo" => Ok(StripIdentity::Debuginfo),
        Some(RawStrip::String(value)) if value == "symbols" => Ok(StripIdentity::Symbols),
        Some(value) => invalid(path, setting, format!("{value:?}")),
    }
}

fn lto(
    path: &Path,
    setting: &str,
    value: Option<RawLto>,
    default: CargoLtoIdentity,
) -> Result<CargoLtoIdentity, BuildSettingsError> {
    match value {
        None => Ok(default),
        Some(RawLto::Boolean(false)) => Ok(CargoLtoIdentity::False),
        Some(RawLto::Boolean(true)) => Ok(CargoLtoIdentity::Fat),
        Some(RawLto::String(value)) if value == "fat" => Ok(CargoLtoIdentity::Fat),
        Some(RawLto::String(value)) if value == "thin" => Ok(CargoLtoIdentity::Thin),
        Some(RawLto::String(value)) if value == "off" => Ok(CargoLtoIdentity::Off),
        Some(value) => invalid(path, setting, format!("{value:?}")),
    }
}

fn panic_strategy(
    path: &Path,
    setting: &str,
    value: Option<String>,
    default: PanicStrategyIdentity,
) -> Result<PanicStrategyIdentity, BuildSettingsError> {
    match value.as_deref() {
        None => Ok(default),
        Some("unwind") => Ok(PanicStrategyIdentity::Unwind),
        Some("abort") => Ok(PanicStrategyIdentity::Abort),
        Some(value) => invalid(path, setting, value.to_string()),
    }
}

fn codegen_units(
    path: &Path,
    setting: &str,
    value: Option<i64>,
    default: u32,
) -> Result<u32, BuildSettingsError> {
    match value {
        None => Ok(default),
        Some(value) => u32::try_from(value)
            .ok()
            .filter(|value| *value != 0)
            .ok_or_else(|| BuildSettingsError::InvalidSetting {
                path: path.to_path_buf(),
                setting: setting.to_string(),
                value: value.to_string(),
            }),
    }
}

fn invalid<T>(path: &Path, setting: &str, value: String) -> Result<T, BuildSettingsError> {
    Err(BuildSettingsError::InvalidSetting {
        path: path.to_path_buf(),
        setting: setting.to_string(),
        value,
    })
}

#[derive(Deserialize, Default)]
struct Manifest {
    #[serde(default)]
    profile: Profiles,
}

#[derive(Deserialize, Default)]
struct Profiles {
    release: Option<RawReleaseProfile>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct RawReleaseProfile {
    opt_level: Option<RawOptimizationLevel>,
    debug: Option<RawDebugInfo>,
    split_debuginfo: Option<String>,
    strip: Option<RawStrip>,
    debug_assertions: Option<bool>,
    overflow_checks: Option<bool>,
    lto: Option<RawLto>,
    panic: Option<String>,
    incremental: Option<bool>,
    codegen_units: Option<i64>,
    rpath: Option<bool>,
    #[serde(default)]
    package: BTreeMap<String, RawPackageProfile>,
    #[serde(default)]
    build_override: RawPackageProfile,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
struct RawPackageProfile {
    opt_level: Option<RawOptimizationLevel>,
    debug: Option<RawDebugInfo>,
    split_debuginfo: Option<String>,
    strip: Option<RawStrip>,
    debug_assertions: Option<bool>,
    overflow_checks: Option<bool>,
    incremental: Option<bool>,
    codegen_units: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RawOptimizationLevel {
    Integer(i64),
    String(String),
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RawDebugInfo {
    Boolean(bool),
    Integer(i64),
    String(String),
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RawStrip {
    Boolean(bool),
    String(String),
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RawLto {
    Boolean(bool),
    String(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    const NRF_MANIFEST: &str = "personal-hopspot/embedded/nrf52840/Cargo.toml";
    const ESP_MANIFEST: &str = "personal-hopspot/embedded/esp32/Cargo.toml";

    #[test]
    fn configured_nrf_release_settings_are_normalized() -> Result<(), BuildSettingsError> {
        let settings = resolve(repository(), NRF_MANIFEST, LtoMode::Configured)?;
        assert_eq!(
            settings.effective_release,
            ReleaseSettingsIdentity {
                opt_level: OptimizationLevelIdentity::SizeMin,
                debug: DebugInfoIdentity::Full,
                split_debuginfo: SplitDebuginfoIdentity::ToolchainDefault,
                strip: StripIdentity::None,
                debug_assertions: false,
                overflow_checks: false,
                lto: CargoLtoIdentity::Fat,
                panic: PanicStrategyIdentity::Unwind,
                incremental: false,
                codegen_units: 1,
                rpath: false,
            }
        );
        assert!(settings.package_overrides.is_empty());
        assert_eq!(
            settings.build_override.opt_level,
            OptimizationLevelIdentity::Zero
        );
        assert_eq!(settings.build_override.debug, DebugInfoIdentity::None);
        assert_eq!(settings.build_override.codegen_units, 256);
        Ok(())
    }

    #[test]
    fn explicit_lto_changes_only_effective_lto() -> Result<(), BuildSettingsError> {
        let configured = resolve(repository(), NRF_MANIFEST, LtoMode::Configured)?;
        let thin = resolve(repository(), NRF_MANIFEST, LtoMode::Thin)?;
        assert_eq!(thin.effective_release.lto, CargoLtoIdentity::Thin);
        assert_eq!(
            package_settings(&configured.effective_release),
            package_settings(&thin.effective_release)
        );
        assert_eq!(configured.package_overrides, thin.package_overrides);
        assert_eq!(configured.build_override, thin.build_override);
        Ok(())
    }

    #[test]
    fn package_override_is_recorded_as_effective_settings() -> Result<(), BuildSettingsError> {
        let settings = resolve(repository(), ESP_MANIFEST, LtoMode::Configured)?;
        assert_eq!(settings.package_overrides.len(), 1);
        let override_ = &settings.package_overrides[0];
        assert_eq!(override_.package, "esp-radio");
        assert_eq!(
            override_.settings.opt_level,
            OptimizationLevelIdentity::Three
        );
        assert_eq!(override_.settings.codegen_units, 1);
        assert_eq!(override_.settings.debug, DebugInfoIdentity::Full);
        Ok(())
    }

    fn repository() -> &'static Path {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .expect("resources crate must live below the repository root")
    }
}
