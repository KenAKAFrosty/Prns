use std::{net::SocketAddr, num::NonZeroU16};

use serde::Deserialize;

use super::*;

const CONFIGURATION_BYTES: u64 = 4096;
const MAX_QUALIFICATION_SECONDS: u16 = 300;
const PUBLIC_KEY_BYTES: usize = 64;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct LaunchConfiguration {
    listen: SocketAddr,
    tcp_mode: TcpMode,
    radio: Radio,
}

#[derive(Deserialize)]
enum TcpMode {
    Gateway,
    PointToPoint,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
enum Radio {
    Disabled,
    HaLow { device: String, scope: String },
}

#[derive(Clone, clap::ValueEnum)]
pub(super) enum ControllerAccess {
    Inspection,
    ApplicationProbe,
    InterfaceWatch,
}

#[derive(Debug, thiserror::Error)]
pub(super) enum QualificationInputError {
    #[error("controller public key must contain exactly 128 hexadecimal characters")]
    ControllerKey,
    #[error("qualification duration must be 1..=300 seconds")]
    Duration,
}

#[derive(Clone)]
pub(super) struct ControllerPublicKey(String);

impl std::str::FromStr for ControllerPublicKey {
    type Err = QualificationInputError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let mut bytes = [0; PUBLIC_KEY_BYTES];
        hex::decode_to_slice(value, &mut bytes)
            .map_err(|_| QualificationInputError::ControllerKey)?;
        Ok(Self(hex::encode(bytes)))
    }
}

#[derive(Clone)]
pub(super) struct QualificationWindow(NonZeroU16);

impl std::str::FromStr for QualificationWindow {
    type Err = QualificationInputError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let seconds = value
            .parse::<NonZeroU16>()
            .map_err(|_| QualificationInputError::Duration)?;
        if seconds.get() > MAX_QUALIFICATION_SECONDS {
            return Err(QualificationInputError::Duration);
        }
        Ok(Self(seconds))
    }
}

pub(super) enum Purpose {
    Service,
    Qualification {
        controller: ControllerPublicKey,
        access: ControllerAccess,
        window: QualificationWindow,
    },
}

pub(super) struct LaunchInputs<'a> {
    pub root: &'a Path,
    pub config: &'a Path,
    pub ram_directory: &'a Path,
    pub ram_reserve_bytes: u64,
}

impl LaunchConfiguration {
    fn arguments(&self, state: &Path, purpose: Purpose) -> Result<Vec<String>, CommandError> {
        let mut arguments = vec![
            "--state-dir".to_owned(),
            state.to_string_lossy().into_owned(),
            "--listen".to_owned(),
            self.listen.to_string(),
            "--tcp-mode".to_owned(),
            match self.tcp_mode {
                TcpMode::Gateway => "gateway",
                TcpMode::PointToPoint => "point-to-point",
            }
            .to_owned(),
        ];
        match &self.radio {
            Radio::Disabled => {}
            Radio::HaLow { device, scope } => {
                if device.is_empty()
                    || device.len() > 15
                    || !device
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"_-.".contains(&byte))
                    || scope.is_empty()
                    || scope.len() > 64
                    || !scope
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || b"_-.".contains(&byte))
                {
                    return Err(CommandError::Configuration);
                }
                arguments.extend([
                    "--halow-device".to_owned(),
                    device.clone(),
                    "--halow-scope".to_owned(),
                    scope.clone(),
                ]);
            }
        }
        match purpose {
            Purpose::Service => {}
            Purpose::Qualification {
                controller,
                access,
                window,
            } => arguments.extend([
                match access {
                    ControllerAccess::Inspection => "--controller-public-key",
                    ControllerAccess::ApplicationProbe => "--app-controller-public-key",
                    ControllerAccess::InterfaceWatch => "--watch-controller-public-key",
                }
                .to_owned(),
                controller.0,
                "--run-for".to_owned(),
                window.0.to_string(),
            ]),
        }
        Ok(arguments)
    }
}

pub(super) fn execute(
    mut appliance: Appliance<'_>,
    inputs: LaunchInputs<'_>,
    purpose: Purpose,
) -> Result<(), CommandError> {
    let LaunchInputs {
        root,
        config,
        ram_directory,
        ram_reserve_bytes,
    } = inputs;
    if ram_directory == Path::new("/tmp")
        || !ram_directory.starts_with("/tmp")
        || ram_directory.components().any(|part| {
            matches!(
                part,
                std::path::Component::ParentDir | std::path::Component::CurDir
            )
        })
        || ram_directory.starts_with(root)
    {
        return Err(CommandError::Configuration);
    }
    let mut bytes = Vec::new();
    fs::File::open(config)?
        .take(CONFIGURATION_BYTES + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > CONFIGURATION_BYTES {
        return Err(CommandError::Configuration);
    }
    let configuration: LaunchConfiguration = serde_json::from_slice(&bytes)?;
    let arguments = configuration.arguments(&root.join("state"), purpose)?;
    let space = inspection::ram_space(ram_reserve_bytes)?;
    let (executable, candidate) = appliance.prepare_launch(ram_directory, space)?;
    eprintln!("appliance_launch {}", serde_json::to_string(&candidate)?);
    drop(appliance);
    let mut command = Command::new(executable);
    command.args(arguments);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        Err(command.exec().into())
    }
    #[cfg(not(unix))]
    {
        Err(CommandError::UnsupportedHost)
    }
}

#[cfg(test)]
mod tests;
