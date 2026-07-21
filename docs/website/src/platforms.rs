#[derive(Clone, Copy, PartialEq)]
pub enum Tier {
    Shipping,
    Flashable,
    BringUp,
    Roadmap,
}

impl Tier {
    pub fn chip_badge(self) -> Option<&'static str> {
        match self {
            Tier::Shipping => None,
            Tier::Flashable => Some("flashable"),
            Tier::BringUp => Some("bring-up"),
            Tier::Roadmap => Some("roadmap"),
        }
    }

    pub fn muted(self) -> bool {
        matches!(self, Tier::BringUp | Tier::Roadmap)
    }

    pub fn flash_card_class(self) -> &'static str {
        match self {
            Tier::Shipping => "flash-board-card--runtime",
            Tier::Flashable => "flash-board-card--flashable",
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
            Group::Microcontroller => "Microcontrollers",
            Group::Web => "Web & browsers",
            Group::Server => "Servers & edge",
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

pub mod board_images {
    include!(concat!(env!("OUT_DIR"), "/board_images.rs"));
}

pub mod shipping_boards {
    include!(concat!(env!("OUT_DIR"), "/shipping_boards.rs"));
}

pub use shipping_boards::SHIPPING_BOARD_TARGETS;

#[derive(Clone, Copy, PartialEq)]
pub struct BoardTarget {
    pub name: &'static str,
    pub slug: &'static str,
    pub silicon: &'static str,
    pub tier: Tier,
    pub interfaces: &'static [&'static str],
    pub icon: Option<&'static str>,
}

impl BoardTarget {
    pub fn is_flashable(&self) -> bool {
        matches!(self.tier, Tier::Flashable)
    }

    pub fn image(&self) -> Option<&'static BoardImage> {
        match self.slug {
            "heltec-v4" => Some(&board_images::HELTEC_V4),
            "t-beam-supreme" => Some(&board_images::T_BEAM_SUPREME),
            "xiao-esp32-c6" => Some(&board_images::XIAO_ESP32_C6),
            "t-echo" => Some(&board_images::T_ECHO),
            _ => None,
        }
    }
}

pub const GROUPS: &[Group] = &[
    Group::Desktop,
    Group::Mobile,
    Group::Microcontroller,
    Group::Web,
    Group::Server,
    Group::Language,
    Group::GameEngine,
];

pub const ROADMAP_BOARD_TARGETS: &[BoardTarget] = &[
    BoardTarget {
        name: "Heltec V3/V3.1",
        slug: "heltec-v3",
        silicon: "ESP32-S3 + SX1262",
        tier: Tier::BringUp,
        interfaces: &[],
        icon: Some("espressif"),
    },
    BoardTarget {
        name: "RAK WisBlock Starter Kit",
        slug: "rak-wisblock-starter-kit",
        silicon: "RAK19007 + RAK4631, nRF52840 + SX1262",
        tier: Tier::Roadmap,
        interfaces: &[],
        icon: Some("nordicsemiconductor"),
    },
    BoardTarget {
        name: "muzi works Base Duo",
        slug: "muzi-works-base-duo",
        silicon: "nRF52840 + LR1121",
        tier: Tier::Roadmap,
        interfaces: &[],
        icon: Some("nordicsemiconductor"),
    },
    BoardTarget {
        name: "Seeed Card Tracker T1000-E",
        slug: "seeed-card-tracker-t1000-e",
        silicon: "nRF52840 + LR1110",
        tier: Tier::Roadmap,
        interfaces: &[],
        icon: Some("nordicsemiconductor"),
    },
    BoardTarget {
        name: "Seeed Wio Tracker L1",
        slug: "seeed-wio-tracker-l1",
        silicon: "nRF52840 + SX1262",
        tier: Tier::Roadmap,
        interfaces: &[],
        icon: Some("nordicsemiconductor"),
    },
    BoardTarget {
        name: "Heltec Mesh Node T114",
        slug: "heltec-mesh-node-t114",
        silicon: "nRF52840 + SX1262",
        tier: Tier::Roadmap,
        interfaces: &[],
        icon: Some("nordicsemiconductor"),
    },
    BoardTarget {
        name: "LILYGO LoRa32 T3-S3",
        slug: "lilygo-lora32-t3-s3",
        silicon: "ESP32-S3 + SX1262/SX1276/SX1280/LR1121 variants",
        tier: Tier::Roadmap,
        interfaces: &[],
        icon: Some("espressif"),
    },
    BoardTarget {
        name: "B&Q Nano G2 Ultra",
        slug: "bq-nano-g2-ultra",
        silicon: "nRF52840 + SX1262",
        tier: Tier::Roadmap,
        interfaces: &[],
        icon: Some("nordicsemiconductor"),
    },
    BoardTarget {
        name: "B&Q Station G2",
        slug: "bq-station-g2",
        silicon: "ESP32-S3 + SX1262",
        tier: Tier::Roadmap,
        interfaces: &[],
        icon: Some("espressif"),
    },
];

pub fn board_target_by_slug(slug: &str) -> Option<&'static BoardTarget> {
    SHIPPING_BOARD_TARGETS
        .iter()
        .chain(ROADMAP_BOARD_TARGETS.iter())
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
        tier: Tier::BringUp,
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
        tier: Tier::BringUp,
        icon: Some("googlechrome"),
    },
    Platform {
        name: "Firefox",
        group: Group::Web,
        tier: Tier::BringUp,
        icon: Some("firefoxbrowser"),
    },
    Platform {
        name: "Safari",
        group: Group::Web,
        tier: Tier::BringUp,
        icon: Some("safari"),
    },
    Platform {
        name: "Node",
        group: Group::Server,
        tier: Tier::BringUp,
        icon: Some("nodedotjs"),
    },
    Platform {
        name: "Bun",
        group: Group::Server,
        tier: Tier::BringUp,
        icon: Some("bun"),
    },
    Platform {
        name: "Deno",
        group: Group::Server,
        tier: Tier::BringUp,
        icon: Some("deno"),
    },
    Platform {
        name: "Cloudflare Workers",
        group: Group::Server,
        tier: Tier::BringUp,
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
        tier: Tier::BringUp,
        icon: Some("typescript"),
    },
    Platform {
        name: "Kotlin",
        group: Group::Language,
        tier: Tier::BringUp,
        icon: Some("kotlin"),
    },
    Platform {
        name: "Swift",
        group: Group::Language,
        tier: Tier::BringUp,
        icon: Some("swift"),
    },
    Platform {
        name: "Python",
        group: Group::Language,
        tier: Tier::Roadmap,
        icon: Some("python"),
    },
    Platform {
        name: "Ruby",
        group: Group::Language,
        tier: Tier::Roadmap,
        icon: Some("ruby"),
    },
    Platform {
        name: "Java",
        group: Group::Language,
        tier: Tier::Roadmap,
        icon: Some("openjdk"),
    },
    Platform {
        name: ".NET",
        group: Group::Language,
        tier: Tier::Roadmap,
        icon: Some("dotnet"),
    },
    Platform {
        name: "C",
        group: Group::Language,
        tier: Tier::Roadmap,
        icon: Some("c"),
    },
    Platform {
        name: "C++",
        group: Group::Language,
        tier: Tier::Roadmap,
        icon: Some("cplusplus"),
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
        name: "nRF52840",
        icon: Some("nordicsemiconductor"),
    },
    LandingPlatformChip {
        name: "SX1262",
        icon: Some("semtech"),
    },
    LandingPlatformChip {
        name: "Heltec V4",
        icon: Some("espressif"),
    },
    LandingPlatformChip {
        name: "T-Beam Supreme",
        icon: Some("espressif"),
    },
    LandingPlatformChip {
        name: "T-Echo",
        icon: Some("nordicsemiconductor"),
    },
    LandingPlatformChip {
        name: "XIAO ESP32-C6",
        icon: Some("espressif"),
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
        name: "WebAssembly",
        icon: Some("webassembly"),
    },
    LandingPlatformChip {
        name: "Dioxus",
        icon: Some("dioxus.png"),
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
    LandingPlatformChip {
        name: "Deno",
        icon: Some("deno"),
    },
    LandingPlatformChip {
        name: "Cloudflare Workers",
        icon: Some("cloudflareworkers"),
    },
    LandingPlatformChip {
        name: "Fastly",
        icon: Some("fastly"),
    },
];
