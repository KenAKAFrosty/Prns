use std::env;
use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

const TOOL_MANIFEST_DIR: &str = env!("CARGO_MANIFEST_DIR");

#[derive(Debug, PartialEq, Eq)]
enum ArgumentError {
    ConflictingProfiles,
}

impl fmt::Display for ArgumentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ConflictingProfiles => {
                formatter.write_str("--debug cannot be combined with --release, -r, or --profile")
            }
        }
    }
}

fn main() -> ExitCode {
    let args: Vec<OsString> = env::args_os().skip(1).collect();
    if help_requested(&args) {
        print_help();
        return ExitCode::SUCCESS;
    }

    let manifest = repo_root().join("prnsd/Cargo.toml");
    let cargo_args = match cargo_arguments(&args, &manifest) {
        Ok(cargo_args) => cargo_args,
        Err(error) => {
            eprintln!("prnsd: {error}");
            return ExitCode::from(2);
        }
    };

    match Command::new("cargo").args(cargo_args).status() {
        Ok(status) if status.success() => ExitCode::SUCCESS,
        Ok(status) => match status.code() {
            Some(code) => {
                eprintln!("prnsd: cargo exited with status {code}");
                ExitCode::from(code.clamp(1, 255) as u8)
            }
            None => {
                eprintln!("prnsd: cargo exited unsuccessfully");
                ExitCode::FAILURE
            }
        },
        Err(error) => {
            eprintln!("prnsd: failed to run cargo: {error}");
            ExitCode::FAILURE
        }
    }
}

fn cargo_arguments(args: &[OsString], manifest: &Path) -> Result<Vec<OsString>, ArgumentError> {
    let separator = separator_index(args);
    let build_args = &args[..separator];
    let debug = build_args.iter().any(|arg| arg == "--debug");
    let release = build_args
        .iter()
        .any(|arg| arg == "--release" || arg == "-r");
    let profile = build_args.iter().any(|arg| {
        arg == "--profile"
            || arg
                .to_str()
                .is_some_and(|arg| arg.starts_with("--profile="))
    });

    if debug && (release || profile) {
        return Err(ArgumentError::ConflictingProfiles);
    }

    let mut cargo_args = vec![
        OsString::from("run"),
        OsString::from("--manifest-path"),
        manifest.as_os_str().to_owned(),
    ];
    if !debug && !release && !profile {
        cargo_args.push(OsString::from("--release"));
    }
    for (index, arg) in args.iter().enumerate() {
        if index >= separator || arg != "--debug" {
            cargo_args.push(arg.clone());
        }
    }
    Ok(cargo_args)
}

fn help_requested(args: &[OsString]) -> bool {
    args[..separator_index(args)]
        .iter()
        .any(|arg| arg == "--help" || arg == "-h")
}

fn separator_index(args: &[OsString]) -> usize {
    args.iter()
        .position(|arg| arg == "--")
        .unwrap_or(args.len())
}

fn repo_root() -> PathBuf {
    PathBuf::from(TOOL_MANIFEST_DIR)
        .parent()
        .and_then(Path::parent)
        .expect("tools/prnsd lives under tools/")
        .to_path_buf()
}

fn print_help() {
    println!(
        "Run the Personal Reticulum daemon.\n\n\
Usage:\n    cargo prnsd [BUILD OPTIONS] [-- PRNSD OPTIONS]\n\n\
Profiles:\n    (default)             Build and run with --release\n    --debug               Build and run with Cargo's development profile\n    -r, --release         Build and run with the release profile\n    --profile <PROFILE>   Build and run with a named Cargo profile\n\n\
Other build options are passed to cargo run. Arguments after -- are passed to prnsd.\n\
Runtime log verbosity is controlled separately with RUST_LOG.\n\n\
Examples:\n    cargo prnsd\n    cargo prnsd --debug\n    cargo prnsd --features otlp -- --config \"$HOME/.reticulum\"\n    RUST_LOG=debug cargo prnsd\n    cargo prnsd -- --help"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    fn expected(values: &[&str]) -> Result<Vec<OsString>, ArgumentError> {
        Ok(args(values))
    }

    #[test]
    fn release_is_the_default_profile() {
        assert_eq!(
            cargo_arguments(&[], Path::new("/repo/prnsd/Cargo.toml")),
            expected(&[
                "run",
                "--manifest-path",
                "/repo/prnsd/Cargo.toml",
                "--release",
            ])
        );
    }

    #[test]
    fn debug_selects_the_development_profile() {
        assert_eq!(
            cargo_arguments(&args(&["--debug"]), Path::new("prnsd/Cargo.toml")),
            expected(&["run", "--manifest-path", "prnsd/Cargo.toml"])
        );
    }

    #[test]
    fn explicit_release_profiles_are_forwarded_once() {
        for release in ["--release", "-r"] {
            assert_eq!(
                cargo_arguments(&args(&[release]), Path::new("prnsd/Cargo.toml")),
                expected(&["run", "--manifest-path", "prnsd/Cargo.toml", release])
            );
        }
    }

    #[test]
    fn named_profiles_override_the_default() {
        assert_eq!(
            cargo_arguments(
                &args(&["--profile", "profiling"]),
                Path::new("prnsd/Cargo.toml"),
            ),
            expected(&[
                "run",
                "--manifest-path",
                "prnsd/Cargo.toml",
                "--profile",
                "profiling",
            ])
        );
        assert_eq!(
            cargo_arguments(
                &args(&["--profile=profiling"]),
                Path::new("prnsd/Cargo.toml"),
            ),
            expected(&[
                "run",
                "--manifest-path",
                "prnsd/Cargo.toml",
                "--profile=profiling",
            ])
        );
    }

    #[test]
    fn debug_rejects_other_profile_selectors() {
        for conflict in [
            args(&["--debug", "--release"]),
            args(&["--debug", "-r"]),
            args(&["--debug", "--profile", "dev"]),
            args(&["--debug", "--profile=dev"]),
        ] {
            assert_eq!(
                cargo_arguments(&conflict, Path::new("prnsd/Cargo.toml")),
                Err(ArgumentError::ConflictingProfiles)
            );
        }
    }

    #[test]
    fn cargo_build_options_are_forwarded() {
        assert_eq!(
            cargo_arguments(
                &args(&["--features", "otlp", "--locked", "--offline"]),
                Path::new("prnsd/Cargo.toml"),
            ),
            expected(&[
                "run",
                "--manifest-path",
                "prnsd/Cargo.toml",
                "--release",
                "--features",
                "otlp",
                "--locked",
                "--offline",
            ])
        );
    }

    #[test]
    fn daemon_arguments_after_the_separator_are_untouched() {
        assert_eq!(
            cargo_arguments(
                &args(&["--features", "otlp", "--", "--debug", "--config", "path"]),
                Path::new("prnsd/Cargo.toml"),
            ),
            expected(&[
                "run",
                "--manifest-path",
                "prnsd/Cargo.toml",
                "--release",
                "--features",
                "otlp",
                "--",
                "--debug",
                "--config",
                "path",
            ])
        );
    }

    #[test]
    fn help_only_belongs_to_the_launcher_before_the_separator() {
        assert!(help_requested(&args(&["--help"])));
        assert!(help_requested(&args(&["--features", "otlp", "-h"])));
        assert!(!help_requested(&args(&["--", "--help"])));
    }
}
