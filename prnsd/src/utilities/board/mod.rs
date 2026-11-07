mod args;

pub use args::BoardArgs;
use args::BoardCommand;

use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

use data_encoding::HEXLOWER;
use personal_rns::identity::{IdentityPublicKeys, PublicIdentityMaterial, IDENTITY_PUBLIC_KEY_LEN};
use personal_rns::interfaces::subghz::{ResolvedSubGMode, SubGConfiguration, SubGRegion};
use personal_rns::interfaces::{ConnectionState, InterfaceId, InterfaceKind, InterfaceMode};
use personal_rns::remote_control::{
    RemoteControlControllerAuthority, RemoteControlInterfaceConfigOutcome,
    RemoteControlInterfaceContinuation, RemoteControlInterfaceEntry, RemoteControlInterfacePage,
    RemoteControlLoRaOutcome, RemoteControlLoRaProfile, RemoteControlRequestSet,
    RemoteControlTargetAccess, RemoteControlTargetIdentity,
};
use personal_rns::runtime::{RemoteControlTargetAccessControl, RemoteControlTargetHandle};
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
use personal_rns::usb_auto::enrol_webusb_board;

use super::configuration::LoadedConfiguration;
use super::session::{
    UtilityNodeClient, UtilityNodeIdentity, UtilityNodeSession, UtilityPathError,
    UtilityRemoteControlIdentity,
};
use crate::daemon::identity::load_or_create_remote_control_identities;

const TARGET_FILE: &str = "board_target";
const INTERFACE_PAGE_LIMIT: usize = 256;
const APPLY_READBACK_DELAY: Duration = Duration::from_secs(2);

pub async fn run(args: BoardArgs) -> Result<(), BoardError> {
    let context = BoardContext::open(args.config())?;
    match args.command {
        BoardCommand::Identity(_) => {
            let controller = context.controller()?;
            println!(
                "controller hash: {}",
                hex(controller.identity_hash().as_bytes())
            );
            println!("controller key: {}", hex(&controller.public_key_bytes()));
        }
        #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
        BoardCommand::Enrol(_) => {
            let controller = context.controller()?;
            let target = enrol_webusb_board(&controller).await?;
            context.save_target(&target)?;
            println!(
                "enrolled {} on the board",
                hex(controller.identity_hash().as_bytes())
            );
            println!("board target key: {}", hex(&target.public_key_bytes()));
            println!(
                "board target hash: {}",
                hex(target.identity_hash().as_bytes())
            );
            println!(
                "last enrolled board saved in {}",
                context.target_path.display()
            );
        }
        BoardCommand::Lora(options) => {
            let profile = auto_lora_profile(options.region)?;
            let board = context.target(options.target_key)?;
            let applied =
                apply_lora_profile(&context, &board, profile, options.path_timeout).await?;
            println!("board {}", hex(board.identity_hash().as_bytes()));
            println!(
                "interface {} {}",
                hex(applied.interface.as_bytes()),
                outcome_text(applied.outcome)
            );
            if let Some(card) = applied.readback {
                println!("read back {}", card);
            }
        }
        BoardCommand::Status(options) => {
            let board = context.target(options.target_key)?;
            let entries = board_interfaces(&context, &board, options.path_timeout).await?;
            println!("board {}", hex(board.identity_hash().as_bytes()));
            for entry in entries {
                println!(
                    "  {:<14} {:<12} {:>16} {:>14} tx {} B rx {} B links {}",
                    kind_text(entry.kind),
                    connection_text(entry.connection),
                    hex(entry.id.as_bytes()),
                    mode_text(entry.mode),
                    entry.tx_bytes,
                    entry.rx_bytes,
                    entry.links
                );
            }
        }
    }
    Ok(())
}

struct BoardContext {
    configuration: LoadedConfiguration,
    target_path: PathBuf,
}

impl BoardContext {
    fn open(config: Option<PathBuf>) -> Result<Self, BoardError> {
        let configuration =
            LoadedConfiguration::load(config.as_deref()).map_err(BoardError::Configuration)?;
        Ok(Self {
            target_path: Self::storage_dir(&configuration).join(TARGET_FILE),
            configuration,
        })
    }

    fn storage_dir(configuration: &LoadedConfiguration) -> PathBuf {
        configuration.discovered.dir.join("storage")
    }

    fn controller(&self) -> Result<IdentityPublicKeys, BoardError> {
        Self::controller_identities(&self.configuration)
    }

    fn controller_identities(
        configuration: &LoadedConfiguration,
    ) -> Result<IdentityPublicKeys, BoardError> {
        let bootstrap = load_or_create_remote_control_identities(&Self::storage_dir(configuration))
            .map_err(BoardError::RemoteControlIdentity)?;
        let (secrets, _) = bootstrap.into_parts();
        Ok(*secrets.identities().controller().public_keys())
    }

    fn target(
        &self,
        explicit: Option<[u8; IDENTITY_PUBLIC_KEY_LEN]>,
    ) -> Result<IdentityPublicKeys, BoardError> {
        let bytes = match explicit {
            Some(bytes) => bytes,
            None => self.saved_target()?,
        };
        let material = PublicIdentityMaterial::from_slice(&bytes).map_err(BoardError::TargetKey)?;
        Ok(material.public_keys().to_owned())
    }

    fn saved_target(&self) -> Result<[u8; IDENTITY_PUBLIC_KEY_LEN], BoardError> {
        let text = std::fs::read_to_string(&self.target_path).map_err(|source| {
            BoardError::TargetFile {
                path: self.target_path.clone(),
                source,
            }
        })?;
        let bytes = HEXLOWER.decode(text.trim().as_bytes()).map_err(|source| {
            BoardError::TargetFileContent {
                path: self.target_path.clone(),
                source: source.to_string(),
            }
        })?;
        bytes
            .as_slice()
            .try_into()
            .map_err(|_| BoardError::TargetFileContent {
                path: self.target_path.clone(),
                source: format!("expected {IDENTITY_PUBLIC_KEY_LEN} hexadecimal bytes"),
            })
    }

    fn save_target(&self, target: &IdentityPublicKeys) -> Result<(), BoardError> {
        let parent = self.target_path.parent().ok_or(BoardError::TargetPath)?;
        std::fs::create_dir_all(parent).map_err(|source| BoardError::TargetFile {
            path: parent.to_path_buf(),
            source,
        })?;
        std::fs::write(
            &self.target_path,
            format!("{}\n", hex(&target.public_key_bytes())),
        )
        .map_err(|source| BoardError::TargetFile {
            path: self.target_path.clone(),
            source,
        })
    }
}

struct AppliedProfile {
    interface: InterfaceId,
    outcome: RemoteControlLoRaOutcome,
    readback: Option<String>,
}

fn auto_lora_profile(region: args::BoardRegion) -> Result<RemoteControlLoRaProfile, BoardError> {
    let configuration = SubGConfiguration::auto_lora_for(region.region()).map_err(|_| {
        BoardError::RegionHasNoAutoProfile {
            region: region.region(),
        }
    })?;
    let ResolvedSubGMode::LoRa(profile) = configuration.resolve();
    RemoteControlLoRaProfile::from_profile(profile).ok_or(BoardError::ProfileRejected {
        region: region.region(),
    })
}

async fn apply_lora_profile(
    context: &BoardContext,
    board: &IdentityPublicKeys,
    profile: RemoteControlLoRaProfile,
    path_timeout: Duration,
) -> Result<AppliedProfile, BoardError> {
    let session = connect(context, path_timeout).await?;
    session
        .run(move |client| async move {
            let interface = lora_interface(&client, board, path_timeout).await?;
            let outcome = {
                let connection = board_connection(&client, board, path_timeout).await?;
                let (outcome, _rtt) = connection
                    .set_interface_lora_profile(interface, profile)
                    .await
                    .map_err(BoardError::Operation)?;
                connection.close();
                outcome
            };
            if outcome != RemoteControlLoRaOutcome::Applied {
                return Ok(AppliedProfile {
                    interface,
                    outcome,
                    readback: None,
                });
            }
            // Applying a profile restarts the board's LoRa interface, so an immediate
            // readback reports the interface as gone.
            tokio::time::sleep(APPLY_READBACK_DELAY).await;
            let readback = {
                let connection = board_connection(&client, board, path_timeout).await?;
                let interface = lora_interface(&client, board, path_timeout).await?;
                match connection.inventory_interface_config(interface).await {
                    Ok((RemoteControlInterfaceConfigOutcome::Card(card), _rtt)) => {
                        Some(card.config.to_string())
                    }
                    Ok((other, _rtt)) => Some(format!("{other:?}")),
                    Err(error) => Some(format!("{error:?}")),
                }
            };
            Ok(AppliedProfile {
                interface,
                outcome,
                readback,
            })
        })
        .await
        .map_err(BoardError::SessionStopped)?
}

async fn board_interfaces(
    context: &BoardContext,
    board: &IdentityPublicKeys,
    path_timeout: Duration,
) -> Result<Vec<RemoteControlInterfaceEntry>, BoardError> {
    let session = connect(context, path_timeout).await?;
    session
        .run(move |client| async move {
            let connection = board_connection(&client, board, path_timeout).await?;
            let entries = inventory_interfaces(&connection).await?;
            connection.close();
            Ok(entries)
        })
        .await
        .map_err(BoardError::SessionStopped)?
}

async fn connect(
    context: &BoardContext,
    rpc_timeout: Duration,
) -> Result<UtilityNodeSession, BoardError> {
    let secrets = load_or_create_remote_control_identities(&BoardContext::storage_dir(
        &context.configuration,
    ))
    .map_err(BoardError::RemoteControlIdentity)?
    .into_parts()
    .0;
    UtilityNodeSession::connect_with_remote_control(
        &context.configuration,
        UtilityNodeIdentity::Anonymous,
        UtilityRemoteControlIdentity::Persistent(Box::new(secrets)),
        rpc_timeout,
    )
    .await
    .map_err(BoardError::Session)
}

async fn board_connection<'client>(
    client: &'client UtilityNodeClient,
    board: &IdentityPublicKeys,
    path_timeout: Duration,
) -> Result<RemoteControlTargetHandle<'client>, BoardError> {
    let target = RemoteControlTargetIdentity::new(*board);
    let endpoint = target.endpoint().destination_hash();
    let target_hash = target.identity_hash();
    let grant = RemoteControlTargetAccess::new(
        target,
        RemoteControlControllerAuthority::Administrator,
        RemoteControlRequestSet::all(),
    )
    .map_err(BoardError::Grant)?;
    client
        .handle()
        .set_remote_control_target_access(grant)
        .await
        .map_err(BoardError::Provision)?;
    client
        .ensure_path(endpoint, path_timeout)
        .await
        .map_err(|source| BoardError::Path { source, endpoint })?;
    client
        .handle()
        .connect_remote_control_target(target_hash)
        .await
        .map_err(BoardError::Connect)
}

async fn lora_interface(
    client: &UtilityNodeClient,
    board: &IdentityPublicKeys,
    path_timeout: Duration,
) -> Result<InterfaceId, BoardError> {
    let connection = board_connection(client, board, path_timeout).await?;
    let found = inventory_interfaces(&connection)
        .await?
        .into_iter()
        .find(|entry| entry.kind == InterfaceKind::LoRa)
        .map(|entry| entry.id)
        .ok_or(BoardError::NoLoRaInterface)?;
    connection.close();
    Ok(found)
}

async fn inventory_interfaces(
    connection: &RemoteControlTargetHandle<'_>,
) -> Result<Vec<RemoteControlInterfaceEntry>, BoardError> {
    let mut entries = Vec::new();
    let mut page = RemoteControlInterfacePage::First;
    for _ in 0..INTERFACE_PAGE_LIMIT {
        let (inventory, _rtt) = connection
            .inventory_interfaces_page(page)
            .await
            .map_err(BoardError::Operation)?;
        entries.extend(inventory.entries().iter().copied());
        match inventory.continuation() {
            RemoteControlInterfaceContinuation::Complete => return Ok(entries),
            RemoteControlInterfaceContinuation::More(cursor) => {
                page = RemoteControlInterfacePage::After(cursor)
            }
        }
    }
    Err(BoardError::InventoryOverflow)
}

fn hex(bytes: &[u8]) -> String {
    HEXLOWER.encode(bytes)
}

fn outcome_text(outcome: RemoteControlLoRaOutcome) -> String {
    match outcome {
        RemoteControlLoRaOutcome::Applied => "profile applied".to_owned(),
        RemoteControlLoRaOutcome::UnknownInterface => "board has no such interface".to_owned(),
        RemoteControlLoRaOutcome::Failed => "board refused the profile".to_owned(),
    }
}

fn kind_text(kind: InterfaceKind) -> &'static str {
    match kind {
        InterfaceKind::LoRa => "LoRa",
        InterfaceKind::BluetoothAuto => "Bluetooth",
        InterfaceKind::UsbAutoDevice => "USB",
        _ => "Other",
    }
}

fn connection_text(connection: ConnectionState) -> &'static str {
    match connection {
        ConnectionState::Connected => "Connected",
        ConnectionState::Disconnected => "Disconnected",
        ConnectionState::Reconnecting => "Reconnecting",
        ConnectionState::Initializing => "Initializing",
        ConnectionState::Degraded => "Degraded",
        ConnectionState::Failed => "Failed",
        ConnectionState::Disabled => "Disabled",
        ConnectionState::Unknown => "Unknown",
    }
}

fn mode_text(mode: InterfaceMode) -> &'static str {
    match mode {
        InterfaceMode::Full => "full",
        InterfaceMode::PointToPoint => "point-to-point",
        InterfaceMode::AccessPoint => "access point",
        InterfaceMode::Roaming => "roaming",
        InterfaceMode::Boundary => "boundary",
        InterfaceMode::Gateway => "gateway",
        InterfaceMode::Internal => "internal",
    }
}

#[derive(Debug)]
pub enum BoardError {
    Configuration(super::configuration::UtilityConfigurationError),
    RemoteControlIdentity(personal_rns::runtime::RemoteControlFileIdentityBootstrapError),
    Enrolment(personal_rns::usb_auto::WebUsbEnrolmentError),
    TargetKey(personal_rns::identity::IdentityMaterialLengthError),
    TargetPath,
    TargetFile {
        path: PathBuf,
        source: std::io::Error,
    },
    TargetFileContent {
        path: PathBuf,
        source: String,
    },
    RegionHasNoAutoProfile {
        region: SubGRegion,
    },
    ProfileRejected {
        region: SubGRegion,
    },
    Session(super::session::UtilityNodeSessionError),
    SessionStopped(super::session::UtilityNodeStopped),
    Grant(personal_rns::remote_control::RemoteControlTargetAccessError),
    Provision(personal_rns::runtime::SetRemoteControlTargetAccessControlError),
    Path {
        source: UtilityPathError,
        endpoint: personal_rns::wire::DestinationHash,
    },
    Connect(personal_rns::runtime::ConnectRemoteControlTargetError),
    Operation(personal_rns::runtime::RemoteControlTargetOperationError),
    NoLoRaInterface,
    InventoryOverflow,
}

impl fmt::Display for BoardError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Configuration(source) => write!(formatter, "{source}"),
            Self::RemoteControlIdentity(source) => {
                write!(formatter, "controller identity: {source}")
            }
            Self::Enrolment(source) => write!(formatter, "{source}"),
            Self::TargetKey(source) => write!(formatter, "target key: {source}"),
            Self::TargetPath => write!(formatter, "the daemon storage directory has no parent"),
            Self::TargetFile { path, source } => {
                write!(formatter, "board target file {}: {source}", path.display())
            }
            Self::TargetFileContent { path, source } => {
                write!(formatter, "board target file {}: {source}", path.display())
            }
            Self::RegionHasNoAutoProfile { region } => {
                write!(
                    formatter,
                    "region {} has no auto LoRa profile",
                    region.label()
                )
            }
            Self::ProfileRejected { region } => {
                write!(
                    formatter,
                    "region {} profile rejected by the radio contract",
                    region.label()
                )
            }
            Self::Session(source) => write!(formatter, "{source}"),
            Self::SessionStopped(source) => write!(formatter, "{source}"),
            Self::Grant(source) => write!(formatter, "board grant: {source:?}"),
            Self::Provision(source) => write!(formatter, "board target provisioning: {source:?}"),
            Self::Path { source, endpoint } => write!(
                formatter,
                "no path to board endpoint {}: {source}",
                hex(endpoint.as_bytes())
            ),
            Self::Connect(source) => write!(formatter, "board connection: {source:?}"),
            Self::Operation(source) => write!(formatter, "board request: {source:?}"),
            Self::NoLoRaInterface => write!(formatter, "the board reports no LoRa interface"),
            Self::InventoryOverflow => {
                write!(
                    formatter,
                    "the board inventory exceeded {INTERFACE_PAGE_LIMIT} pages"
                )
            }
        }
    }
}

impl std::error::Error for BoardError {}

impl From<personal_rns::usb_auto::WebUsbEnrolmentError> for BoardError {
    fn from(source: personal_rns::usb_auto::WebUsbEnrolmentError) -> Self {
        Self::Enrolment(source)
    }
}
