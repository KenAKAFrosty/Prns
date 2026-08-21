use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    AfterResetStrategy, BeforeResetStrategy, BoardId, ChipFamily, ImmutableArtifactPath,
    PreparationProfile, ProvisioningFormat, SoftdeviceIdentity, Uf2BoardIdMatch,
    Uf2BoardIdMatchKind, Uf2MountLabel, UsbVidPid, ValidatedNrfSerialDfuSerialTransport,
    CONFIG_OFFSET, CONFIG_PASSWORD_MAX_BYTES, CONFIG_SIZE, CONFIG_SSID_MAX_BYTES, CONFIG_VERSION,
};

const CATALOG_JSON: &str = include_str!("../../release/flash/boards.json");
const BOARD_CATALOG_SCHEMA: u32 = 4;
const SHIPPING_BOARD_SLUGS: [&str; 8] = [
    "heltec-v4",
    "heltec-v4-r8",
    "t-beam-supreme",
    "xiao-esp32-c6",
    "t-echo",
    "t114",
    "t096",
    "t1000-e",
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
    NrfSerialDfu(Box<NrfSerialDfuBuild>),
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
    pub recovery_bootloader: NrfSerialDfuRecoveryBootloader,
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
pub struct NrfSerialDfuRecoveryBootloader {
    pub usb: UsbVendorProductId,
    pub manufacturer: String,
    pub product: String,
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
    #[error(
        "recovery bootloader USB vendor/product identity is not canonical nonzero hexadecimal"
    )]
    InvalidRecoveryBootloaderUsb,
    #[error("Nordic serial DFU USB identities must be distinct")]
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
    #[error("recovery bootloader USB strings must be nonempty printable ASCII")]
    InvalidRecoveryBootloaderStrings,
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
        let recovery_bootloader_usb = parse_usb_vendor_product_id(&self.recovery_bootloader.usb)
            .ok_or(NrfSerialDfuSerialTransportError::InvalidRecoveryBootloaderUsb)?;
        if touch_application_and_bootloader_usb == managed_application_usb
            || touch_application_and_bootloader_usb == recovery_bootloader_usb
            || managed_application_usb == recovery_bootloader_usb
        {
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
        if !valid_usb_string(&self.recovery_bootloader.manufacturer)
            || !valid_usb_string(&self.recovery_bootloader.product)
        {
            return Err(NrfSerialDfuSerialTransportError::InvalidRecoveryBootloaderStrings);
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
            recovery_bootloader_usb,
            recovery_bootloader_manufacturer: self.recovery_bootloader.manufacturer,
            recovery_bootloader_product: self.recovery_bootloader.product,
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
    pub board_identity: Uf2BoardIdentity,
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
    pub binary: String,
    pub board_feature: String,
    pub rust_target: String,
    pub mount_label: String,
    pub board_identity: Uf2BoardIdentity,
    pub application_usb: Uf2ApplicationUsb,
    pub variants: Vec<Uf2BuildVariant>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Uf2BoardIdentity {
    #[serde(rename = "match")]
    pub match_kind: Uf2BoardIdMatchKind,
    pub value: String,
}

impl Uf2BoardIdentity {
    pub fn validated(&self) -> Result<Uf2BoardIdMatch, crate::DomainValueError> {
        Uf2BoardIdMatch::parse(self.match_kind, self.value.clone())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Uf2ApplicationUsb {
    pub usb: UsbVendorProductId,
    pub manufacturer: String,
    pub product: String,
    pub serial_number: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Uf2BuildVariant {
    pub softdevice_family: String,
    pub softdevice_version: String,
    pub fwid: String,
    pub application_base: String,
    pub application_end_exclusive: String,
    pub family_id: String,
    pub application_link: Uf2ApplicationLink,
    pub target_directory: String,
    pub filename: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Uf2ApplicationLink {
    SoftdeviceS140V6,
    SoftdeviceS140V7,
    BareMetal,
}

impl Uf2ApplicationLink {
    pub const fn cargo_feature(self) -> Option<&'static str> {
        match self {
            Self::SoftdeviceS140V6 => Some("softdevice-s140-v6"),
            Self::SoftdeviceS140V7 => Some("softdevice-s140-v7"),
            Self::BareMetal => None,
        }
    }
}

#[derive(Debug, Error)]
pub enum CatalogError {
    #[error("board catalog is invalid: {0}")]
    Json(#[from] serde_json::Error),
    #[error("unsupported board catalog schema {0}")]
    Schema(u32),
    #[error("duplicate board slug {0:?}")]
    DuplicateSlug(String),
    #[error("UF2 Board-ID match rules overlap between {first:?} and {second:?}")]
    OverlappingUf2BoardIdentities { first: String, second: String },
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
        if self.schema_version != BOARD_CATALOG_SCHEMA {
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
        validate_uf2_board_identities(&self.boards)?;
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
                || Uf2MountLabel::parse(build.mount_label.clone()).is_err()
                || build.board_identity.validated().is_err()
                || build.rust_target != "thumbv7em-none-eabihf"
                || !valid_uf2_application_usb(&build.application_usb)
                || !matches_pinned_uf2_recipe(board, build)
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

/// Reject UF2 Board-ID rules whose accepted identities overlap.
fn validate_uf2_board_identities(boards: &[BoardCatalogEntry]) -> Result<(), CatalogError> {
    let identities = boards
        .iter()
        .filter_map(|board| match &board.build {
            BoardBuild::Uf2(build) => Some((board.slug.as_str(), &build.board_identity)),
            BoardBuild::Esp(_) => None,
            BoardBuild::NrfSerialDfu(build) => {
                Some((board.slug.as_str(), &build.recovery.board_identity))
            }
        })
        .collect::<Vec<_>>();
    for (index, (slug, identity)) in identities.iter().enumerate() {
        let identity = identity
            .validated()
            .map_err(|_| CatalogError::InvalidBoard {
                board: (*slug).to_string(),
                message: "UF2 Board-ID match rule is invalid".to_string(),
            })?;
        for (other_slug, other_identity) in &identities[index + 1..] {
            let other_identity =
                other_identity
                    .validated()
                    .map_err(|_| CatalogError::InvalidBoard {
                        board: (*other_slug).to_string(),
                        message: "UF2 Board-ID match rule is invalid".to_string(),
                    })?;
            if identity.overlaps(&other_identity) {
                return Err(CatalogError::OverlappingUf2BoardIdentities {
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

struct PinnedUf2Recipe {
    preparation_profile: PreparationProfile,
    package: &'static str,
    binary: &'static str,
    board_feature: &'static str,
    manufacturer: &'static str,
    product: &'static str,
    serial_number: &'static str,
    variants: &'static [PinnedUf2Variant],
}

struct PinnedUf2Variant {
    softdevice_family: &'static str,
    softdevice_version: &'static str,
    fwid: &'static str,
    application_base: &'static str,
    application_end_exclusive: &'static str,
    family_id: &'static str,
    application_link: Uf2ApplicationLink,
    target_directory: &'static str,
    filename: &'static str,
}

const T_ECHO_UF2_RECIPE: PinnedUf2Recipe = PinnedUf2Recipe {
    preparation_profile: PreparationProfile::TechoUf2,
    package: "t-echo",
    binary: "t-echo",
    board_feature: "board-t-echo",
    manufacturer: "Stay Personal",
    product: "Personal Hopspot (T-Echo)",
    serial_number: "PERSONAL-RNS-TECHO-HOP",
    variants: &[
        PinnedUf2Variant {
            softdevice_family: "s140",
            softdevice_version: "6.1.1",
            fwid: "0x00b6",
            application_base: "0x00026000",
            application_end_exclusive: "0x000c0000",
            family_id: "0xada52840",
            application_link: Uf2ApplicationLink::SoftdeviceS140V6,
            target_directory: "target/s140-v6",
            filename: "t-echo-s140-6.1.1.uf2",
        },
        PinnedUf2Variant {
            softdevice_family: "s140",
            softdevice_version: "7.3.0",
            fwid: "0x0123",
            application_base: "0x00027000",
            application_end_exclusive: "0x000c0000",
            family_id: "0xada52840",
            application_link: Uf2ApplicationLink::SoftdeviceS140V7,
            target_directory: "target/s140-v7",
            filename: "t-echo-s140-7.3.0.uf2",
        },
    ],
};

const T114_UF2_RECIPE: PinnedUf2Recipe = PinnedUf2Recipe {
    preparation_profile: PreparationProfile::T114Uf2,
    package: "t-echo",
    binary: "heltec-t114",
    board_feature: "board-t114",
    manufacturer: "Stay Personal",
    product: "Personal Hopspot (Heltec T114)",
    serial_number: "PERSONAL-RNS-T114-HOP",
    variants: &[PinnedUf2Variant {
        softdevice_family: "s140",
        softdevice_version: "6.1.1",
        fwid: "0x00b6",
        application_base: "0x00026000",
        application_end_exclusive: "0x000e9000",
        family_id: "0xada52840",
        application_link: Uf2ApplicationLink::BareMetal,
        target_directory: "target/t114",
        filename: "heltec-t114-s140-6.1.1.uf2",
    }],
};

const T096_UF2_RECIPE: PinnedUf2Recipe = PinnedUf2Recipe {
    preparation_profile: PreparationProfile::T096Uf2,
    package: "t-echo",
    binary: "t096",
    board_feature: "board-t096",
    manufacturer: "Stay Personal",
    product: "Personal Hopspot (Heltec T096)",
    serial_number: "PERSONAL-RNS-T096-HOP",
    variants: &[PinnedUf2Variant {
        softdevice_family: "s140",
        softdevice_version: "6.1.1",
        fwid: "0x00b6",
        application_base: "0x00026000",
        application_end_exclusive: "0x000e8000",
        family_id: "0xada52840",
        application_link: Uf2ApplicationLink::SoftdeviceS140V6,
        target_directory: "target/t096",
        filename: "t096-s140-6.1.1.uf2",
    }],
};

fn pinned_uf2_recipe(slug: &str) -> Option<&'static PinnedUf2Recipe> {
    match slug {
        "t-echo" => Some(&T_ECHO_UF2_RECIPE),
        "t096" => Some(&T096_UF2_RECIPE),
        "t114" => Some(&T114_UF2_RECIPE),
        _ => None,
    }
}

fn matches_pinned_uf2_recipe(board: &BoardCatalogEntry, build: &Uf2Build) -> bool {
    let Some(recipe) = pinned_uf2_recipe(&board.slug) else {
        return false;
    };
    PreparationProfile::parse(&board.preparation_profile) == Ok(recipe.preparation_profile)
        && build.package == recipe.package
        && build.binary == recipe.binary
        && build.board_feature == recipe.board_feature
        && build.application_usb.manufacturer == recipe.manufacturer
        && build.application_usb.product == recipe.product
        && build.application_usb.serial_number == recipe.serial_number
        && build
            .variants
            .iter()
            .map(|variant| {
                (
                    variant.softdevice_family.as_str(),
                    variant.softdevice_version.as_str(),
                    variant.fwid.as_str(),
                    variant.application_base.as_str(),
                    variant.application_end_exclusive.as_str(),
                    variant.family_id.as_str(),
                    variant.application_link,
                    variant.target_directory.as_str(),
                    variant.filename.as_str(),
                )
            })
            .eq(recipe.variants.iter().map(|variant| {
                (
                    variant.softdevice_family,
                    variant.softdevice_version,
                    variant.fwid,
                    variant.application_base,
                    variant.application_end_exclusive,
                    variant.family_id,
                    variant.application_link,
                    variant.target_directory,
                    variant.filename,
                )
            }))
        && build.variants.iter().all(|variant| {
            parse_hex_u32(&variant.application_base)
                .zip(parse_hex_u32(&variant.application_end_exclusive))
                .is_some_and(|(base, end)| {
                    base < end && base % 0x1000 == 0 && end % 0x1000 == 0 && end <= 0x0010_0000
                })
                && parse_hex_u32(&variant.family_id).is_some()
        })
}

fn valid_uf2_application_usb(application_usb: &Uf2ApplicationUsb) -> bool {
    use prns_core::interfaces::usb_auto::{WEBUSB_PRODUCT_ID, WEBUSB_VENDOR_ID};
    parse_usb_vendor_product_id(&application_usb.usb)
        .is_some_and(|usb| usb.vendor_id == WEBUSB_VENDOR_ID && usb.product_id == WEBUSB_PRODUCT_ID)
        && valid_usb_string(&application_usb.manufacturer)
        && valid_usb_string(&application_usb.product)
        && valid_usb_string(&application_usb.serial_number)
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
        && recovery.board_identity.validated().is_ok()
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
        assert_eq!(catalog.schema_version, 4);
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
                "t-echo",
                "t114",
                "t096",
                "t1000-e"
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
            [
                "heltec-v4-r8",
                "t-beam-supreme",
                "xiao-esp32-c6",
                "t-echo",
                "t114",
                "t096",
                "t1000-e"
            ]
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
                ("t114", None, None),
                ("t096", None, None),
                ("t1000-e", None, None),
            ]
        );
        Ok(())
    }

    #[test]
    fn t096_shipping_contract_matches_the_hardware_receipt() -> Result<(), CatalogError> {
        let catalog = board_catalog()?;
        let board = catalog
            .board("t096")
            .ok_or_else(|| CatalogError::InvalidBoard {
                board: "t096".to_string(),
                message: "missing shipping target".to_string(),
            })?;
        let BoardBuild::Uf2(build) = &board.build else {
            return Err(invalid(board, "expected a UF2 build"));
        };

        assert_eq!(board.availability, BoardAvailability::Shipping);
        assert_eq!(board.preparation_profile, "t096-uf2");
        assert_eq!(build.package, "t-echo");
        assert_eq!(build.binary, "t096");
        assert_eq!(build.board_feature, "board-t096");
        assert_eq!(build.rust_target, "thumbv7em-none-eabihf");
        assert_eq!(build.mount_label, "HT-n5262G");
        assert_eq!(build.board_identity.match_kind, Uf2BoardIdMatchKind::Exact);
        assert_eq!(build.board_identity.value, "ht-n5262g");
        assert_eq!(build.application_usb.usb.vendor_id, "0x1209");
        assert_eq!(build.application_usb.usb.product_id, "0x0001");
        assert_eq!(build.application_usb.manufacturer, "Stay Personal");
        assert_eq!(
            build.application_usb.product,
            "Personal Hopspot (Heltec T096)"
        );
        assert_eq!(build.application_usb.serial_number, "PERSONAL-RNS-T096-HOP");
        let [variant] = build.variants.as_slice() else {
            return Err(invalid(board, "expected exactly one T096 variant"));
        };
        assert_eq!(variant.softdevice_family, "s140");
        assert_eq!(variant.softdevice_version, "6.1.1");
        assert_eq!(variant.fwid, "0x00b6");
        assert_eq!(variant.application_base, "0x00026000");
        assert_eq!(variant.application_end_exclusive, "0x000e8000");
        assert_eq!(variant.family_id, "0xada52840");
        assert_eq!(
            variant.application_link,
            Uf2ApplicationLink::SoftdeviceS140V6
        );
        assert_eq!(variant.target_directory, "target/t096");
        assert_eq!(variant.filename, "t096-s140-6.1.1.uf2");
        Ok(())
    }

    #[test]
    fn t1000e_shipping_contract_matches_the_recovery_bootloader() -> Result<(), CatalogError> {
        let catalog = board_catalog()?;
        let board = catalog
            .board("t1000-e")
            .ok_or_else(|| CatalogError::InvalidBoard {
                board: "t1000-e".to_string(),
                message: "missing shipping target".to_string(),
            })?;
        let BoardBuild::NrfSerialDfu(build) = &board.build else {
            return Err(invalid(board, "expected Nordic serial DFU build"));
        };
        assert_eq!(board.availability, BoardAvailability::Shipping);
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
        assert_eq!(build.serial.recovery_bootloader.usb.vendor_id, "0x239a");
        assert_eq!(build.serial.recovery_bootloader.usb.product_id, "0x8029");
        assert_eq!(
            build.serial.recovery_bootloader.manufacturer,
            "Seeed Studio"
        );
        assert_eq!(build.serial.recovery_bootloader.product, "T1000-E-BOOT");
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
        assert_eq!(
            build.recovery.board_identity.match_kind,
            Uf2BoardIdMatchKind::Exact
        );
        assert_eq!(build.recovery.board_identity.value, "nrf52840-t1000-e-v1");
        assert_eq!(build.recovery.family_id, "0xada52840");
        Ok(())
    }

    #[test]
    fn t114_shipping_contract_matches_the_hardware_receipt() -> Result<(), CatalogError> {
        let catalog = board_catalog()?;
        let board = catalog
            .board("t114")
            .ok_or_else(|| CatalogError::InvalidBoard {
                board: "t114".to_string(),
                message: "missing shipping target".to_string(),
            })?;
        let BoardBuild::Uf2(build) = &board.build else {
            return Err(invalid(board, "expected a UF2 build"));
        };
        assert_eq!(board.availability, BoardAvailability::Shipping);
        assert_eq!(board.preparation_profile, "t114-uf2");
        assert_eq!(build.package, "t-echo");
        assert_eq!(build.binary, "heltec-t114");
        assert_eq!(build.board_feature, "board-t114");
        assert_eq!(build.mount_label, "HT-n5262");
        assert_eq!(build.board_identity.match_kind, Uf2BoardIdMatchKind::Exact);
        assert_eq!(build.board_identity.value, "ht-n5262");
        assert_eq!(build.application_usb.usb.vendor_id, "0x1209");
        assert_eq!(build.application_usb.usb.product_id, "0x0001");
        assert_eq!(build.application_usb.manufacturer, "Stay Personal");
        assert_eq!(
            build.application_usb.product,
            "Personal Hopspot (Heltec T114)"
        );
        assert_eq!(build.application_usb.serial_number, "PERSONAL-RNS-T114-HOP");
        let [variant] = build.variants.as_slice() else {
            return Err(invalid(board, "expected exactly one T114 variant"));
        };
        assert_eq!(variant.softdevice_family, "s140");
        assert_eq!(variant.softdevice_version, "6.1.1");
        assert_eq!(variant.fwid, "0x00b6");
        assert_eq!(variant.application_base, "0x00026000");
        assert_eq!(variant.application_end_exclusive, "0x000e9000");
        assert_eq!(variant.family_id, "0xada52840");
        assert_eq!(variant.application_link, Uf2ApplicationLink::BareMetal);
        assert_eq!(variant.target_directory, "target/t114");
        assert_eq!(variant.filename, "heltec-t114-s140-6.1.1.uf2");
        Ok(())
    }

    #[test]
    fn nrf_recovery_bootloader_identity_is_distinct_and_exact() -> Result<(), CatalogError> {
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

        let mut colliding = build.serial.clone();
        colliding.recovery_bootloader.usb = colliding.touch_application_and_bootloader.usb.clone();
        assert_eq!(
            colliding.into_validated(),
            Err(NrfSerialDfuSerialTransportError::IndistinguishableUsbModes)
        );

        let mut unnamed = build.serial.clone();
        unnamed.recovery_bootloader.product.clear();
        assert_eq!(
            unnamed.into_validated(),
            Err(NrfSerialDfuSerialTransportError::InvalidRecoveryBootloaderStrings)
        );
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
        build.board_identity = Uf2BoardIdentity {
            match_kind: Uf2BoardIdMatchKind::RevisionPrefix,
            value: "nrf52840-heltec-t114-v".to_string(),
        };
        catalog.validate()?;
        Ok(())
    }

    #[test]
    fn uf2_board_id_match_rules_may_not_overlap() -> Result<(), Box<dyn std::error::Error>> {
        let mut catalog = board_catalog()?;
        let uf2_prefix = catalog
            .boards
            .iter()
            .find_map(|board| match &board.build {
                BoardBuild::Uf2(build)
                    if build.board_identity.match_kind == Uf2BoardIdMatchKind::RevisionPrefix =>
                {
                    Some(build.board_identity.value.clone())
                }
                BoardBuild::Esp(_) | BoardBuild::NrfSerialDfu(_) => None,
                BoardBuild::Uf2(_) => None,
            })
            .ok_or("expected a UF2 board")?;
        let recovery = catalog
            .boards
            .iter_mut()
            .find_map(|board| match &mut board.build {
                BoardBuild::NrfSerialDfu(build) => Some(&mut build.recovery),
                BoardBuild::Esp(_) | BoardBuild::Uf2(_) => None,
            })
            .ok_or("expected a Nordic serial DFU board")?;
        recovery.board_identity = Uf2BoardIdentity {
            match_kind: Uf2BoardIdMatchKind::Exact,
            value: format!("{uf2_prefix}2"),
        };
        assert!(matches!(
            catalog.validate(),
            Err(CatalogError::OverlappingUf2BoardIdentities { .. })
        ));
        Ok(())
    }

    #[test]
    fn a_uf2_board_outside_the_pinned_recipes_is_rejected() -> Result<(), Box<dyn std::error::Error>>
    {
        let mut catalog = board_catalog()?;
        let board = catalog
            .boards
            .iter_mut()
            .find(|board| board.transport == Transport::Uf2MassStorage)
            .ok_or("expected a UF2 board")?;
        board.slug = "nrf52840-second-board".to_string();
        assert!(matches!(
            catalog.validate(),
            Err(CatalogError::InvalidBoard { .. })
        ));
        Ok(())
    }

    #[test]
    fn an_unnormalized_uf2_board_id_match_is_rejected() -> Result<(), Box<dyn std::error::Error>> {
        let mut catalog = board_catalog()?;
        let board = catalog
            .boards
            .iter_mut()
            .find(|board| board.transport == Transport::Uf2MassStorage)
            .ok_or("expected a UF2 board")?;
        let BoardBuild::Uf2(build) = &mut board.build else {
            return Err("expected a UF2 build".into());
        };
        build.board_identity.value = "nRF52840_TEcho_v".to_string();
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
