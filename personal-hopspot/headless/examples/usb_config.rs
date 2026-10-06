//! Configure a screenless nRF52840 Hopspot over USB with Remote Control.
//!
//! `provision` merges the firmware UF2 with a Remote Control vault page that installs this
//! controller as the target's factory Administrator, so one drag-and-drop both flashes the board
//! and makes it trust this machine. Every other command connects over USB Auto, authenticates as
//! that controller, and performs one typed operation.
use std::path::{Path, PathBuf};
use std::time::Duration;

use clap::{Parser, Subcommand, ValueEnum};
use personal_hopspot_headless::control::public_identity;
use personal_hopspot_memory::{memory_profile_named, ProcessorArchitecture, RegionRole};
use personal_rns::identity::{PrivateIdentityMaterial, PublicIdentityMaterial};
use personal_rns::interfaces::subghz::RegulatoryRegion;
use personal_rns::interfaces::InterfaceId;
use personal_rns::prelude::*;
use personal_rns::remote_control::{
    encode_remote_control_vault_page, RemoteControlControllerAuthority,
    RemoteControlInterfaceConfigOutcome, RemoteControlInterfaceContinuation,
    RemoteControlInterfacePage, RemoteControlInterfacePower, RemoteControlLoRaProfile,
    RemoteControlRequestSet, RemoteControlSystemPower,
};
use personal_rns::runtime::{generate_identity_secret, RemoteControlIdentityDirectory};
use personal_rns::usb_auto::AutoUsb;
use serde_json::json;

/// Address of the `remote-control-identity` page in a UF2-flashed nRF52840 memory profile, which the
/// firmware reads at boot for the target identity and its factory owner grant.
fn remote_control_identity_address(board: &str) -> Result<u32, Error> {
    let unsupported = || Error::Board(board.to_string());
    let profile = memory_profile_named(board).ok_or_else(unsupported)?;
    if profile.architecture != ProcessorArchitecture::ThumbV7em {
        return Err(unsupported());
    }
    let region = profile
        .unique_region_for_role(RegionRole::RemoteControlIdentity)
        .map_err(|_| unsupported())?;
    u32::try_from(region.range.start()).map_err(|_| unsupported())
}

const UF2_MAGIC_START0: u32 = 0x0A32_4655;
const UF2_MAGIC_START1: u32 = 0x9E5D_5157;
const UF2_MAGIC_END: u32 = 0x0AB1_6F30;
const UF2_FLAG_FAMILY_ID: u32 = 0x0000_2000;
const UF2_BLOCK_BYTES: usize = 512;
const UF2_PAYLOAD_BYTES: usize = 256;
const NRF52840_UF2_FAMILY: u32 = 0xADA5_2840;
const OPERATION_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Parser)]
struct Options {
    /// Private controller state; keep it, or the board must be re-provisioned.
    #[arg(long, default_value_os_t = default_state_dir())]
    state_dir: PathBuf,
    /// Name of the provisioned board inside the state directory.
    #[arg(long, default_value = "rak4631")]
    device: String,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Build a UF2 that flashes the firmware and installs this controller as the board's owner.
    Provision {
        /// The board's memory profile (rak4631, rak10724, wio-tracker-l1, ...); it sets where the
        /// owner page is written.
        #[arg(long, default_value = "rak4631")]
        board: String,
        /// Firmware UF2 from `tools/build/hopspot-nrf52840.sh BOARD`.
        #[arg(long)]
        firmware: PathBuf,
        /// Combined UF2 to write. It contains the board's private Remote Control key.
        #[arg(long)]
        out: PathBuf,
    },
    /// Build version, interfaces, and the LoRa configuration.
    Status,
    /// Set and persist the LoRa profile.
    Lora {
        /// Regulatory region label, e.g. US915, EU868; `Custom` skips regional limits.
        #[arg(long)]
        region: String,
        #[arg(long)]
        frequency_hz: u32,
        #[arg(long, value_parser = ["125", "250", "500"])]
        bandwidth_khz: String,
        #[arg(long, value_parser = clap::value_parser!(u8).range(5..=12))]
        spreading_factor: u8,
        /// Coding-rate denominator: 5 for 4/5 through 8 for 4/8.
        #[arg(long, value_parser = clap::value_parser!(u8).range(5..=8))]
        coding_rate: u8,
        #[arg(long)]
        tx_power_dbm: i8,
        #[arg(long, default_value_t = 18)]
        preamble: u16,
    },
    /// Turn one interface on or off (IDs come from `status`).
    Interface {
        #[arg(value_parser = interface_id)]
        id: InterfaceId,
        #[arg(value_enum)]
        power: Power,
    },
    /// Put the whole node to sleep or wake it.
    System {
        #[arg(value_enum)]
        power: SystemPower,
    },
    /// Announce the node's page on every interface.
    Announce,
    /// Give another controller (for example the Hopspot Configure page) full control.
    Authorize {
        /// The controller's 64-byte public key as 128 hex digits.
        #[arg(long)]
        public_key: String,
    },
    /// Print the board's public key, which other controllers need to address it.
    BoardKey,
    /// Print this controller's public key, to authorize it on a board owned elsewhere.
    OwnerKey,
    /// Show the board's announced name, or rename it with `--set`.
    Name {
        /// New name: 1 to 64 bytes of UTF-8, without control characters or surrounding whitespace.
        #[arg(long)]
        set: Option<String>,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum Power {
    On,
    Off,
}

#[derive(Clone, Copy, ValueEnum)]
enum SystemPower {
    Awake,
    Asleep,
}

fn default_state_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config/prns-usb-config")
}

fn interface_id(value: &str) -> Result<InterfaceId, String> {
    let bytes = hex::decode(value).map_err(|error| error.to_string())?;
    let bytes: [u8; 8] = bytes
        .try_into()
        .map_err(|_| "interface ID must be 8 bytes (16 hex digits)".to_owned())?;
    Ok(InterfaceId::new(bytes))
}

#[derive(Debug, thiserror::Error)]
enum Error {
    #[error(transparent)]
    Identity(#[from] personal_rns::runtime::RemoteControlFileIdentityBootstrapError),
    #[error("{0}")]
    Io(#[from] std::io::Error),
    #[error("controller state is in use by another run: {0}")]
    Lock(std::fs::TryLockError),
    #[error("firmware UF2: {0}")]
    Uf2(String),
    #[error("`{0}` is not a UF2-flashed nRF52840 memory profile")]
    Board(String),
    #[error("board `{0}` is not provisioned; run `provision` first")]
    NotProvisioned(String),
    #[error("device record: {0}")]
    Record(String),
    #[error("unknown region `{0}`; expected one of: {1}")]
    Region(String, String),
    #[error("the LoRa profile is outside what the firmware accepts")]
    Profile,
    #[error("target grant: {0:?}")]
    Grant(RemoteControlTargetAccessError),
    #[error("target provisioning: {0:?}")]
    Provision(personal_rns::runtime::SetRemoteControlTargetAccessControlError),
    #[error("target connection: {0:?}")]
    Connect(personal_rns::runtime::ConnectRemoteControlTargetError),
    #[error("remote operation: {0:?}")]
    Operation(personal_rns::runtime::RemoteControlTargetOperationError),
    #[error("board did not answer within {OPERATION_TIMEOUT:?} at stage: {0}")]
    Timeout(&'static str),
    #[error("remote inventory exceeded 256 pages")]
    Pagination,
    #[error("controller node stopped: {0:?}")]
    Node(Result<(), personal_rns::runtime::NodeRunError>),
}

fn device_record(state_dir: &Path, device: &str) -> PathBuf {
    state_dir.join("devices").join(format!("{device}.json"))
}

fn write_private(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    use std::io::Write;
    options.open(path)?.write_all(bytes)?;
    Ok(())
}

fn uf2_block(address: u32, payload: &[u8], index: u32, total: u32) -> [u8; UF2_BLOCK_BYTES] {
    let mut block = [0u8; UF2_BLOCK_BYTES];
    let words = [
        UF2_MAGIC_START0,
        UF2_MAGIC_START1,
        UF2_FLAG_FAMILY_ID,
        address,
        UF2_PAYLOAD_BYTES as u32,
        index,
        total,
        NRF52840_UF2_FAMILY,
    ];
    for (slot, word) in block.as_chunks_mut::<4>().0.iter_mut().zip(words) {
        *slot = word.to_le_bytes();
    }
    block[32..32 + payload.len()].copy_from_slice(payload);
    block[UF2_BLOCK_BYTES - 4..].copy_from_slice(&UF2_MAGIC_END.to_le_bytes());
    block
}

fn le_word(block: &[u8], word: usize) -> u32 {
    u32::from_le_bytes(block[word * 4..word * 4 + 4].try_into().unwrap_or_default())
}

/// Append the vault page to the firmware UF2, renumbering every block so the bootloader counts
/// the combined image as one transfer.
fn merge_uf2(firmware: &[u8], page_address: u32, page: &[u8]) -> Result<Vec<u8>, Error> {
    if firmware.is_empty() || !firmware.len().is_multiple_of(UF2_BLOCK_BYTES) {
        return Err(Error::Uf2("not a whole number of 512-byte blocks".into()));
    }
    let page_end = u64::from(page_address) + page.len() as u64;
    let mut payloads = Vec::new();
    for block in firmware.as_chunks::<UF2_BLOCK_BYTES>().0 {
        if le_word(block, 0) != UF2_MAGIC_START0
            || le_word(block, 1) != UF2_MAGIC_START1
            || le_word(block, 127) != UF2_MAGIC_END
        {
            return Err(Error::Uf2("bad block magic".into()));
        }
        if le_word(block, 7) != NRF52840_UF2_FAMILY {
            return Err(Error::Uf2("not an nRF52840 UF2".into()));
        }
        let address = le_word(block, 3);
        let size = le_word(block, 4) as usize;
        if size > 476 {
            return Err(Error::Uf2("oversized block payload".into()));
        }
        if u64::from(address) < page_end
            && u64::from(address) + size as u64 > u64::from(page_address)
        {
            return Err(Error::Uf2(format!(
                "firmware overlaps the Remote Control identity page at {page_address:#x}; rebuild it from this branch"
            )));
        }
        payloads.push((address, block[32..32 + size].to_vec()));
    }
    for (offset, chunk) in page.chunks(UF2_PAYLOAD_BYTES).enumerate() {
        payloads.push((
            page_address + (offset * UF2_PAYLOAD_BYTES) as u32,
            chunk.to_vec(),
        ));
    }
    let total = payloads.len() as u32;
    let mut merged = Vec::with_capacity(payloads.len() * UF2_BLOCK_BYTES);
    for (index, (address, payload)) in payloads.iter().enumerate() {
        merged.extend_from_slice(&uf2_block(*address, payload, index as u32, total));
    }
    Ok(merged)
}

fn lock_state(state_dir: &Path) -> Result<std::fs::File, Error> {
    std::fs::create_dir_all(state_dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(state_dir, std::fs::Permissions::from_mode(0o700))?;
    }
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(state_dir.join("controller.lock"))?;
    lock.try_lock().map_err(Error::Lock)?;
    Ok(lock)
}

fn provision(options: &Options, board: &str, firmware: &Path, out: &Path) -> Result<(), Error> {
    let address = remote_control_identity_address(board)?;
    let (secrets, _) = RemoteControlIdentityDirectory::new(options.state_dir.join("identity"))
        .load_or_generate()?
        .into_parts();
    let identities = secrets.identities();
    let controller = identities.controller().public_keys();
    let target_secret = generate_identity_secret();
    let target_public = PrivateIdentityMaterial::from_bytes(*target_secret).public();
    let page = encode_remote_control_vault_page(&target_secret, controller)
        .map_err(|error| Error::Uf2(format!("vault page: {error:?}")))?;
    let merged = merge_uf2(&std::fs::read(firmware)?, address, &page)?;
    write_private(out, &merged)?;
    let record = json!({
        "board": board,
        "target_public_key": hex::encode(target_public.as_bytes()),
    });
    write_private(
        &device_record(&options.state_dir, &options.device),
        record.to_string().as_bytes(),
    )?;
    println!(
        "{}",
        json!({
            "event": "provisioned",
            "device": options.device,
            "uf2": out,
            "target_hash": hex::encode(target_public.identity_hash().as_bytes()),
            "controller_hash": hex::encode(identities.controller().identity_hash().as_bytes()),
            "note": "copy the UF2 to the board's bootloader drive, then delete it: it holds the board's private key",
        })
    );
    Ok(())
}

fn load_target(options: &Options) -> Result<PublicIdentityMaterial, Error> {
    let path = device_record(&options.state_dir, &options.device);
    let text = std::fs::read_to_string(&path)
        .map_err(|_| Error::NotProvisioned(options.device.clone()))?;
    let record: serde_json::Value =
        serde_json::from_str(&text).map_err(|error| Error::Record(error.to_string()))?;
    let key = record["target_public_key"]
        .as_str()
        .ok_or_else(|| Error::Record("missing target_public_key".into()))?;
    public_identity(key).map_err(|error| Error::Record(error.to_string()))
}

fn lora_profile(
    region: &str,
    frequency_hz: u32,
    bandwidth_khz: &str,
    spreading_factor: u8,
    coding_rate: u8,
    tx_power_dbm: i8,
    preamble: u16,
) -> Result<RemoteControlLoRaProfile, Error> {
    let label = if region.eq_ignore_ascii_case("custom") {
        "Custom"
    } else {
        RegulatoryRegion::ALL
            .into_iter()
            .map(RegulatoryRegion::label)
            .find(|label| label.eq_ignore_ascii_case(region))
            .ok_or_else(|| {
                let known = RegulatoryRegion::ALL
                    .into_iter()
                    .map(RegulatoryRegion::label)
                    .chain(["Custom"])
                    .collect::<Vec<_>>()
                    .join(", ");
                Error::Region(region.to_owned(), known)
            })?
    };
    RemoteControlLoRaProfile::parse(&format!(
        "LoRa,{label},{frequency_hz},{spreading_factor},{bandwidth_khz},{coding_rate},{tx_power_dbm},{preamble}"
    ))
    .ok_or(Error::Profile)
}

async fn control(options: Options) -> Result<(), Error> {
    let _lock = lock_state(&options.state_dir)?;
    if let Command::Provision {
        board,
        firmware,
        out,
    } = &options.command
    {
        return provision(&options, board, firmware, out);
    }
    if let Command::OwnerKey = &options.command {
        let (secrets, _) = RemoteControlIdentityDirectory::new(options.state_dir.join("identity"))
            .load_or_generate()?
            .into_parts();
        let identities = secrets.identities();
        println!(
            "{}",
            json!({
                "event":"owner_key",
                "public_key":hex::encode(identities.controller().public_keys().public_key_bytes()),
            })
        );
        return Ok(());
    }
    if let Command::BoardKey = &options.command {
        let key = load_target(&options)?;
        println!(
            "{}",
            json!({"event":"board_key","device":options.device,"public_key":hex::encode(key.as_bytes())})
        );
        return Ok(());
    }
    let new_name = match &options.command {
        Command::Name { set: Some(name) } => Some(
            personal_rns::remote_control::RemoteControlNodeName::new(name).ok_or_else(|| {
                Error::Record("a name is 1 to 32 bytes, without leading or trailing spaces".into())
            })?,
        ),
        _ => None,
    };
    let authorize = match &options.command {
        Command::Authorize { public_key } => Some(
            hex::decode(public_key.trim())
                .ok()
                .and_then(|bytes| {
                    personal_rns::remote_control::parse_controller_public_keys(&bytes)
                })
                .ok_or_else(|| Error::Record("--public-key must be 128 hex digits".into()))?,
        ),
        _ => None,
    };
    // Validate local arguments before touching USB.
    let lora = match &options.command {
        Command::Lora {
            region,
            frequency_hz,
            bandwidth_khz,
            spreading_factor,
            coding_rate,
            tx_power_dbm,
            preamble,
        } => Some(lora_profile(
            region,
            *frequency_hz,
            bandwidth_khz,
            *spreading_factor,
            *coding_rate,
            *tx_power_dbm,
            *preamble,
        )?),
        _ => None,
    };
    let target_key = load_target(&options)?;
    let (secrets, _) = RemoteControlIdentityDirectory::new(options.state_dir.join("identity"))
        .load_or_generate()?
        .into_parts();
    let target = RemoteControlTargetIdentity::new(target_key.public_keys());
    let endpoint = target.endpoint().destination_hash();
    let target_hash = target.identity_hash();
    let grant = RemoteControlTargetAccess::new(
        target,
        RemoteControlControllerAuthority::Administrator,
        RemoteControlRequestSet::all(),
    )
    .map_err(Error::Grant)?;
    let node = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        remote_control: RemoteControlService::new(
            secrets,
            RemoteControlInitialControllerGrants::Nobody,
            RemoteControlSelfAnnouncement::Unavailable,
        )
        .into(),
        pre_configured_destinations: [] as [PreConfiguredDestination<'static>; 0],
        app_state: personal_rns::runtime::NoRemoteControlHostControls,
        storage: GrowableHeap,
        request_endpoints: request_endpoints![],
        interfaces: ManuallyAttached,
        persistence: NoPersistence,
        on_event: |_event, _state| {},
    });
    let handle = node.handle();
    handle.attach(AutoUsb::default());
    let stage = std::cell::Cell::new("target provisioning");
    let operation = async {
        handle
            .set_remote_control_target_access(grant)
            .await
            .map_err(Error::Provision)?;
        // USB Auto enumerates and handshakes asynchronously; keep asking for the path until the
        // board answers through it.
        stage.set("finding the board over USB");
        while handle.request_path(endpoint).await.is_err() {
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        stage.set("authenticated connection");
        let connection = handle
            .connect_remote_control_target(target_hash)
            .await
            .map_err(Error::Connect)?;
        stage.set("remote operation");
        match &options.command {
            Command::Provision { .. } => {}
            Command::Status => {
                let (build, _) = connection
                    .describe_build()
                    .await
                    .map_err(Error::Operation)?;
                println!("{}", json!({"event":"build","version":build.as_str()}));
                let mut page = RemoteControlInterfacePage::First;
                let mut completion = Err(Error::Pagination);
                for _ in 0..256 {
                    let (inventory, _) = connection
                        .inventory_interfaces_page(page)
                        .await
                        .map_err(Error::Operation)?;
                    for entry in inventory.entries() {
                        let config = match connection
                            .inventory_interface_config(entry.id)
                            .await
                            .map_err(Error::Operation)?
                            .0
                        {
                            RemoteControlInterfaceConfigOutcome::Card(card) => {
                                Some(card.config.as_str().to_owned())
                            }
                            RemoteControlInterfaceConfigOutcome::UnknownInterface => None,
                        };
                        println!(
                            "{}",
                            json!({
                                "event":"interface","id":hex::encode(entry.id.as_bytes()),
                                "kind":format!("{:?}",entry.kind),"enabled":entry.enabled,
                                "connection":format!("{:?}",entry.connection),
                                "tx_bytes":entry.tx_bytes,"rx_bytes":entry.rx_bytes,
                                "config":config,
                            })
                        );
                    }
                    match inventory.continuation() {
                        RemoteControlInterfaceContinuation::Complete => {
                            completion = Ok(());
                            break;
                        }
                        RemoteControlInterfaceContinuation::More(cursor) => {
                            page = RemoteControlInterfacePage::After(cursor)
                        }
                    }
                }
                completion?;
            }
            Command::Lora { .. } => {
                let profile = lora.ok_or(Error::Profile)?;
                let lora_id = lora_interface(&connection).await?;
                let (outcome, _) = connection
                    .set_interface_lora_profile(lora_id, profile)
                    .await
                    .map_err(Error::Operation)?;
                println!(
                    "{}",
                    json!({"event":"lora","interface":hex::encode(lora_id.as_bytes()),"outcome":format!("{outcome:?}")})
                );
            }
            Command::Interface { id, power } => {
                let power = match power {
                    Power::On => RemoteControlInterfacePower::On,
                    Power::Off => RemoteControlInterfacePower::Off,
                };
                let (outcome, _) = connection
                    .set_interface_power(*id, power)
                    .await
                    .map_err(Error::Operation)?;
                println!(
                    "{}",
                    json!({"event":"interface_power","outcome":format!("{outcome:?}")})
                );
            }
            Command::System { power } => {
                let power = match power {
                    SystemPower::Awake => RemoteControlSystemPower::Awake,
                    SystemPower::Asleep => RemoteControlSystemPower::Asleep,
                };
                let (outcome, _) = connection
                    .set_system_power(power)
                    .await
                    .map_err(Error::Operation)?;
                println!(
                    "{}",
                    json!({"event":"system_power","outcome":format!("{outcome:?}")})
                );
            }
            Command::Announce => {
                connection.announce_self().await.map_err(Error::Operation)?;
                println!("{}", json!({"event":"announced"}));
            }
            Command::Authorize { .. } => {
                let controller = authorize.ok_or(Error::Profile)?;
                let (outcome, _) = connection
                    .authorize_controller(controller, RemoteControlRequestSet::all_operator())
                    .await
                    .map_err(Error::Operation)?;
                println!(
                    "{}",
                    json!({"event":"authorize","outcome":format!("{outcome:?}")})
                );
            }
            Command::BoardKey | Command::OwnerKey => {}
            Command::Name { .. } => {
                if let Some(name) = new_name {
                    let (outcome, _) = connection
                        .set_node_name(name)
                        .await
                        .map_err(Error::Operation)?;
                    println!(
                        "{}",
                        json!({"event":"set_name","name":name.as_str(),"outcome":format!("{outcome:?}")})
                    );
                } else {
                    let (name, _) = connection
                        .describe_node_name()
                        .await
                        .map_err(Error::Operation)?;
                    println!("{}", json!({"event":"name","name":name.as_str()}));
                }
            }
        }
        connection.close();
        Ok(())
    };
    tokio::select! {
        result = tokio::time::timeout(OPERATION_TIMEOUT, operation) => result.map_err(|_| Error::Timeout(stage.get()))?,
        result = node.run() => Err(Error::Node(result)),
    }
}

/// The LoRa interface's ID changes with its profile, so look it up rather than asking for it.
async fn lora_interface(
    connection: &personal_rns::runtime::RemoteControlTargetHandle<'_>,
) -> Result<InterfaceId, Error> {
    let mut page = RemoteControlInterfacePage::First;
    for _ in 0..256 {
        let (inventory, _) = connection
            .inventory_interfaces_page(page)
            .await
            .map_err(Error::Operation)?;
        if let Some(entry) = inventory
            .entries()
            .iter()
            .find(|entry| format!("{:?}", entry.kind).contains("LoRa"))
        {
            return Ok(entry.id);
        }
        match inventory.continuation() {
            RemoteControlInterfaceContinuation::Complete => break,
            RemoteControlInterfaceContinuation::More(cursor) => {
                page = RemoteControlInterfacePage::After(cursor)
            }
        }
    }
    Err(Error::Record("the board reports no LoRa interface".into()))
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> std::process::ExitCode {
    match control(Options::parse()).await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("usb_config_failed: {error}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn firmware(blocks: &[(u32, u8)]) -> Vec<u8> {
        let total = blocks.len() as u32;
        blocks
            .iter()
            .enumerate()
            .flat_map(|(index, (address, fill))| {
                uf2_block(*address, &[*fill; UF2_PAYLOAD_BYTES], index as u32, total)
            })
            .collect()
    }

    #[test]
    fn merge_appends_the_page_and_renumbers_every_block() {
        let page = [0xA5u8; 4096];
        let merged = merge_uf2(&firmware(&[(0x26000, 1), (0x26100, 2)]), 0xE2000, &page).unwrap();
        let blocks: Vec<_> = merged.chunks_exact(UF2_BLOCK_BYTES).collect();
        assert_eq!(blocks.len(), 2 + 16);
        for (index, block) in blocks.iter().enumerate() {
            assert_eq!(le_word(block, 5), index as u32);
            assert_eq!(le_word(block, 6), 18);
            assert_eq!(le_word(block, 7), NRF52840_UF2_FAMILY);
        }
        assert_eq!(le_word(blocks[2], 3), 0xE2000);
        assert_eq!(le_word(blocks[17], 3), 0xE2F00);
        assert_eq!(blocks[17][32], 0xA5);
        assert_eq!(blocks[1][32], 2);
    }

    #[test]
    fn merge_refuses_firmware_that_reaches_the_identity_page() {
        let error = merge_uf2(&firmware(&[(0xE2000, 1)]), 0xE2000, &[0; 4096]).unwrap_err();
        assert!(matches!(error, Error::Uf2(_)));
    }

    #[test]
    fn lora_profile_accepts_rnode_style_arguments() {
        assert!(lora_profile("us915", 918_300_000, "250", 10, 5, 22, 18).is_ok());
        assert!(matches!(
            lora_profile("mars", 918_300_000, "250", 10, 5, 22, 18),
            Err(Error::Region(..))
        ));
    }
}
