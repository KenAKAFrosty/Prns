use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

use dioxus::prelude::{Signal, WritableExt};
use prns_flash_manifest::FlashPartKind;

use crate::platforms::{BoardFlashTarget, BoardTarget, PreparationProfile, SHIPPING_BOARD_TARGETS};

use super::contract::BridgePhase;

#[derive(Clone, Copy, PartialEq)]
pub(super) enum WifiAction {
    Preserve,
    Configure,
    Clear,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum WebSerialCapability {
    Checking,
    Supported,
    Unavailable,
}

impl WebSerialCapability {
    pub(super) const fn permits_esp_flash(self) -> bool {
        matches!(self, Self::Supported)
    }
}

impl WifiAction {
    pub(super) const fn wire(self) -> &'static str {
        match self {
            Self::Preserve => "preserve",
            Self::Configure => "configure",
            Self::Clear => "clear",
        }
    }
}

#[derive(Clone)]
pub(super) struct ReleaseDetails {
    pub(super) version: String,
    pub(super) channel: String,
    pub(super) total: u64,
    pub(super) parts: Vec<PartDetails>,
}

#[derive(Clone)]
pub(super) struct PartDetails {
    pub(super) kind: &'static str,
    pub(super) size: u64,
    pub(super) sha256: String,
}

#[derive(Clone)]
pub(super) struct FlasherState {
    pub(super) flash_target: BoardFlashTarget,
    pub(super) phase: Signal<BridgePhase>,
    pub(super) status: Signal<String>,
    pub(super) progress_current: Signal<u64>,
    pub(super) progress_total: Signal<u64>,
    pub(super) preparation_active: Signal<bool>,
    pub(super) preparation_generation: Arc<AtomicU64>,
    pub(super) prepared: Signal<bool>,
    pub(super) release: Signal<Option<ReleaseDetails>>,
    pub(super) ssid: Signal<String>,
    pub(super) password: Signal<String>,
    pub(super) tcp_enabled: Signal<bool>,
    pub(super) tcp_target: Signal<String>,
}

impl FlasherState {
    pub(super) fn begin_preparation(&mut self) -> u64 {
        let generation = self
            .preparation_generation
            .fetch_add(1, Ordering::SeqCst)
            .wrapping_add(1);
        self.preparation_active.set(true);
        generation
    }

    pub(super) fn invalidate_preparation(&mut self) {
        self.preparation_generation.fetch_add(1, Ordering::SeqCst);
        self.preparation_active.set(false);
    }

    pub(super) fn preparation_is_current(&self, generation: u64) -> bool {
        self.preparation_generation.load(Ordering::SeqCst) == generation
    }
}

pub(super) struct PreparationGuide {
    pub(super) lead: &'static str,
    pub(super) steps: Vec<String>,
}

pub(super) fn preparation_guide(
    profile: PreparationProfile,
    target: BoardFlashTarget,
) -> PreparationGuide {
    match profile {
        PreparationProfile::EspUsbBoot => PreparationGuide {
            lead: "The flasher will try the board's cataloged automatic reset strategy first.",
            steps: vec![
                "Use a USB data cable connected directly to this computer, and close serial monitors using the board.".to_string(),
                "When asked, choose this board's serial port. Do not identify a board solely by chip family when catalog targets share one.".to_string(),
                "If automatic connection fails, hold BOOT, tap RESET, release BOOT, then restart the complete connect-and-flash step.".to_string(),
            ],
        },
        PreparationProfile::TechoUf2 => PreparationGuide {
            lead: "T-Echo uses its UF2 bootloader; the website only verifies and downloads the UF2 file.",
            steps: match target {
                BoardFlashTarget::Uf2MassStorage { mount_label } => vec![
                    "Prepare the verified UF2 here before entering bootloader mode.".to_string(),
                    format!(
                        "Connect with a USB data cable and double-press RESET until the {mount_label} drive appears."
                    ),
                    format!(
                        "Copy the downloaded UF2 to {mount_label} and wait for the copy to finish. The drive disappears when the device reboots."
                    ),
                ],
                BoardFlashTarget::EspSerial { .. } => {
                    unreachable!("the UF2 preparation profile requires a cataloged UF2 target")
                }
            },
        },
    }
}

pub(super) const fn guided_steps(target: BoardFlashTarget) -> &'static [&'static str] {
    match target {
        BoardFlashTarget::Uf2MassStorage { .. } => &[
            "Confirm the exact board pictured above.",
            "Prepare the release; its Minisign signature, byte count, and SHA-256 are checked locally.",
            "Download the verified UF2, follow the board preparation instructions, and copy it to the bootloader drive.",
            "The bootloader drive disappears when the device reboots.",
        ],
        BoardFlashTarget::EspSerial { .. } => &[
            "Confirm the exact board pictured above.",
            "Prepare the release; all sparse parts are downloaded and SHA-256 verified before USB access.",
            "Connect and choose the board's USB serial port.",
            "The chip family is checked before any write begins.",
            "Every part receives device-side MD5 verification before reset.",
        ],
    }
}

pub(super) const fn initial_status(target: BoardFlashTarget) -> &'static str {
    match target {
        BoardFlashTarget::EspSerial { .. } => {
            "Confirm the exact board before preparing its sparse serial flash plan."
        }
        BoardFlashTarget::Uf2MassStorage { .. } => {
            "Confirm the exact board before preparing its verified UF2 download."
        }
    }
}

pub(super) const fn part_kind(kind: FlashPartKind) -> &'static str {
    match kind {
        FlashPartKind::Bootloader => "bootloader",
        FlashPartKind::PartitionTable => "partition-table",
        FlashPartKind::Application => "application",
        FlashPartKind::Uf2 => "uf2",
    }
}

pub(super) fn shares_serial_chip_identity(target: &BoardTarget) -> bool {
    let Some(expected_chip) = target
        .flash_target
        .and_then(BoardFlashTarget::expected_chip)
    else {
        return false;
    };
    SHIPPING_BOARD_TARGETS
        .iter()
        .filter(|candidate| {
            candidate
                .flash_target
                .and_then(BoardFlashTarget::expected_chip)
                == Some(expected_chip)
        })
        .count()
        > 1
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platforms::board_target_by_slug;

    #[test]
    fn catalog_profiles_select_transport_specific_preparation() {
        let heltec = board_target_by_slug("heltec-v4").expect("shipping board");
        let t_echo = board_target_by_slug("t-echo").expect("shipping board");

        let esp = preparation_guide(
            heltec.preparation_profile.expect("flashable profile"),
            heltec.flash_target.expect("flash target"),
        );
        assert!(esp.steps.iter().any(|step| step.contains("hold BOOT")));
        assert!(esp.steps.iter().any(|step| step.contains("tap RESET")));

        let uf2 = preparation_guide(
            t_echo.preparation_profile.expect("flashable profile"),
            t_echo.flash_target.expect("flash target"),
        );
        assert!(uf2
            .steps
            .iter()
            .any(|step| step.contains("double-press RESET")));
        assert!(uf2.steps.iter().any(|step| step.contains("TECHOBOOT")));
        assert!(uf2.lead.contains("only verifies and downloads"));
    }

    #[test]
    fn generated_catalog_owns_transport_provisioning_and_same_chip_confirmation() {
        let heltec = board_target_by_slug("heltec-v4").expect("shipping board");
        let t_beam = board_target_by_slug("t-beam-supreme").expect("shipping board");
        let xiao = board_target_by_slug("xiao-esp32-c6").expect("shipping board");
        let t_echo = board_target_by_slug("t-echo").expect("shipping board");

        assert!(heltec.flash_target.expect("flash target").uses_web_serial());
        assert!(heltec
            .flash_target
            .expect("flash target")
            .supports_provisioning());
        assert!(heltec
            .flash_target
            .expect("flash target")
            .supports_tcp_client_provisioning());
        assert!(t_beam
            .flash_target
            .expect("flash target")
            .supports_tcp_client_provisioning());
        assert!(!xiao
            .flash_target
            .expect("flash target")
            .supports_provisioning());
        assert!(!xiao
            .flash_target
            .expect("flash target")
            .supports_tcp_client_provisioning());
        assert!(matches!(
            t_echo.flash_target.expect("flash target"),
            BoardFlashTarget::Uf2MassStorage { .. }
        ));
        assert!(shares_serial_chip_identity(heltec));
        assert!(shares_serial_chip_identity(t_beam));
        assert!(!shares_serial_chip_identity(xiao));
        assert!(!shares_serial_chip_identity(t_echo));
    }

    #[test]
    fn web_serial_capability_fails_closed_until_support_is_proven() {
        assert!(!WebSerialCapability::Checking.permits_esp_flash());
        assert!(!WebSerialCapability::Unavailable.permits_esp_flash());
        assert!(WebSerialCapability::Supported.permits_esp_flash());
    }
}
