use prns_flash_manifest::Uf2BoardIdMatchKind;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Tier {
    Shipping,
    SdkPreview,
    Flashable,
    Qualification,
    BringUp,
    Roadmap,
}

impl Tier {
    pub fn chip_badge(self) -> Option<&'static str> {
        match self {
            Tier::Shipping => None,
            Tier::SdkPreview => Some("SDK preview"),
            Tier::Flashable => Some("flashable"),
            Tier::Qualification => Some("qualification"),
            Tier::BringUp => Some("bring-up"),
            Tier::Roadmap => Some("roadmap"),
        }
    }

    pub fn muted(self) -> bool {
        matches!(self, Tier::BringUp | Tier::Roadmap)
    }

    pub fn flash_card_class(self) -> &'static str {
        match self {
            Tier::Shipping | Tier::SdkPreview => "flash-board-card--runtime",
            Tier::Flashable => "flash-board-card--flashable",
            Tier::Qualification => "flash-board-card--qualification",
            Tier::BringUp => "flash-board-card--bringup",
            Tier::Roadmap => "flash-board-card--roadmap",
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum Group {
    Desktop,
    Mobile,
    Microcontroller,
    SingleBoardComputer,
    Web,
    Server,
    Language,
    GameEngine,
}

impl Group {
    pub fn label(self) -> &'static str {
        match self {
            Group::Desktop => "Desktop",
            Group::Mobile => "Mobile",
            Group::Microcontroller => "Microcontrollers & radios",
            Group::SingleBoardComputer => "Single-board computers",
            Group::Web => "Web & browsers",
            Group::Server => "Web servers & edge",
            Group::Language => "Languages & bindings",
            Group::GameEngine => "Game engines",
        }
    }
}

pub struct Platform {
    pub name: &'static str,
    pub group: Group,
    pub tier: Tier,
    /// A Simple Icons slug maps to bundled `/assets/logos/<slug>.svg`; CSS masks tint it to the chip's text color. `None` selects a text-only chip when no clean logo exists.
    pub icon: Option<&'static str>,
}

pub struct LandingPlatformChip {
    pub name: &'static str,
    pub icon: Option<&'static str>,
}

pub struct BoardImage {
    pub data_uri: &'static str,
}

pub const ESPRESSIF_NATIVE_USB_VENDOR_ID: u16 = 0x303a;

#[derive(Clone, Copy, PartialEq)]
pub enum PreparationProfile {
    EspUsbBoot,
    TechoUf2,
    T114Uf2,
    T096Uf2,
    T1000eNrfSerialDfu,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BoardFlashTarget {
    EspSerial {
        expected_chip: &'static str,
        web_serial_vendor_id: u16,
        supports_provisioning: bool,
        supports_tcp_client_provisioning: bool,
    },
    Uf2MassStorage {
        mount_label: &'static str,
        board_id_match_kind: Uf2BoardIdMatchKind,
        board_id: &'static str,
    },
    NrfSerialDfu {
        recovery_mount_label: &'static str,
        recovery_board_id_match_kind: Uf2BoardIdMatchKind,
        recovery_board_id: &'static str,
    },
}

impl BoardFlashTarget {
    pub const fn uses_web_serial(self) -> bool {
        matches!(self, Self::EspSerial { .. } | Self::NrfSerialDfu { .. })
    }

    pub const fn supports_provisioning(self) -> bool {
        matches!(
            self,
            Self::EspSerial {
                supports_provisioning: true,
                ..
            }
        )
    }

    pub const fn expected_chip(self) -> Option<&'static str> {
        match self {
            Self::EspSerial { expected_chip, .. } => Some(expected_chip),
            Self::Uf2MassStorage { .. } | Self::NrfSerialDfu { .. } => None,
        }
    }

    pub const fn supports_tcp_client_provisioning(self) -> bool {
        matches!(
            self,
            Self::EspSerial {
                supports_tcp_client_provisioning: true,
                ..
            }
        )
    }
}

pub mod board_images {
    include!(concat!(env!("OUT_DIR"), "/board_images.rs"));
}

pub mod shipping_boards {
    include!(concat!(env!("OUT_DIR"), "/shipping_boards.rs"));
}

pub use shipping_boards::{QUALIFICATION_BOARD_TARGETS, SHIPPING_BOARD_TARGETS};

#[derive(Clone, Copy, PartialEq)]
pub struct BoardTarget {
    pub name: &'static str,
    pub slug: &'static str,
    pub silicon: &'static str,
    pub tier: Tier,
    pub interfaces: &'static [&'static str],
    pub icon: Option<&'static str>,
    pub preparation_profile: Option<PreparationProfile>,
    pub flash_target: Option<BoardFlashTarget>,
}

impl BoardTarget {
    pub fn is_flashable(&self) -> bool {
        matches!(self.tier, Tier::Flashable)
            || (cfg!(feature = "local-dev-flasher")
                && matches!(self.tier, Tier::Qualification)
                && self.preparation_profile.is_some()
                && self.flash_target.is_some())
    }

    pub fn image(&self) -> Option<&'static BoardImage> {
        match self.slug {
            "heltec-v4" => Some(&board_images::HELTEC_V4),
            "heltec-v4-r8" => Some(&board_images::HELTEC_V4),
            "t-beam-supreme" => Some(&board_images::T_BEAM_SUPREME),
            "xiao-esp32-c6" => Some(&board_images::XIAO_ESP32_C6),
            "t-echo" => Some(&board_images::T_ECHO),
            "t114" => Some(&board_images::T114),
            "t1000-e" => Some(&board_images::SEEED_CARD_TRACKER_T1000_E),
            "t096" => Some(&board_images::HELTEC_MESH_NODE_T096),
            "mesh-tower-v2" => Some(&board_images::MESH_TOWER_V2),
            _ => None,
        }
    }
}

pub const GROUPS: &[Group] = &[
    Group::Desktop,
    Group::Mobile,
    Group::SingleBoardComputer,
    Group::Microcontroller,
    Group::Web,
    Group::Server,
    Group::Language,
    Group::GameEngine,
];

pub const UPCOMING_BOARD_TARGETS: &[BoardTarget] = &[
    BoardTarget {
        name: "muzi.works Base Duo",
        slug: "muzi-works-base-duo",
        silicon: "nRF52840 + LR1121",
        tier: Tier::BringUp,
        interfaces: &[],
        icon: Some("nordicsemiconductor"),
        preparation_profile: None,
        flash_target: None,
    },
    BoardTarget {
        name: "Heltec Wireless Stick Lite V3",
        slug: "heltec-wireless-stick-lite-v3",
        silicon: "ESP32-S3 + SX1262",
        tier: Tier::BringUp,
        interfaces: &[],
        icon: Some("espressif"),
        preparation_profile: None,
        flash_target: None,
    },
    BoardTarget {
        name: "Raspberry Pi Zero 2 W",
        slug: "raspberry-pi-zero-2-w",
        silicon: "RP3A0, quad-core Arm Cortex-A53",
        tier: Tier::BringUp,
        interfaces: &[],
        icon: Some("raspberrypi"),
        preparation_profile: None,
        flash_target: None,
    },
    BoardTarget {
        name: "Heltec V3/V3.1",
        slug: "heltec-v3",
        silicon: "ESP32-S3 + SX1262",
        tier: Tier::Roadmap,
        interfaces: &[],
        icon: Some("espressif"),
        preparation_profile: None,
        flash_target: None,
    },
    BoardTarget {
        name: "RAK WisBlock Starter Kit",
        slug: "rak-wisblock-starter-kit",
        silicon: "RAK19007 + RAK4631, nRF52840 + SX1262",
        tier: Tier::BringUp,
        interfaces: &[],
        icon: Some("nordicsemiconductor"),
        preparation_profile: None,
        flash_target: None,
    },
    BoardTarget {
        name: "Seeed Wio Tracker L1",
        slug: "seeed-wio-tracker-l1",
        silicon: "nRF52840 + SX1262",
        tier: Tier::Roadmap,
        interfaces: &[],
        icon: Some("nordicsemiconductor"),
        preparation_profile: None,
        flash_target: None,
    },
    BoardTarget {
        name: "SenseCAP Solar Node P1",
        slug: "seeed-sensecap-solar-node-p1",
        silicon: "nRF52840 + SX1262",
        tier: Tier::Roadmap,
        interfaces: &[],
        icon: Some("nordicsemiconductor"),
        preparation_profile: None,
        flash_target: None,
    },
    BoardTarget {
        name: "LILYGO LoRa32 T3-S3",
        slug: "lilygo-lora32-t3-s3",
        silicon: "ESP32-S3 + SX1262/SX1276/SX1280/LR1121 variants",
        tier: Tier::Roadmap,
        interfaces: &[],
        icon: Some("espressif"),
        preparation_profile: None,
        flash_target: None,
    },
    BoardTarget {
        name: "B&Q Nano G2 Ultra",
        slug: "bq-nano-g2-ultra",
        silicon: "nRF52840 + SX1262",
        tier: Tier::Roadmap,
        interfaces: &[],
        icon: Some("nordicsemiconductor"),
        preparation_profile: None,
        flash_target: None,
    },
    BoardTarget {
        name: "B&Q Station G2",
        slug: "bq-station-g2",
        silicon: "ESP32-S3 + SX1262",
        tier: Tier::Roadmap,
        interfaces: &[],
        icon: Some("espressif"),
        preparation_profile: None,
        flash_target: None,
    },
];

pub const IN_PROGRESS_BOARD_TARGETS: &[BoardTarget] = &[BoardTarget {
    name: "Heltec MeshTower V2",
    slug: "mesh-tower-v2",
    silicon: "nRF52840 + SX1262 + KCT8103L PA",
    tier: Tier::Qualification,
    interfaces: &[],
    icon: Some("nordicsemiconductor"),
    preparation_profile: None,
    flash_target: None,
}];

pub fn board_target_by_slug(slug: &str) -> Option<&'static BoardTarget> {
    SHIPPING_BOARD_TARGETS
        .iter()
        .chain(QUALIFICATION_BOARD_TARGETS.iter())
        .chain(IN_PROGRESS_BOARD_TARGETS.iter())
        .chain(UPCOMING_BOARD_TARGETS.iter())
        .find(|board| board.slug == slug)
}

pub const PLATFORMS: &[Platform] = &[
    Platform {
        name: "Linux",
        group: Group::Desktop,
        tier: Tier::Shipping,
        icon: Some("linux"),
    },
    Platform {
        name: "macOS",
        group: Group::Desktop,
        tier: Tier::Shipping,
        icon: Some("apple"),
    },
    Platform {
        name: "Windows",
        group: Group::Desktop,
        tier: Tier::Shipping,
        icon: Some("windows"),
    },
    Platform {
        name: "Android",
        group: Group::Mobile,
        tier: Tier::Shipping,
        icon: Some("android"),
    },
    Platform {
        name: "iOS",
        group: Group::Mobile,
        tier: Tier::Shipping,
        icon: Some("apple"),
    },
    Platform {
        name: "ESP32-S3",
        group: Group::Microcontroller,
        tier: Tier::Shipping,
        icon: Some("espressif"),
    },
    Platform {
        name: "ESP32-C6",
        group: Group::Microcontroller,
        tier: Tier::Shipping,
        icon: Some("espressif"),
    },
    Platform {
        name: "RISC-V",
        group: Group::Microcontroller,
        tier: Tier::Shipping,
        icon: Some("riscv"),
    },
    Platform {
        name: "nRF52840",
        group: Group::Microcontroller,
        tier: Tier::Shipping,
        icon: Some("nordicsemiconductor"),
    },
    Platform {
        name: "SX1262",
        group: Group::Microcontroller,
        tier: Tier::Shipping,
        icon: Some("semtech"),
    },
    Platform {
        name: "LR1110",
        group: Group::Microcontroller,
        tier: Tier::Shipping,
        icon: Some("semtech"),
    },
    Platform {
        name: "Raspberry Pi RP3A0",
        group: Group::SingleBoardComputer,
        tier: Tier::BringUp,
        icon: Some("raspberrypi"),
    },
    Platform {
        name: "RP2040",
        group: Group::Microcontroller,
        tier: Tier::Roadmap,
        icon: Some("raspberrypi"),
    },
    Platform {
        name: "STM32",
        group: Group::Microcontroller,
        tier: Tier::Roadmap,
        icon: Some("stmicroelectronics"),
    },
    Platform {
        name: "WebAssembly",
        group: Group::Web,
        tier: Tier::Shipping,
        icon: Some("webassembly"),
    },
    Platform {
        name: "Dioxus",
        group: Group::Web,
        tier: Tier::BringUp,
        icon: Some("dioxus.png"),
    },
    Platform {
        name: "Chrome",
        group: Group::Web,
        tier: Tier::Shipping,
        icon: Some("googlechrome"),
    },
    Platform {
        name: "Firefox",
        group: Group::Web,
        tier: Tier::Shipping,
        icon: Some("firefoxbrowser"),
    },
    Platform {
        name: "Safari",
        group: Group::Web,
        tier: Tier::Shipping,
        icon: Some("safari"),
    },
    Platform {
        name: "Node",
        group: Group::Server,
        tier: Tier::Shipping,
        icon: Some("nodedotjs"),
    },
    Platform {
        name: "Bun",
        group: Group::Server,
        tier: Tier::Shipping,
        icon: Some("bun"),
    },
    Platform {
        name: "Deno",
        group: Group::Server,
        tier: Tier::Roadmap,
        icon: Some("deno"),
    },
    Platform {
        name: "Cloudflare Workers",
        group: Group::Server,
        tier: Tier::Roadmap,
        icon: Some("cloudflareworkers"),
    },
    Platform {
        name: "Fastly",
        group: Group::Server,
        tier: Tier::Roadmap,
        icon: Some("fastly"),
    },
    Platform {
        name: "Rust",
        group: Group::Language,
        tier: Tier::Shipping,
        icon: Some("rust"),
    },
    Platform {
        name: "TypeScript",
        group: Group::Language,
        tier: Tier::Shipping,
        icon: Some("typescript"),
    },
    Platform {
        name: "Kotlin",
        group: Group::Language,
        tier: Tier::SdkPreview,
        icon: Some("kotlin"),
    },
    Platform {
        name: "Swift",
        group: Group::Language,
        tier: Tier::SdkPreview,
        icon: Some("swift"),
    },
    Platform {
        name: "Python",
        group: Group::Language,
        tier: Tier::SdkPreview,
        icon: Some("python"),
    },
    Platform {
        name: "Go",
        group: Group::Language,
        tier: Tier::SdkPreview,
        icon: Some("go"),
    },
    Platform {
        name: "Julia",
        group: Group::Language,
        tier: Tier::SdkPreview,
        icon: Some("julia"),
    },
    Platform {
        name: "Java",
        group: Group::Language,
        tier: Tier::SdkPreview,
        icon: Some("openjdk"),
    },
    Platform {
        name: ".NET",
        group: Group::Language,
        tier: Tier::SdkPreview,
        icon: Some("dotnet"),
    },
    Platform {
        name: "C",
        group: Group::Language,
        tier: Tier::SdkPreview,
        icon: Some("c"),
    },
    Platform {
        name: "C++",
        group: Group::Language,
        tier: Tier::SdkPreview,
        icon: Some("cplusplus"),
    },
    Platform {
        name: "Ruby",
        group: Group::Language,
        tier: Tier::Roadmap,
        icon: Some("ruby"),
    },
    Platform {
        name: "Zig",
        group: Group::Language,
        tier: Tier::Roadmap,
        icon: Some("zig"),
    },
    Platform {
        name: "Unity",
        group: Group::GameEngine,
        tier: Tier::Roadmap,
        icon: Some("unity"),
    },
    Platform {
        name: "Godot",
        group: Group::GameEngine,
        tier: Tier::BringUp,
        icon: Some("godotengine"),
    },
    Platform {
        name: "MonoGame",
        group: Group::GameEngine,
        tier: Tier::Roadmap,
        icon: Some("monogame"),
    },
];

pub const LANDING_PLATFORM_CHIPS: &[LandingPlatformChip] = &[
    LandingPlatformChip {
        name: "Linux",
        icon: Some("linux"),
    },
    LandingPlatformChip {
        name: "macOS",
        icon: Some("apple"),
    },
    LandingPlatformChip {
        name: "Windows",
        icon: Some("windows"),
    },
    LandingPlatformChip {
        name: "Android",
        icon: Some("android"),
    },
    LandingPlatformChip {
        name: "iOS",
        icon: Some("apple"),
    },
    LandingPlatformChip {
        name: "ESP32-S3",
        icon: Some("espressif"),
    },
    LandingPlatformChip {
        name: "ESP32-C6",
        icon: Some("espressif"),
    },
    LandingPlatformChip {
        name: "RISC-V",
        icon: Some("riscv"),
    },
    LandingPlatformChip {
        name: "nRF52840",
        icon: Some("nordicsemiconductor"),
    },
    LandingPlatformChip {
        name: "SX1262",
        icon: Some("semtech"),
    },
    LandingPlatformChip {
        name: "LR1110",
        icon: Some("semtech"),
    },
    LandingPlatformChip {
        name: "Rust",
        icon: Some("rust"),
    },
    LandingPlatformChip {
        name: "TypeScript",
        icon: Some("typescript"),
    },
    LandingPlatformChip {
        name: "Kotlin",
        icon: Some("kotlin"),
    },
    LandingPlatformChip {
        name: "Swift",
        icon: Some("swift"),
    },
    LandingPlatformChip {
        name: "Python",
        icon: Some("python"),
    },
    LandingPlatformChip {
        name: "Go",
        icon: Some("go"),
    },
    LandingPlatformChip {
        name: "Java",
        icon: Some("openjdk"),
    },
    LandingPlatformChip {
        name: ".NET",
        icon: Some("dotnet"),
    },
    LandingPlatformChip {
        name: "Julia",
        icon: Some("julia"),
    },
    LandingPlatformChip {
        name: "C",
        icon: Some("c"),
    },
    LandingPlatformChip {
        name: "C++",
        icon: Some("cplusplus"),
    },
    LandingPlatformChip {
        name: "WebAssembly",
        icon: Some("webassembly"),
    },
    LandingPlatformChip {
        name: "Chrome",
        icon: Some("googlechrome"),
    },
    LandingPlatformChip {
        name: "Firefox",
        icon: Some("firefoxbrowser"),
    },
    LandingPlatformChip {
        name: "Safari",
        icon: Some("safari"),
    },
    LandingPlatformChip {
        name: "Node",
        icon: Some("nodedotjs"),
    },
    LandingPlatformChip {
        name: "Bun",
        icon: Some("bun"),
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_active_board_work_is_presented_as_bring_up() {
        let bring_up = UPCOMING_BOARD_TARGETS
            .iter()
            .filter(|board| board.tier == Tier::BringUp)
            .map(|board| board.name)
            .collect::<Vec<_>>();

        assert_eq!(
            bring_up,
            vec![
                "muzi.works Base Duo",
                "Heltec Wireless Stick Lite V3",
                "Raspberry Pi Zero 2 W",
                "RAK WisBlock Starter Kit",
            ]
        );
        assert!(
            UPCOMING_BOARD_TARGETS
                .iter()
                .filter(|board| !bring_up.contains(&board.name))
                .all(|board| board.tier == Tier::Roadmap),
            "every other non-shipping board should remain on the roadmap"
        );
    }

    #[test]
    fn in_progress_boards_sit_in_the_main_grid_with_their_status() {
        let cards = IN_PROGRESS_BOARD_TARGETS
            .iter()
            .map(|board| (board.slug, board.tier, board.image().is_some()))
            .collect::<Vec<_>>();
        assert_eq!(cards, vec![("mesh-tower-v2", Tier::Qualification, true)]);
    }

    #[test]
    fn promoted_nordic_boards_come_from_the_shared_shipping_catalog() {
        assert!(QUALIFICATION_BOARD_TARGETS.is_empty());
        let cards = SHIPPING_BOARD_TARGETS
            .iter()
            .filter(|board| matches!(board.slug, "t096" | "t1000-e"))
            .map(|board| (board.slug, board.tier, board.image().is_some()))
            .collect::<Vec<_>>();
        assert_eq!(
            cards,
            vec![
                ("t096", Tier::Flashable, true),
                ("t1000-e", Tier::Flashable, true),
            ]
        );
        assert!(SHIPPING_BOARD_TARGETS
            .iter()
            .filter(|board| matches!(board.slug, "t096" | "t1000-e"))
            .all(|board| board.is_flashable()
                && board.preparation_profile.is_some()
                && board.flash_target.is_some()));
    }

    #[test]
    fn implemented_sdk_tiers_match_release_readiness() {
        let expected = [
            ("Rust", Tier::Shipping),
            ("TypeScript", Tier::Shipping),
            ("Kotlin", Tier::SdkPreview),
            ("Swift", Tier::SdkPreview),
            ("Python", Tier::SdkPreview),
            ("Go", Tier::SdkPreview),
            ("Java", Tier::SdkPreview),
            (".NET", Tier::SdkPreview),
            ("Julia", Tier::SdkPreview),
            ("C", Tier::SdkPreview),
            ("C++", Tier::SdkPreview),
            ("Ruby", Tier::Roadmap),
            ("Zig", Tier::Roadmap),
        ];

        assert_eq!(
            PLATFORMS
                .iter()
                .filter(|platform| platform.group == Group::Language)
                .count(),
            expected.len(),
            "every language and binding should have an explicit expected tier"
        );
        for (name, tier) in expected {
            let platform = PLATFORMS
                .iter()
                .find(|platform| platform.name == name)
                .unwrap_or_else(|| panic!("{name} should be present in the platform catalog"));
            assert!(
                platform.tier == tier,
                "{name} should have the expected release tier"
            );
        }
    }

    #[test]
    fn homepage_platform_marquee_does_not_name_specific_boards() {
        let board_names = ["Heltec V4", "T-Beam Supreme", "T-Echo", "XIAO ESP32-C6"];

        assert!(
            LANDING_PLATFORM_CHIPS
                .iter()
                .all(|platform| !board_names.contains(&platform.name)),
            "the Runs on marquee should name platform families, not boards"
        );
    }

    #[test]
    fn homepage_platform_marquee_does_not_present_roadmap_work_as_available() {
        assert!(
            LANDING_PLATFORM_CHIPS.iter().all(|chip| {
                PLATFORMS
                    .iter()
                    .find(|platform| platform.name == chip.name)
                    .is_none_or(|platform| platform.tier != Tier::Roadmap)
            }),
            "the unbadged Runs on marquee should omit roadmap platforms"
        );
    }

    #[test]
    fn deferred_server_platforms_remain_on_the_roadmap() {
        for name in ["Deno", "Cloudflare Workers", "Fastly"] {
            let platform = PLATFORMS
                .iter()
                .find(|platform| platform.name == name)
                .unwrap_or_else(|| panic!("{name} should be present in the platform catalog"));
            assert!(platform.tier == Tier::Roadmap, "{name} should be roadmap");
        }
    }

    #[test]
    fn raspberry_pi_zero_2_w_platform_is_presented_as_bring_up() {
        let platform = PLATFORMS
            .iter()
            .find(|platform| platform.name == "Raspberry Pi RP3A0")
            .expect("the Zero 2 W platform should be present in the platform catalog");

        assert!(platform.group == Group::SingleBoardComputer);
        assert!(platform.tier == Tier::BringUp);
        assert!(platform.icon == Some("raspberrypi"));
    }

    #[test]
    fn single_board_computers_are_listed_before_microcontrollers() {
        let single_board_computers = GROUPS
            .iter()
            .position(|group| *group == Group::SingleBoardComputer)
            .expect("single-board computers should be listed");
        let microcontrollers = GROUPS
            .iter()
            .position(|group| *group == Group::Microcontroller)
            .expect("microcontrollers should be listed");

        assert!(single_board_computers < microcontrollers);
    }

    #[test]
    fn riscv_is_presented_as_a_shipping_platform() {
        let platform = PLATFORMS
            .iter()
            .find(|platform| platform.name == "RISC-V")
            .expect("RISC-V should remain in the platform catalog");

        assert!(platform.group == Group::Microcontroller);
        assert!(platform.tier == Tier::Shipping);
        assert!(platform.icon == Some("riscv"));
        assert!(
            LANDING_PLATFORM_CHIPS
                .iter()
                .any(|chip| chip.name == "RISC-V" && chip.icon == Some("riscv")),
            "RISC-V should remain in the homepage platform marquee"
        );
    }
}
