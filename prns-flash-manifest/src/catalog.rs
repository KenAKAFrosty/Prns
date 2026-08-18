use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    AfterResetStrategy, BeforeResetStrategy, BoardId, ChipFamily, PreparationProfile,
    ProvisioningFormat, Uf2BoardIdPrefix, Uf2MountLabel, CONFIG_OFFSET, CONFIG_PASSWORD_MAX_BYTES,
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
    /// Catalog schema version.
    pub schema: u32,
    pub boards: Vec<BoardCatalogEntry>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoardCatalogEntry {
    pub availability: BoardAvailability,
    pub slug: String,
    /// User-facing board name.
    pub display_name: String,
    /// Concise silicon description.
    pub silicon: String,
    /// User-facing supported interfaces.
    pub interfaces: Vec<String>,
    /// Website icon identifier.
    pub icon: String,
    pub transport: Transport,
    /// Expected Espressif chip, or `None` for UF2 targets.
    pub expected_chip: Option<String>,
    /// Physical flash capacity in bytes, when applicable.
    pub flash_size: Option<u32>,
    /// Stable instruction profile used by localized clients.
    pub preparation_profile: String,
    /// Optional local provisioning slot.
    pub provisioning: Option<ProvisioningDescriptor>,
    /// Developer-build inputs.
    pub build: BoardBuild,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BoardAvailability {
    Shipping,
    Qualification,
}

impl BoardCatalogEntry {
    /// Whether this board supports local Wi-Fi provisioning.
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

/// Public transport used by a board.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Transport {
    /// Espressif serial bootloader protocol.
    EspSerial,
    /// UF2 bootloader mass-storage copy.
    Uf2MassStorage,
}

/// Provisioning slot contract shared with firmware.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProvisioningDescriptor {
    /// Wire-format identifier.
    pub format: String,
    /// Wire-format version.
    pub version: u8,
    /// Absolute flash offset.
    pub offset: u32,
    /// Reserved slot size.
    pub size: u32,
    /// Maximum encoded SSID bytes.
    pub ssid_max_bytes: usize,
    /// Maximum encoded password bytes.
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

/// Developer build recipe.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum BoardBuild {
    /// Espressif Rust firmware build.
    Esp(EspBuild),
    /// Nordic UF2 build.
    Uf2(Uf2Build),
}

/// Espressif build and flash parameters.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EspBuild {
    /// espflash chip name.
    pub chip: String,
    /// Rust target triple.
    pub rust_target: String,
    /// Partition CSV filename.
    pub partition_table: String,
    /// Cargo package.
    pub package: String,
    /// Produced ELF basename.
    pub binary: String,
    /// espflash flash-size spelling.
    pub flash_size_label: String,
    /// SPI mode supplied to the browser flasher.
    pub flash_mode: String,
    /// SPI frequency supplied to the browser flasher.
    pub flash_frequency: String,
    /// Reset behavior before connecting.
    pub before_reset: String,
    /// Reset behavior after successful flashing.
    pub after_reset: String,
}

/// Nordic UF2 build parameters.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Uf2Build {
    /// Cargo package.
    pub package: String,
    /// Rust target triple.
    pub rust_target: String,
    /// Bootloader volume label.
    pub mount_label: String,
    /// Normalized `Board-ID` prefix this bootloader publishes in `INFO_UF2.TXT`.
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

/// Catalog loading or invariant failure.
#[derive(Debug, Error)]
pub enum CatalogError {
    /// Catalog JSON could not be parsed.
    #[error("board catalog is invalid: {0}")]
    Json(#[from] serde_json::Error),
    /// Catalog schema is unsupported.
    #[error("unsupported board catalog schema {0}")]
    Schema(u32),
    /// A board slug occurs more than once.
    #[error("duplicate board slug {0:?}")]
    DuplicateSlug(String),
    /// Two UF2 boards could both claim one mounted bootloader drive.
    #[error("UF2 board-id prefixes overlap between {first:?} and {second:?}")]
    OverlappingUf2BoardIdPrefixes {
        /// One board's slug.
        first: String,
        /// The other board's slug.
        second: String,
    },
    /// A catalog invariant is invalid.
    #[error("board {board:?}: {message}")]
    InvalidBoard {
        /// Board slug.
        board: String,
        /// Failure detail.
        message: String,
    },
}

impl BoardCatalog {
    /// Parse and validate a catalog document.
    pub fn from_json(bytes: &[u8]) -> Result<Self, CatalogError> {
        let catalog: Self = serde_json::from_slice(bytes)?;
        catalog.validate()?;
        Ok(catalog)
    }

    /// Validate catalog-wide and per-board invariants.
    pub fn validate(&self) -> Result<(), CatalogError> {
        if self.schema != 2 {
            return Err(CatalogError::Schema(self.schema));
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

    /// Find a board by stable slug.
    pub fn board(&self, slug: &str) -> Option<&BoardCatalogEntry> {
        self.boards.iter().find(|board| board.slug == slug)
    }

    pub fn shipping_boards(&self) -> impl Iterator<Item = &BoardCatalogEntry> {
        self.boards
            .iter()
            .filter(|board| board.availability == BoardAvailability::Shipping)
    }
}

/// Load the catalog embedded from `release/flash/boards.json`.
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

fn parse_hex_u32(value: &str) -> Option<u32> {
    u32::from_str_radix(value.strip_prefix("0x")?, 16).ok()
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
            ]
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
            .pop();
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
