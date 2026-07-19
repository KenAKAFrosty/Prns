use std::env;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::arguments::{option_present, validate_profiles, ArgumentError, Invocation};
use crate::CommandError;

pub(super) fn build_daemon(
    invocation: &Invocation,
    root: &Path,
    manifest: &Path,
    canonical: bool,
) -> Result<PathBuf, CommandError> {
    let build_args = if canonical {
        canonical_build_arguments(invocation, manifest)?
    } else {
        cargo_build_arguments(invocation, manifest)?
    };
    run_cargo(build_args, root)?;
    let binary = daemon_binary_path(
        &invocation.build_args,
        root,
        manifest,
        env::var_os("CARGO_TARGET_DIR").as_deref(),
    );
    if binary.is_file() {
        Ok(binary)
    } else {
        Err(CommandError::BinaryMissing(binary))
    }
}

fn run_cargo(args: Vec<OsString>, working_dir: &Path) -> Result<(), CommandError> {
    let status = cargo_status(args, working_dir)?;
    if status.success() {
        Ok(())
    } else {
        Err(CommandError::CargoFailed(status.code()))
    }
}

pub(super) fn run_daemon_through_cargo(
    args: Vec<OsString>,
    working_dir: &Path,
) -> Result<(), CommandError> {
    let status = cargo_status(args, working_dir)?;
    if status.success() {
        Ok(())
    } else {
        Err(CommandError::DaemonExited(status.code()))
    }
}

fn cargo_status(
    args: Vec<OsString>,
    working_dir: &Path,
) -> Result<std::process::ExitStatus, CommandError> {
    let status = Command::new("cargo")
        .args(args)
        .current_dir(working_dir)
        .status()
        .map_err(CommandError::CargoSpawn)?;
    Ok(status)
}

fn cargo_build_arguments(
    invocation: &Invocation,
    manifest: &Path,
) -> Result<Vec<OsString>, ArgumentError> {
    cargo_build_arguments_with_mode(invocation, manifest, false)
}

fn canonical_build_arguments(
    invocation: &Invocation,
    manifest: &Path,
) -> Result<Vec<OsString>, ArgumentError> {
    cargo_build_arguments_with_mode(invocation, manifest, true)
}

fn cargo_build_arguments_with_mode(
    invocation: &Invocation,
    manifest: &Path,
    canonical: bool,
) -> Result<Vec<OsString>, ArgumentError> {
    let mut args = cargo_arguments("build", invocation, manifest, false)?;
    if canonical {
        if !args.iter().any(|arg| arg == "--locked") {
            args.push(OsString::from("--locked"));
        }
        args.push(OsString::from("--features"));
        args.push(OsString::from("otlp"));
    }
    Ok(args)
}

pub(super) fn cargo_run_arguments(
    invocation: &Invocation,
    manifest: &Path,
) -> Result<Vec<OsString>, ArgumentError> {
    cargo_arguments("run", invocation, manifest, true)
}

fn cargo_arguments(
    command: &str,
    invocation: &Invocation,
    manifest: &Path,
    include_daemon_args: bool,
) -> Result<Vec<OsString>, ArgumentError> {
    validate_profiles(&invocation.build_args)?;
    let debug = invocation.build_args.iter().any(|arg| arg == "--debug");
    let release = invocation
        .build_args
        .iter()
        .any(|arg| arg == "--release" || arg == "-r");
    let profile = option_present(&invocation.build_args, "--profile");

    let mut cargo_args = vec![
        OsString::from(command),
        OsString::from("--manifest-path"),
        manifest.as_os_str().to_owned(),
    ];
    if command == "build" {
        cargo_args.push(OsString::from("--bin"));
        cargo_args.push(OsString::from("prnsd"));
    }
    if !debug && !release && !profile {
        cargo_args.push(OsString::from("--release"));
    }
    cargo_args.extend(
        invocation
            .build_args
            .iter()
            .filter(|arg| *arg != "--debug")
            .cloned(),
    );
    if include_daemon_args {
        cargo_args.push(OsString::from("--"));
        cargo_args.extend(invocation.daemon_args.iter().cloned());
    }
    Ok(cargo_args)
}

fn daemon_binary_path(
    build_args: &[OsString],
    repo_root: &Path,
    manifest: &Path,
    cargo_target_dir: Option<&OsStr>,
) -> PathBuf {
    let target_dir = option_value(build_args, "--target-dir")
        .or_else(|| cargo_target_dir.map(OsStr::to_owned))
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            manifest
                .parent()
                .expect("prnsd manifest has a parent")
                .join("target")
        });
    let mut path = if target_dir.is_absolute() {
        target_dir
    } else {
        repo_root.join(target_dir)
    };
    if let Some(target) = option_value(build_args, "--target") {
        path.push(target);
    }
    path.push(profile_directory(build_args));
    path.push(if cfg!(windows) { "prnsd.exe" } else { "prnsd" });
    path
}

fn profile_directory(build_args: &[OsString]) -> OsString {
    if build_args.iter().any(|arg| arg == "--debug") {
        return OsString::from("debug");
    }
    match option_value(build_args, "--profile") {
        Some(profile) if profile == "dev" => OsString::from("debug"),
        Some(profile) => profile,
        None => OsString::from("release"),
    }
}

fn option_value(args: &[OsString], name: &str) -> Option<OsString> {
    for (index, arg) in args.iter().enumerate() {
        if arg == name {
            return args.get(index + 1).cloned();
        }
        if let Some(value) = arg
            .to_str()
            .and_then(|arg| arg.strip_prefix(name))
            .and_then(|arg| arg.strip_prefix('='))
        {
            return Some(OsString::from(value));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arguments::parse_invocation;

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    fn invocation(values: &[&str]) -> Invocation {
        parse_invocation(&args(values)).unwrap()
    }
    #[test]
    fn release_is_the_default_profile_for_builds() {
        assert_eq!(
            cargo_build_arguments(&invocation(&[]), Path::new("/repo/prnsd/Cargo.toml")),
            Ok(args(&[
                "build",
                "--manifest-path",
                "/repo/prnsd/Cargo.toml",
                "--bin",
                "prnsd",
                "--release",
            ]))
        );
    }

    #[test]
    fn canonical_build_is_locked_release_with_otlp() {
        assert_eq!(
            canonical_build_arguments(&invocation(&["build"]), Path::new("/repo/prnsd/Cargo.toml")),
            Ok(args(&[
                "build",
                "--manifest-path",
                "/repo/prnsd/Cargo.toml",
                "--bin",
                "prnsd",
                "--release",
                "--locked",
                "--features",
                "otlp",
            ]))
        );
    }

    #[test]
    fn debug_selects_the_development_profile() {
        assert_eq!(
            cargo_build_arguments(&invocation(&["--debug"]), Path::new("prnsd/Cargo.toml")),
            Ok(args(&[
                "build",
                "--manifest-path",
                "prnsd/Cargo.toml",
                "--bin",
                "prnsd",
            ]))
        );
    }

    #[test]
    fn explicit_release_and_named_profiles_are_forwarded_once() {
        for values in [
            vec!["--release"],
            vec!["-r"],
            vec!["--profile", "profiling"],
            vec!["--profile=profiling"],
        ] {
            let parsed = invocation(&values);
            let built = cargo_build_arguments(&parsed, Path::new("prnsd/Cargo.toml")).unwrap();
            assert_eq!(built[5..], args(&values));
        }
    }

    #[test]
    fn cargo_build_options_are_forwarded() {
        let parsed = invocation(&["--features", "otlp", "--locked", "--offline"]);
        assert_eq!(
            cargo_build_arguments(&parsed, Path::new("prnsd/Cargo.toml")),
            Ok(args(&[
                "build",
                "--manifest-path",
                "prnsd/Cargo.toml",
                "--bin",
                "prnsd",
                "--release",
                "--features",
                "otlp",
                "--locked",
                "--offline",
            ]))
        );
    }

    #[test]
    fn daemon_arguments_are_excluded_from_build_and_preserved_for_one_shot_runs() {
        let parsed = invocation(&["--features", "otlp", "--", "--version"]);
        assert_eq!(
            cargo_build_arguments(&parsed, Path::new("prnsd/Cargo.toml")),
            Ok(args(&[
                "build",
                "--manifest-path",
                "prnsd/Cargo.toml",
                "--bin",
                "prnsd",
                "--release",
                "--features",
                "otlp",
            ]))
        );
        assert_eq!(
            cargo_run_arguments(&parsed, Path::new("prnsd/Cargo.toml")),
            Ok(args(&[
                "run",
                "--manifest-path",
                "prnsd/Cargo.toml",
                "--release",
                "--features",
                "otlp",
                "--",
                "--version",
            ]))
        );
    }

    #[test]
    fn binary_path_tracks_profile_target_and_target_directory() {
        let manifest = Path::new("/repo/prnsd/Cargo.toml");
        assert_eq!(
            daemon_binary_path(&[], Path::new("/repo"), manifest, None),
            Path::new("/repo/prnsd/target/release/prnsd")
        );
        assert_eq!(
            daemon_binary_path(
                &args(&[
                    "--profile=profiling",
                    "--target",
                    "aarch64-apple-darwin",
                    "--target-dir",
                    "build-output",
                ]),
                Path::new("/repo"),
                manifest,
                None,
            ),
            Path::new("/repo/build-output/aarch64-apple-darwin/profiling/prnsd")
        );
        assert_eq!(
            daemon_binary_path(
                &args(&["--profile", "dev"]),
                Path::new("/repo"),
                manifest,
                None,
            ),
            Path::new("/repo/prnsd/target/debug/prnsd")
        );
    }
}
