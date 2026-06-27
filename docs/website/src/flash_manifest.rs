#[derive(Clone, Copy, PartialEq)]
#[allow(dead_code)]
pub enum FlashArtifactState {
    Published,
    ArtifactPending,
}

impl FlashArtifactState {
    pub fn label(self) -> &'static str {
        match self {
            FlashArtifactState::Published => "Published",
            FlashArtifactState::ArtifactPending => "Artifact pending",
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum FlashTransport {
    EspWebSerial,
    Uf2MassStorage,
}

impl FlashTransport {
    pub fn action_label(self) -> &'static str {
        match self {
            FlashTransport::EspWebSerial => "Connect and flash",
            FlashTransport::Uf2MassStorage => "Download UF2",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            FlashTransport::EspWebSerial => "Web Serial",
            FlashTransport::Uf2MassStorage => "UF2 mass storage",
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
pub enum FlashArtifactFormat {
    EspBin,
    Uf2,
}

#[derive(Clone, Copy, PartialEq)]
#[allow(dead_code)]
pub enum EmbeddedPolicy {
    HostedOnly,
    Bundled,
}

#[derive(Clone, Copy, PartialEq)]
pub struct FlashArtifactRecord {
    pub board_slug: &'static str,
    pub state: FlashArtifactState,
    pub transport: FlashTransport,
    pub format: FlashArtifactFormat,
    pub release_channel: &'static str,
    pub version: &'static str,
    pub artifact_path: Option<&'static str>,
    pub artifact_sha256: Option<&'static str>,
    pub artifact_size: Option<u64>,
    pub local_command: &'static str,
    pub browser_support: &'static str,
    pub embedded_policy: EmbeddedPolicy,
    pub summary: &'static str,
    pub steps: &'static [&'static str],
}

impl FlashArtifactRecord {
    pub fn web_action_enabled(self, embedded_site: bool) -> bool {
        matches!(self.state, FlashArtifactState::Published)
            && (!embedded_site || matches!(self.embedded_policy, EmbeddedPolicy::Bundled))
    }

    pub fn download_path(self, embedded_site: bool) -> Option<&'static str> {
        if self.web_action_enabled(embedded_site)
            && matches!(self.transport, FlashTransport::Uf2MassStorage)
        {
            self.artifact_path
        } else {
            None
        }
    }

    pub fn esp_web_manifest_path(self, embedded_site: bool) -> Option<String> {
        if self.web_action_enabled(embedded_site)
            && matches!(self.transport, FlashTransport::EspWebSerial)
        {
            self.artifact_path.and_then(|path| {
                let parent = path.rsplit_once('/')?.0;
                Some(format!("{parent}/manifest.json"))
            })
        } else {
            None
        }
    }

    pub fn status_note(self, embedded_site: bool) -> &'static str {
        if embedded_site && matches!(self.embedded_policy, EmbeddedPolicy::HostedOnly) {
            "This embedded copy cannot serve hosted firmware. Build this repo locally and flash the board."
        } else if matches!(self.state, FlashArtifactState::ArtifactPending) {
            "Firmware artifact is not published yet."
        } else if matches!(self.transport, FlashTransport::Uf2MassStorage) {
            "Download the UF2, mount the board's bootloader drive, and copy the file over."
        } else if matches!(self.transport, FlashTransport::EspWebSerial) {
            "Your browser will ask to connect. The dialog may present as \"Bluetooth\" but will work with your USB cable."
        } else {
            "Ready."
        }
    }

    pub fn action_label(self, embedded_site: bool) -> &'static str {
        if embedded_site && matches!(self.embedded_policy, EmbeddedPolicy::HostedOnly) {
            "Open online flasher"
        } else if matches!(self.state, FlashArtifactState::ArtifactPending) {
            "Artifact pending"
        } else {
            self.transport.action_label()
        }
    }
}

pub mod generated {
    include!(concat!(env!("OUT_DIR"), "/flash_manifest.rs"));
}

pub use generated::FLASH_ARTIFACTS;

pub fn flash_artifact_for_board(slug: &str) -> Option<&'static FlashArtifactRecord> {
    FLASH_ARTIFACTS
        .iter()
        .find(|artifact| artifact.board_slug == slug)
}

pub fn embedded_docs_mode() -> bool {
    option_env!("PRNS_EMBEDDED_SITE")
        .map(|value| matches!(value, "1" | "true" | "TRUE" | "yes" | "YES"))
        .unwrap_or(false)
}
