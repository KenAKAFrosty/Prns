//! Platform catalog — the single source of truth for "what Reticulum runs on".
//!
//! Shared by the landing-page marquee (which shows every name as an
//! aspirational ticker) and the dedicated `/platforms` page (which groups them
//! and marks shipping vs. roadmap). Editing this one list keeps both in sync.
//!
//! Tiers reflect the site's own framing: only the microcontrollers beyond the
//! ESP32-C6 reference are written up as "next". Everything else is presented as
//! available today. Adjust freely — it's just data.

#[derive(Clone, Copy, PartialEq)]
pub enum Tier {
    /// Runs today.
    Shipping,
    /// Roadmap / "next" — part of the aspirational north star, not yet shipping.
    Roadmap,
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
    /// Section heading on the platforms page.
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
    /// Simple Icons slug → bundled SVG at /assets/logos/<slug>.svg, tinted to
    /// the chip's text color via CSS mask. None = text-only chip (no clean logo).
    pub icon: Option<&'static str>,
}

/// Section order on the platforms page.
pub const GROUPS: &[Group] = &[
    Group::Desktop,
    Group::Mobile,
    Group::Microcontroller,
    Group::Web,
    Group::Server,
    Group::Language,
    Group::GameEngine,
];

pub const PLATFORMS: &[Platform] = &[
    Platform { name: "Linux",              group: Group::Desktop,         tier: Tier::Shipping, icon: Some("linux") },
    Platform { name: "macOS",              group: Group::Desktop,         tier: Tier::Shipping, icon: Some("apple") },
    Platform { name: "Windows",            group: Group::Desktop,         tier: Tier::Shipping, icon: Some("windows") },
    Platform { name: "Android",            group: Group::Mobile,          tier: Tier::Shipping, icon: Some("android") },
    Platform { name: "iOS",                group: Group::Mobile,          tier: Tier::Shipping, icon: Some("apple") },
    Platform { name: "ESP32-C6",           group: Group::Microcontroller, tier: Tier::Shipping, icon: Some("espressif") },
    Platform { name: "ESP32-S3",           group: Group::Microcontroller, tier: Tier::Roadmap,  icon: Some("espressif") },
    Platform { name: "nRF",                group: Group::Microcontroller, tier: Tier::Roadmap,  icon: Some("nordicsemiconductor") },
    Platform { name: "RP2040",             group: Group::Microcontroller, tier: Tier::Roadmap,  icon: Some("raspberrypi") },
    Platform { name: "STM32",              group: Group::Microcontroller, tier: Tier::Roadmap,  icon: Some("stmicroelectronics") },
    Platform { name: "RISC-V",             group: Group::Microcontroller, tier: Tier::Shipping, icon: Some("riscv") },
    Platform { name: "WebAssembly",        group: Group::Web,             tier: Tier::Shipping, icon: Some("webassembly") },
    Platform { name: "Chrome",             group: Group::Web,             tier: Tier::Shipping, icon: Some("googlechrome") },
    Platform { name: "Firefox",            group: Group::Web,             tier: Tier::Shipping, icon: Some("firefoxbrowser") },
    Platform { name: "Safari",             group: Group::Web,             tier: Tier::Shipping, icon: Some("safari") },
    Platform { name: "Node",               group: Group::Server,          tier: Tier::Shipping, icon: Some("nodedotjs") },
    Platform { name: "Bun",                group: Group::Server,          tier: Tier::Shipping, icon: Some("bun") },
    Platform { name: "Deno",               group: Group::Server,          tier: Tier::Shipping, icon: Some("deno") },
    Platform { name: "Cloudflare Workers", group: Group::Server,          tier: Tier::Shipping, icon: Some("cloudflareworkers") },
    Platform { name: "Fastly",             group: Group::Server,          tier: Tier::Shipping, icon: Some("fastly") },
    Platform { name: "Rust",               group: Group::Language,        tier: Tier::Shipping, icon: Some("rust") },
    Platform { name: "TypeScript",         group: Group::Language,        tier: Tier::Shipping, icon: Some("typescript") },
    Platform { name: "Python",             group: Group::Language,        tier: Tier::Shipping, icon: Some("python") },
    Platform { name: "Ruby",               group: Group::Language,        tier: Tier::Shipping, icon: Some("ruby") },
    Platform { name: "Kotlin",             group: Group::Language,        tier: Tier::Shipping, icon: Some("kotlin") },
    Platform { name: "Java",               group: Group::Language,        tier: Tier::Shipping, icon: Some("openjdk") },
    Platform { name: "Swift",              group: Group::Language,        tier: Tier::Shipping, icon: Some("swift") },
    Platform { name: ".NET",               group: Group::Language,        tier: Tier::Shipping, icon: Some("dotnet") },
    Platform { name: "C",                  group: Group::Language,        tier: Tier::Roadmap,  icon: Some("c") },
    Platform { name: "C++",                group: Group::Language,        tier: Tier::Roadmap,  icon: Some("cplusplus") },
    Platform { name: "Zig",                group: Group::Language,        tier: Tier::Roadmap,  icon: Some("zig") },
    Platform { name: "Unity",              group: Group::GameEngine,      tier: Tier::Shipping, icon: Some("unity") },
    Platform { name: "Godot",              group: Group::GameEngine,      tier: Tier::Shipping, icon: Some("godotengine") },
    Platform { name: "MonoGame",           group: Group::GameEngine,      tier: Tier::Shipping, icon: Some("monogame") },
];
