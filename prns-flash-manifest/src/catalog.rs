use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    AfterResetStrategy, BeforeResetStrategy, BoardId, ChipFamily, ImmutableArtifactPath,
    PreparationProfile, ProvisioningFormat, SoftdeviceIdentity, Uf2BoardIdPrefix, Uf2MountLabel,
    UsbVidPid, ValidatedNrfSerialDfuSerialTransport, CONFIG_OFFSET, CONFIG_PASSWORD_MAX_BYTES,
    CONFIG_SIZE, CONFIG_SSID_MAX_BYTES, CONFIG_VERSION,
};

const CATALOG_JSON: &str = include_str!("../../release/flash/boards.json");
const SHIPPING_BOARD_SLUGS: [&str; 5] = [
    "heltec-v4",
    "heltec-v4-r8",
    "t-beam-supreme",
    "xiao-esp32-c6",
    "t-echo",
];

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoardCatalog {
    #[serde(rename = "schema")]
    pub schema_version: u32,
    pub boards: Vec<BoardCatalogEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoardCatalogEntry {
    pub availability: BoardAvailability,
    pub slug: String,
    pub display_name: String,
    pub silicon: String,
    pub interfaces: Vec<String>,
    pub icon: String,
    pub transport: Transport,
    pub expected_chip: Option<String>,
    pub flash_size: Option<u32>,
    pub preparation_profile: String,
    pub provisioning: Option<ProvisioningDescriptor>,
    pub build: BoardBuild,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BoardAvailability {
    Shipping,
    Qualification,
}

impl BoardCatalogEntry {
    pub fn supports_provisioning(&self) -> bool {
        self.provisioning.is_some()
    }

    pub fn supports_tcp_client_provisioning(&self) -> bool {
        self.provisioning
            .as_ref()
            .and_then(|slot| slot.tcp_client.as_ref())
            .is_some()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Transport {
    EspSerial,
    Uf2MassStorage,
    NrfSerialDfu,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProvisioningDescriptor {
    pub format: String,
    pub version: u8,
    pub offset: u32,
    pub size: u32,
    pub ssid_max_bytes: usize,
    pub password_max_bytes: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tcp_client: Option<TcpClientProvisioningDescriptor>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TcpClientProvisioningDescriptor {
    pub target_format: String,
    pub max_clients: u8,
    pub default_port: u16,
    pub hostname_max_bytes: usize,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum BoardBuild {
    Esp(EspBuild),
    Uf2(Uf2Build),
    NrfSerialDfu(NrfSerialDfuBuild),
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NrfSerialDfuBuild {
    pub package: String,
    pub binary: String,
    pub rust_target: String,
    pub cargo_feature: String,
    pub target_directory: String,
    pub application_filename: String,
    pub init_packet_filename: String,
    pub serial: NrfSerialDfuSerialTransport,
    pub compatibility: NrfSerialDfuCompatibility,
    pub recovery: NrfSerialDfuRecoveryBuild,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NrfSerialDfuSerialTransport {
    pub touch_application_and_bootloader: NrfSerialDfuTouchApplicationAndBootloader,
    pub managed_application: NrfSerialDfuControlApplication,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NrfSerialDfuTouchApplicationAndBootloader {
    pub usb: UsbVendorProductId,
    pub touch_baud_rate: u32,
    pub transfer_baud_rate: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NrfSerialDfuControlApplication {
    pub usb: UsbVendorProductId,
    pub manufacturer: String,
    pub product: String,
    pub serial_number: String,
    pub interface_number: u8,
    pub request: String,
    pub value: String,
    pub index: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UsbVendorProductId {
    pub vendor_id: String,
    pub product_id: String,
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum NrfSerialDfuSerialTransportError {
    #[error(
        "touch application and bootloader USB vendor/product identity is not canonical nonzero hexadecimal"
    )]
    InvalidTouchApplicationAndBootloaderUsb,
    #[error(
        "managed application USB vendor/product identity is not canonical nonzero hexadecimal"
    )]
    InvalidManagedApplicationUsb,
    #[error("managed application and serial DFU USB identities must differ")]
    IndistinguishableUsbModes,
    #[error("application bootloader-touch baud rate must be nonzero")]
    ZeroTouchBaudRate,
    #[error("bootloader DFU baud rate must be nonzero")]
    ZeroTransferBaudRate,
    #[error("managed application bootloader-entry request is not canonical hexadecimal")]
    InvalidManagedApplicationRequest,
    #[error("managed application bootloader-entry control value is not canonical hexadecimal")]
    InvalidManagedApplicationValue,
    #[error("managed application bootloader-entry control index is not canonical hexadecimal")]
    InvalidManagedApplicationIndex,
    #[error("managed application bootloader-entry control contract differs from USB Auto")]
    ManagedApplicationContractMismatch,
    #[error("managed application USB strings must be nonempty printable ASCII")]
    InvalidManagedApplicationStrings,
}

impl NrfSerialDfuSerialTransport {
    pub fn into_validated(
        self,
    ) -> Result<ValidatedNrfSerialDfuSerialTransport, NrfSerialDfuSerialTransportError> {
        let touch_application_and_bootloader_usb =
            parse_usb_vendor_product_id(&self.touch_application_and_bootloader.usb)
                .ok_or(NrfSerialDfuSerialTransportError::InvalidTouchApplicationAndBootloaderUsb)?;
        let managed_application_usb = parse_usb_vendor_product_id(&self.managed_application.usb)
            .ok_or(NrfSerialDfuSerialTransportError::InvalidManagedApplicationUsb)?;
        if touch_application_and_bootloader_usb == managed_application_usb {
            return Err(NrfSerialDfuSerialTransportError::IndistinguishableUsbModes);
        }
        if self.touch_application_and_bootloader.touch_baud_rate == 0 {
            return Err(NrfSerialDfuSerialTransportError::ZeroTouchBaudRate);
        }
        if self.touch_application_and_bootloader.transfer_baud_rate == 0 {
            return Err(NrfSerialDfuSerialTransportError::ZeroTransferBaudRate);
        }
        if !valid_usb_string(&self.managed_application.manufacturer)
            || !valid_usb_string(&self.managed_application.product)
            || !valid_usb_string(&self.managed_application.serial_number)
        {
            return Err(NrfSerialDfuSerialTransportError::InvalidManagedApplicationStrings);
        }
        let request = parse_hex_u8(&self.managed_application.request)
            .ok_or(NrfSerialDfuSerialTransportError::InvalidManagedApplicationRequest)?;
        let value = parse_hex_u16(&self.managed_application.value)
            .ok_or(NrfSerialDfuSerialTransportError::InvalidManagedApplicationValue)?;
        let index = parse_hex_u16(&self.managed_application.index)
            .ok_or(NrfSerialDfuSerialTransportError::InvalidManagedApplicationIndex)?;
        use prns_core::interfaces::usb_auto::{
            BOOTLOADER_ENTRY_CONTROL_INDEX, BOOTLOADER_ENTRY_CONTROL_REQUEST,
            BOOTLOADER_ENTRY_CONTROL_VALUE, WEBUSB_PRODUCT_ID, WEBUSB_VENDOR_ID,
        };
        if managed_application_usb.vendor_id != WEBUSB_VENDOR_ID
            || managed_application_usb.product_id != WEBUSB_PRODUCT_ID
            || request != BOOTLOADER_ENTRY_CONTROL_REQUEST
            || value != BOOTLOADER_ENTRY_CONTROL_VALUE
            || index != BOOTLOADER_ENTRY_CONTROL_INDEX
        {
            return Err(NrfSerialDfuSerialTransportError::ManagedApplicationContractMismatch);
        }
        Ok(ValidatedNrfSerialDfuSerialTransport {
            touch_application_and_bootloader_usb,
            touch_baud_rate: self.touch_application_and_bootloader.touch_baud_rate,
            managed_application_usb,
            managed_application_manufacturer: self.managed_application.manufacturer,
            managed_application_product: self.managed_application.product,
            managed_application_serial_number: self.managed_application.serial_number,
            managed_application_interface_number: self.managed_application.interface_number,
            managed_application_request: request,
            managed_application_value: value,
            managed_application_index: index,
            transfer_baud_rate: self.touch_application_and_bootloader.transfer_baud_rate,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NrfSerialDfuCompatibility {
    pub softdevice_family: String,
    pub softdevice_version: String,
    pub fwid: String,
    pub device_type: String,
    pub device_revision: u16,
    pub application_version: NrfDfuApplicationVersion,
    pub application_base: String,
    pub application_end_exclusive: String,
    pub bank_layout: NrfDfuBankLayout,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NrfDfuApplicationVersion {
    NotEnforced,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NrfDfuBankLayout {
    Single,
    Dual,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NrfSerialDfuRecoveryBuild {
    pub mount_label: String,
    pub board_id_prefix: String,
    pub family_id: String,
    pub filename: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EspBuild {
    pub chip: String,
    pub rust_target: String,
    pub partition_table: String,
    pub package: String,
    pub binary: String,
    pub flash_size_label: String,
    pub flash_mode: String,
    pub flash_frequency: String,
    pub before_reset: String,
    pub after_reset: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Uf2Build {
    pub package: String,
    pub rust_target: String,
    pub mount_label: String,
    pub board_id_prefix: String,
    pub variants: Vec<Uf2BuildVariant>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Uf2BuildVariant {
    pub softdevice_family: String,
    pub softdevice_version: String,
    pub fwid: String,
    pub application_base: String,
    pub family_id: String,
    pub cargo_feature: String,
    pub target_directory: String,
    pub filename: String,
}

#[derive(Debug, Error)]
pub enum CatalogError {
    #[error("board catalog is invalid: {0}")]
    Json(#[from] serde_json::Error),
    #[error("unsupported board catalog schema {0}")]
    Schema(u32),
    #[error("duplicate board slug {0:?}")]
    DuplicateSlug(String),
    #[error("UF2 board-id prefixes overlap between {first:?} and {second:?}")]
    OverlappingUf2BoardIdPrefixes { first: String, second: String },
    #[error("board {board:?}: {message}")]
    InvalidBoard { board: String, message: String },
}

impl BoardCatalog {
    pub fn from_json(bytes: &[u8]) -> Result<Self, CatalogError> {
        let catalog: Self = serde_json::from_slice(bytes)?;
        catalog.validate()?;
        Ok(catalog)
    }

    pub fn validate(&self) -> Result<(), CatalogError> {
        if self.schema_version != 2 {
            return Err(CatalogError::Schema(self.schema_version));
        }
        let mut slugs = std::collections::BTreeSet::new();
        for board in &self.boards {
            if !slugs.insert(board.slug.as_str()) {
                return Err(CatalogError::DuplicateSlug(board.slug.clone()));
            }
            validate_slug(board)?;
            validate_transport(board)?;
            validate_provisioning(board)?;
        }
        validate_uf2_board_id_prefixes(&self.boards)?;
        let shipping = self
            .shipping_boards()
            .map(|board| board.slug.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        let expected = SHIPPING_BOARD_SLUGS
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>();
        if shipping != expected {
            return Err(CatalogError::InvalidBoard {
                board: "catalog".to_string(),
                message: format!("shipping board set must be exactly {expected:?}"),
            });
        }
        Ok(())
    }

    pub fn board(&self, slug: &str) -> Option<&BoardCatalogEntry> {
        self.boards.iter().find(|board| board.slug == slug)
    }

    pub fn shipping_boards(&self) -> impl Iterator<Item = &BoardCatalogEntry> {
        self.boards
            .iter()
            .filter(|board| board.availability == BoardAvailability::Shipping)
    }
}

pub fn board_catalog() -> Result<BoardCatalog, CatalogError> {
    BoardCatalog::from_json(CATALOG_JSON.as_bytes())
}

fn validate_slug(board: &BoardCatalogEntry) -> Result<(), CatalogError> {
    if BoardId::parse(board.slug.clone()).is_err() {
        return Err(invalid(
            board,
            "slug must use lowercase ASCII, digits, and hyphens",
        ));
    }
    if board.display_name.trim().is_empty()
        || board.silicon.trim().is_empty()
        || board.preparation_profile.trim().is_empty()
        || board.interfaces.is_empty()
        || board.interfaces.iter().any(|value| value.trim().is_empty())
        || board
            .interfaces
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len()
            != board.interfaces.len()
    {
        return Err(invalid(
            board,
            "display name, silicon, preparation profile, and unique interfaces are required",
        ));
    }
    Ok(())
}

fn validate_transport(board: &BoardCatalogEntry) -> Result<(), CatalogError> {
    match (&board.transport, &board.build) {
        (Transport::EspSerial, BoardBuild::Esp(build)) => {
            let expected_flash_size_label = match board.flash_size {
                Some(4_194_304) => "4mb",
                Some(8_388_608) => "8mb",
                Some(16_777_216) => "16mb",
                _ => {
                    return Err(invalid(
                        board,
                        "ESP chip/build/flash/reset parameters are unsupported or disagree",
                    ));
                }
            };
            if board.expected_chip.as_deref() != Some(build.chip.as_str())
                || ChipFamily::parse(&build.chip).is_err()
                || build.flash_size_label != expected_flash_size_label
                || build.flash_mode != "dio"
                || build.flash_frequency != "40m"
                || BeforeResetStrategy::parse(&build.before_reset).is_err()
                || AfterResetStrategy::parse(&build.after_reset).is_err()
                || PreparationProfile::parse(&board.preparation_profile)
                    != Ok(PreparationProfile::EspUsbBoot)
                || build.package.trim().is_empty()
                || build.binary.trim().is_empty()
                || build.partition_table.contains(['/', '\\'])
                || !build.partition_table.ends_with(".csv")
                || (build.chip == "esp32s3" && build.rust_target != "xtensa-esp32s3-none-elf")
                || (build.chip == "esp32c6" && build.rust_target != "riscv32imac-unknown-none-elf")
            {
                return Err(invalid(
                    board,
                    "ESP chip/build/flash/reset parameters are unsupported or disagree",
                ));
            }
        }
        (Transport::Uf2MassStorage, BoardBuild::Uf2(build)) => {
            if board.expected_chip.is_some()
                || board.flash_size.is_some()
                || PreparationProfile::parse(&board.preparation_profile)
                    != Ok(PreparationProfile::TechoUf2)
                || Uf2MountLabel::parse(build.mount_label.clone()).is_err()
                || Uf2BoardIdPrefix::parse(build.board_id_prefix.clone()).is_err()
                || build.package.trim().is_empty()
                || build.rust_target != "thumbv7em-none-eabihf"
                || !valid_uf2_build_variants(&build.variants)
            {
                return Err(invalid(
                    board,
                    "UF2 chip/flash/preparation/mount fields are unsupported or disagree",
                ));
            }
        }
        (Transport::NrfSerialDfu, BoardBuild::NrfSerialDfu(build)) => {
            if board.expected_chip.is_some()
                || board.flash_size.is_some()
                || PreparationProfile::parse(&board.preparation_profile)
                    != Ok(PreparationProfile::T1000eNrfDfu)
                || !valid_nrf_serial_dfu_build(build)
            {
                return Err(invalid(
                    board,
                    "Nordic serial DFU build and recovery fields are unsupported or disagree",
                ));
            }
        }
        _ => return Err(invalid(board, "transport and build recipe disagree")),
    }
    Ok(())
}

/// Reject nested UF2 Board-ID prefixes so one drive can never identify as multiple boards.
fn validate_uf2_board_id_prefixes(boards: &[BoardCatalogEntry]) -> Result<(), CatalogError> {
    let prefixes = boards
        .iter()
        .filter_map(|board| match &board.build {
            BoardBuild::Uf2(build) => Some((board.slug.as_str(), build.board_id_prefix.as_str())),
            BoardBuild::Esp(_) => None,
            BoardBuild::NrfSerialDfu(build) => {
                Some((board.slug.as_str(), build.recovery.board_id_prefix.as_str()))
            }
        })
        .collect::<Vec<_>>();
    for (index, (slug, prefix)) in prefixes.iter().enumerate() {
        for (other_slug, other_prefix) in &prefixes[index + 1..] {
            if prefix.starts_with(other_prefix) || other_prefix.starts_with(prefix) {
                return Err(CatalogError::OverlappingUf2BoardIdPrefixes {
                    first: (*slug).to_string(),
                    second: (*other_slug).to_string(),
                });
            }
        }
    }
    Ok(())
}

fn parse_hex_u16(value: &str) -> Option<u16> {
    let digits = value.strip_prefix("0x")?;
    (digits.len() == 4
        && digits
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()))
    .then(|| u16::from_str_radix(digits, 16).ok())
    .flatten()
}

fn parse_hex_u8(value: &str) -> Option<u8> {
    let digits = value.strip_prefix("0x")?;
    (digits.len() == 2
        && digits
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()))
    .then(|| u8::from_str_radix(digits, 16).ok())
    .flatten()
}

fn parse_hex_u32(value: &str) -> Option<u32> {
    let digits = value.strip_prefix("0x")?;
    (digits.len() == 8
        && digits
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()))
    .then(|| u32::from_str_radix(digits, 16).ok())
    .flatten()
}

fn valid_uf2_build_variants(variants: &[Uf2BuildVariant]) -> bool {
    let expected = [
        (
            "s140",
            "6.1.1",
            "0x00b6",
            "0x00026000",
            "0xada52840",
            "softdevice-s140-v6",
            "target/s140-v6",
            "t-echo-s140-6.1.1.uf2",
        ),
        (
            "s140",
            "7.3.0",
            "0x0123",
            "0x00027000",
            "0xada52840",
            "softdevice-s140-v7",
            "target/s140-v7",
            "t-echo-s140-7.3.0.uf2",
        ),
    ];
    variants
        .iter()
        .map(|variant| {
            (
                variant.softdevice_family.as_str(),
                variant.softdevice_version.as_str(),
                variant.fwid.as_str(),
                variant.application_base.as_str(),
                variant.family_id.as_str(),
                variant.cargo_feature.as_str(),
                variant.target_directory.as_str(),
                variant.filename.as_str(),
            )
        })
        .eq(expected)
        && variants.iter().all(|variant| {
            parse_hex_u32(&variant.application_base).is_some()
                && parse_hex_u32(&variant.family_id).is_some()
        })
}

fn valid_nrf_serial_dfu_build(build: &NrfSerialDfuBuild) -> bool {
    let compatibility = &build.compatibility;
    let recovery = &build.recovery;
    let application_base = parse_hex_u32(&compatibility.application_base);
    let application_end = parse_hex_u32(&compatibility.application_end_exclusive);
    let application_region_is_valid =
        application_base
            .zip(application_end)
            .is_some_and(|(base, end)| {
                base < end && base % 0x1000 == 0 && end % 0x1000 == 0 && end <= 0x0010_0000
            });
    valid_cargo_name(&build.package)
        && valid_cargo_name(&build.binary)
        && build.rust_target == "thumbv7em-none-eabihf"
        && valid_cargo_name(&build.cargo_feature)
        && ImmutableArtifactPath::parse(build.target_directory.clone()).is_ok()
        && valid_artifact_filename(&build.application_filename, ".bin")
        && valid_artifact_filename(&build.init_packet_filename, ".dat")
        && build.application_filename != build.init_packet_filename
        && build.serial.clone().into_validated().is_ok()
        && SoftdeviceIdentity::parse(
            &compatibility.softdevice_family,
            compatibility.softdevice_version.clone(),
        )
        .is_ok()
        && parse_hex_u16(&compatibility.fwid).is_some_and(|fwid| fwid != 0xfffe)
        && parse_hex_u16(&compatibility.device_type).is_some()
        && compatibility.device_revision != 0
        && application_region_is_valid
        && Uf2MountLabel::parse(recovery.mount_label.clone()).is_ok()
        && Uf2BoardIdPrefix::parse(recovery.board_id_prefix.clone()).is_ok()
        && parse_hex_u32(&recovery.family_id).is_some()
        && valid_artifact_filename(&recovery.filename, ".uf2")
        && recovery.filename != build.application_filename
        && recovery.filename != build.init_packet_filename
}

fn parse_usb_vendor_product_id(identity: &UsbVendorProductId) -> Option<UsbVidPid> {
    let vendor_id = parse_hex_u16(&identity.vendor_id).filter(|value| *value != 0)?;
    let product_id = parse_hex_u16(&identity.product_id).filter(|value| *value != 0)?;
    Some(UsbVidPid {
        vendor_id,
        product_id,
    })
}

fn valid_usb_string(value: &str) -> bool {
    !value.is_empty()
        && value.trim() == value
        && value
            .bytes()
            .all(|byte| byte.is_ascii_graphic() || byte == b' ')
}

fn valid_cargo_name(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_alphanumeric())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn valid_artifact_filename(value: &str, extension: &str) -> bool {
    !value.is_empty()
        && value.ends_with(extension)
        && !value.contains(['/', '\\'])
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
}

fn validate_provisioning(board: &BoardCatalogEntry) -> Result<(), CatalogError> {
    let Some(slot) = &board.provisioning else {
        return Ok(());
    };
    if board.transport != Transport::EspSerial {
        return Err(invalid(
            board,
            "only ESP boards can have a provisioning slot",
        ));
    }
    if ProvisioningFormat::parse(&slot.format) != Ok(ProvisioningFormat::Hspcfg1)
        || slot.version != CONFIG_VERSION
        || slot.offset != CONFIG_OFFSET
        || slot.size != CONFIG_SIZE as u32
        || slot.ssid_max_bytes != CONFIG_SSID_MAX_BYTES
        || slot.password_max_bytes != CONFIG_PASSWORD_MAX_BYTES
    {
        return Err(invalid(
            board,
            "provisioning descriptor disagrees with the wire contract",
        ));
    }
    if let Some(tcp_client) = &slot.tcp_client {
        if tcp_client.target_format != "ipv4-or-dns"
            || tcp_client.max_clients != 1
            || tcp_client.default_port == 0
            || tcp_client.hostname_max_bytes != crate::CONFIG_TCP_CLIENT_HOSTNAME_MAX_BYTES
        {
            return Err(invalid(
                board,
                "TCP client provisioning must allow one IPv4 or DNS target",
            ));
        }
        let BoardBuild::Esp(build) = &board.build else {
            return Err(invalid(
                board,
                "TCP client provisioning requires an ESP build",
            ));
        };
        if build.chip != "esp32s3" || !board.interfaces.iter().any(|value| value == "TCP Client") {
            return Err(invalid(
                board,
                "TCP client provisioning requires a capable ESP32-S3 target",
            ));
        }
    }
    Ok(())
}

fn invalid(board: &BoardCatalogEntry, message: &str) -> CatalogError {
    CatalogError::InvalidBoard {
        board: board.slug.clone(),
        message: message.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_catalog_has_all_shipping_boards() -> Result<(), CatalogError> {
        let catalog = board_catalog()?;
        let slugs = catalog
            .shipping_boards()
            .map(|board| board.slug.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            slugs,
            [
                "heltec-v4",
                "heltec-v4-r8",
                "t-beam-supreme",
                "xiao-esp32-c6",
                "t-echo"
            ]
        );
        Ok(())
    }

    #[test]
    fn qualification_boards_are_absent_from_the_shipping_view() -> Result<(), CatalogError> {
        let mut catalog = board_catalog()?;
        catalog.boards[0].availability = BoardAvailability::Qualification;
        let shipping = catalog
            .shipping_boards()
            .map(|board| board.slug.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            shipping,
            ["heltec-v4-r8", "t-beam-supreme", "xiao-esp32-c6", "t-echo"]
        );
        assert!(catalog.board("heltec-v4").is_some());
        Ok(())
    }

    #[test]
    fn embedded_catalog_has_exact_physical_flash_contracts() -> Result<(), CatalogError> {
        let catalog = board_catalog()?;
        let contracts = catalog
            .boards
            .iter()
            .map(|board| {
                let build = match &board.build {
                    BoardBuild::Esp(build) => Some((
                        build.partition_table.as_str(),
                        build.flash_size_label.as_str(),
                    )),
                    BoardBuild::Uf2(_) => None,
                    BoardBuild::NrfSerialDfu(_) => None,
                };
                (board.slug.as_str(), board.flash_size, build)
            })
            .collect::<Vec<_>>();
        assert_eq!(
            contracts,
            [
                (
                    "heltec-v4",
                    Some(16_777_216),
                    Some(("partitions-hopspot-16mb.csv", "16mb"))
                ),
                (
                    "heltec-v4-r8",
                    Some(16_777_216),
                    Some(("partitions-hopspot-16mb.csv", "16mb"))
                ),
                (
                    "t-beam-supreme",
                    Some(8_388_608),
                    Some(("partitions-hopspot-8mb.csv", "8mb"))
                ),
                (
                    "xiao-esp32-c6",
                    Some(4_194_304),
                    Some(("partitions-hopspot-4mb.csv", "4mb"))
                ),
                ("t-echo", None, None),
                ("t1000-e", None, None),
            ]
        );
        Ok(())
    }

    #[test]
    fn t1000e_qualification_contract_matches_the_recovery_bootloader() -> Result<(), CatalogError> {
        let catalog = board_catalog()?;
        let board = catalog
            .board("t1000-e")
            .ok_or_else(|| CatalogError::InvalidBoard {
                board: "t1000-e".to_string(),
                message: "missing qualification target".to_string(),
            })?;
        let BoardBuild::NrfSerialDfu(build) = &board.build else {
            return Err(invalid(board, "expected Nordic serial DFU build"));
        };
        assert_eq!(board.availability, BoardAvailability::Qualification);
        assert_eq!(build.package, "t-echo");
        assert_eq!(build.binary, "t1000e");
        assert_eq!(build.cargo_feature, "board-t1000e");
        assert_eq!(
            build.serial.touch_application_and_bootloader.usb.vendor_id,
            "0x2886"
        );
        assert_eq!(
            build.serial.touch_application_and_bootloader.usb.product_id,
            "0x0057"
        );
        assert_eq!(
            build
                .serial
                .touch_application_and_bootloader
                .touch_baud_rate,
            1200
        );
        assert_eq!(build.serial.managed_application.usb.vendor_id, "0x1209");
        assert_eq!(build.serial.managed_application.usb.product_id, "0x0001");
        assert_eq!(
            build.serial.managed_application.manufacturer,
            "Stay Personal"
        );
        assert_eq!(
            build.serial.managed_application.product,
            "Personal Hopspot (T1000-E)"
        );
        assert_eq!(
            build.serial.managed_application.serial_number,
            "PERSONAL-RNS-T1000E-HOP"
        );
        assert_eq!(build.serial.managed_application.interface_number, 0);
        assert_eq!(build.serial.managed_application.request, "0x50");
        assert_eq!(build.serial.managed_application.value, "0x5052");
        assert_eq!(build.serial.managed_application.index, "0x4e53");
        assert_eq!(
            build
                .serial
                .touch_application_and_bootloader
                .transfer_baud_rate,
            115200
        );
        assert_eq!(build.compatibility.softdevice_family, "s140");
        assert_eq!(build.compatibility.softdevice_version, "7.3.0");
        assert_eq!(build.compatibility.fwid, "0x0123");
        assert_eq!(build.compatibility.device_type, "0x0052");
        assert_eq!(build.compatibility.device_revision, 52840);
        assert_eq!(build.compatibility.application_base, "0x00027000");
        assert_eq!(build.compatibility.application_end_exclusive, "0x000ea000");
        assert_eq!(build.compatibility.bank_layout, NrfDfuBankLayout::Single);
        assert_eq!(build.recovery.mount_label, "T1000-E");
        assert_eq!(build.recovery.board_id_prefix, "nrf52840-t1000-e-v1");
        assert_eq!(build.recovery.family_id, "0xada52840");
        Ok(())
    }

    #[test]
    fn embedded_catalog_limits_tcp_client_provisioning_to_roomy_wifi_boards(
    ) -> Result<(), CatalogError> {
        let catalog = board_catalog()?;
        let capable = catalog
            .boards
            .iter()
            .filter(|board| board.supports_tcp_client_provisioning())
            .map(|board| board.slug.as_str())
            .collect::<Vec<_>>();
        assert_eq!(capable, ["heltec-v4", "heltec-v4-r8", "t-beam-supreme"]);
        Ok(())
    }

    #[test]
    fn a_shipping_board_cannot_be_removed() -> Result<(), Box<dyn std::error::Error>> {
        let mut value = serde_json::to_value(board_catalog()?)?;
        value["boards"]
            .as_array_mut()
            .ok_or("boards is not an array")?
            .remove(0);
        assert!(matches!(
            BoardCatalog::from_json(&serde_json::to_vec(&value)?),
            Err(CatalogError::InvalidBoard { .. })
        ));
        Ok(())
    }

    #[test]
    fn a_uf2_board_is_not_tied_to_one_bootloader_volume() -> Result<(), Box<dyn std::error::Error>>
    {
        let mut catalog = board_catalog()?;
        let board = catalog
            .boards
            .iter_mut()
            .find(|board| board.transport == Transport::Uf2MassStorage)
            .ok_or("expected a UF2 board")?;
        let BoardBuild::Uf2(build) = &mut board.build else {
            return Err("expected a UF2 build".into());
        };
        build.mount_label = "T114BOOT".to_string();
        build.board_id_prefix = "nrf52840-heltec-t114-v".to_string();
        catalog.validate()?;
        Ok(())
    }

    #[test]
    fn one_uf2_board_id_prefix_may_not_begin_with_another() -> Result<(), Box<dyn std::error::Error>>
    {
        let mut catalog = board_catalog()?;
        let mut second = catalog
            .boards
            .iter()
            .find(|board| board.transport == Transport::Uf2MassStorage)
            .ok_or("expected a UF2 board")?
            .clone();
        second.slug = "nrf52840-second-board".to_string();
        let BoardBuild::Uf2(build) = &mut second.build else {
            return Err("expected a UF2 build".into());
        };
        build.board_id_prefix = format!("{}2", build.board_id_prefix);
        catalog.boards.push(second);
        assert!(matches!(
            catalog.validate(),
            Err(CatalogError::OverlappingUf2BoardIdPrefixes { .. })
        ));
        Ok(())
    }

    #[test]
    fn an_unnormalized_uf2_board_id_prefix_is_rejected() -> Result<(), Box<dyn std::error::Error>> {
        let mut catalog = board_catalog()?;
        let board = catalog
            .boards
            .iter_mut()
            .find(|board| board.transport == Transport::Uf2MassStorage)
            .ok_or("expected a UF2 board")?;
        let BoardBuild::Uf2(build) = &mut board.build else {
            return Err("expected a UF2 build".into());
        };
        build.board_id_prefix = "nRF52840_TEcho_v".to_string();
        assert!(matches!(
            catalog.validate(),
            Err(CatalogError::InvalidBoard { .. })
        ));
        Ok(())
    }

    #[test]
    fn a_malformed_uf2_mount_label_is_rejected() -> Result<(), Box<dyn std::error::Error>> {
        let mut catalog = board_catalog()?;
        let board = catalog
            .boards
            .iter_mut()
            .find(|board| board.transport == Transport::Uf2MassStorage)
            .ok_or("expected a UF2 board")?;
        let BoardBuild::Uf2(build) = &mut board.build else {
            return Err("expected a UF2 build".into());
        };
        build.mount_label = "../TECHOBOOT".to_string();
        assert!(matches!(
            catalog.validate(),
            Err(CatalogError::InvalidBoard { .. })
        ));
        Ok(())
    }

    #[test]
    fn unsupported_reset_strategy_is_rejected() -> Result<(), Box<dyn std::error::Error>> {
        let mut catalog = board_catalog()?;
        let BoardBuild::Esp(build) = &mut catalog.boards[0].build else {
            return Err("expected ESP test board".into());
        };
        build.after_reset = "mystery-reset".to_string();
        assert!(matches!(
            catalog.validate(),
            Err(CatalogError::InvalidBoard { .. })
        ));
        Ok(())
    }
}
