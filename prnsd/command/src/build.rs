use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::Deserialize;

use crate::arguments::{option_present, validate_profiles, ArgumentError, Invocation};
use crate::CommandError;

#[derive(Deserialize)]
#[serde(tag = "reason", rename_all = "kebab-case")]
enum CargoMessage {
    CompilerArtifact {
        target: CargoTarget,
        executable: Option<PathBuf>,
    },
    CompilerMessage {
        message: CargoDiagnostic,
    },
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
struct CargoTarget {
    name: String,
    kind: Vec<String>,
}

#[derive(Deserialize)]
struct CargoDiagnostic {
    rendered: Option<String>,
}

#[derive(Default)]
struct DaemonArtifact {
    executable: Option<PathBuf>,
}

impl DaemonArtifact {
    fn record(
        &mut self,
        target: CargoTarget,
        executable: Option<PathBuf>,
    ) -> Result<(), CommandError> {
        if target.name != "prnsd" || !target.kind.iter().any(|kind| kind == "bin") {
            return Ok(());
        }
        let Some(executable) = executable else {
            return Ok(());
        };
        match &self.executable {
            None => self.executable = Some(executable),
            Some(existing) if existing == &executable => {}
            Some(existing) => {
                return Err(CommandError::DaemonArtifactConflict {
                    first: existing.clone(),
                    second: executable,
                });
            }
        }
        Ok(())
    }

    fn finish(self) -> Result<PathBuf, CommandError> {
        self.executable.ok_or(CommandError::DaemonArtifactMissing)
    }
}

fn process_cargo_message(
    artifact: &mut DaemonArtifact,
    message: CargoMessage,
) -> Result<Option<String>, CommandError> {
    match message {
        CargoMessage::CompilerArtifact { target, executable } => {
            artifact.record(target, executable)?;
            Ok(None)
        }
        CargoMessage::CompilerMessage { message } => Ok(message.rendered),
        CargoMessage::Other => Ok(None),
    }
}

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
    cargo_build_artifact(build_args, root)
}

fn cargo_build_artifact(args: Vec<OsString>, working_dir: &Path) -> Result<PathBuf, CommandError> {
    let mut child = Command::new("cargo")
        .args(args)
        .current_dir(working_dir)
        .stdout(Stdio::piped())
        .spawn()
        .map_err(CommandError::CargoSpawn)?;
    let Some(stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err(CommandError::CargoStdoutUnavailable);
    };
    let mut artifact = DaemonArtifact::default();
    let messages = serde_json::Deserializer::from_reader(stdout).into_iter::<CargoMessage>();
    let message_result: Result<(), CommandError> = messages
        .map(|message| message.map_err(CommandError::CargoMessage))
        .try_for_each(|message| {
            if let Some(rendered) = process_cargo_message(&mut artifact, message?)? {
                eprint!("{rendered}");
            }
            Ok(())
        });
    let status = child.wait().map_err(CommandError::CargoWait)?;
    if !status.success() {
        return Err(CommandError::CargoFailed(status.code()));
    }
    message_result?;
    artifact.finish()
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
        cargo_args.push(OsString::from("--message-format"));
        cargo_args.push(OsString::from("json-render-diagnostics"));
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
                "--message-format",
                "json-render-diagnostics",
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
                "--message-format",
                "json-render-diagnostics",
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
                "--message-format",
                "json-render-diagnostics",
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
            assert_eq!(built[7..], args(&values));
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
                "--message-format",
                "json-render-diagnostics",
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
                "--message-format",
                "json-render-diagnostics",
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

    fn cargo_message(value: &str) -> CargoMessage {
        serde_json::from_str(value).unwrap()
    }

    #[test]
    fn cargo_artifacts_select_the_daemon_executable() {
        let mut artifact = DaemonArtifact::default();
        process_cargo_message(
            &mut artifact,
            cargo_message(
                r#"{"reason":"compiler-artifact","target":{"name":"dependency","kind":["lib"]},"executable":"/target/dependency"}"#,
            ),
        )
        .unwrap();
        process_cargo_message(
            &mut artifact,
            cargo_message(
                r#"{"reason":"compiler-artifact","target":{"name":"prnsd","kind":["bin"]},"executable":"/custom-target/prnsd"}"#,
            ),
        )
        .unwrap();
        assert_eq!(
            artifact.finish().unwrap(),
            Path::new("/custom-target/prnsd")
        );
    }

    #[test]
    fn missing_daemon_artifact_is_rejected() {
        assert!(matches!(
            DaemonArtifact::default().finish(),
            Err(CommandError::DaemonArtifactMissing)
        ));
    }

    #[test]
    fn conflicting_daemon_artifacts_are_rejected() {
        let mut artifact = DaemonArtifact::default();
        artifact
            .record(
                CargoTarget {
                    name: String::from("prnsd"),
                    kind: vec![String::from("bin")],
                },
                Some(PathBuf::from("/first/prnsd")),
            )
            .unwrap();
        assert!(matches!(
            artifact.record(
                CargoTarget {
                    name: String::from("prnsd"),
                    kind: vec![String::from("bin")],
                },
                Some(PathBuf::from("/second/prnsd")),
            ),
            Err(CommandError::DaemonArtifactConflict { .. })
        ));
    }

    #[test]
    fn malformed_cargo_message_is_rejected() {
        assert!(serde_json::from_str::<CargoMessage>("{").is_err());
    }
}
