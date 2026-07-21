use dioxus::prelude::Signal;
use prns_flash_manifest::FlashPartKind;

use crate::platforms::{BoardTarget, PreparationProfile};

#[derive(Clone, Copy, PartialEq)]
pub(super) enum WifiAction {
    Preserve,
    Configure,
    Clear,
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

#[derive(Clone, Copy)]
pub(super) struct FlasherState {
    pub(super) phase: Signal<String>,
    pub(super) status: Signal<String>,
    pub(super) progress_current: Signal<u64>,
    pub(super) progress_total: Signal<u64>,
    pub(super) prepared: Signal<bool>,
    pub(super) release: Signal<Option<ReleaseDetails>>,
    pub(super) ssid: Signal<String>,
    pub(super) password: Signal<String>,
}

pub(super) struct PreparationGuide {
    pub(super) lead: &'static str,
    pub(super) steps: &'static [&'static str],
}

pub(super) const fn preparation_guide(profile: PreparationProfile) -> PreparationGuide {
    match profile {
        PreparationProfile::EspUsbBoot => PreparationGuide {
            lead: "The flasher will try the board's cataloged automatic reset strategy first.",
            steps: &[
                "Use a USB data cable connected directly to this computer, and close serial monitors using the board.",
                "When asked, choose this board's serial port. Do not identify Heltec V4 versus T-Beam Supreme by chip name alone.",
                "If automatic connection fails, hold BOOT, tap RESET, release BOOT, then restart the complete connect-and-flash step.",
            ],
        },
        PreparationProfile::TechoUf2 => PreparationGuide {
            lead: "T-Echo uses its UF2 bootloader; the website only verifies and downloads the UF2 file.",
            steps: &[
                "Prepare the verified UF2 here before entering bootloader mode.",
                "Connect with a USB data cable and double-press RESET until the TECHOBOOT drive appears.",
                "Copy the downloaded UF2 to TECHOBOOT and wait for the copy to finish. The drive disappears when the device reboots.",
            ],
        },
    }
}

pub(super) const fn guided_steps(uf2: bool) -> &'static [&'static str] {
    if uf2 {
        &[
            "Confirm the exact T-Echo pictured above.",
            "Prepare the release; its Minisign signature, byte count, and SHA-256 are checked locally.",
            "Download the verified UF2, double-tap RESET, and copy it to TECHOBOOT.",
            "The bootloader drive disappears when the device reboots.",
        ]
    } else {
        &[
            "Confirm the exact board pictured above.",
            "Prepare the release; all sparse parts are downloaded and SHA-256 verified before USB access.",
            "Connect and choose the board's USB serial port.",
            "The chip family is checked before any write begins.",
            "Every part receives device-side MD5 verification before reset.",
        ]
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

pub(super) fn shares_esp32_s3_identity(target: &BoardTarget) -> bool {
    matches!(target.slug, "heltec-v4" | "t-beam-supreme")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platforms::board_target_by_slug;

    #[test]
    fn catalog_profiles_select_transport_specific_preparation() {
        let heltec = board_target_by_slug("heltec-v4").expect("shipping board");
        let t_echo = board_target_by_slug("t-echo").expect("shipping board");

        let esp = preparation_guide(heltec.preparation_profile.expect("flashable profile"));
        assert!(esp.steps.iter().any(|step| step.contains("hold BOOT")));
        assert!(esp.steps.iter().any(|step| step.contains("tap RESET")));

        let uf2 = preparation_guide(t_echo.preparation_profile.expect("flashable profile"));
        assert!(uf2
            .steps
            .iter()
            .any(|step| step.contains("double-press RESET")));
        assert!(uf2.steps.iter().any(|step| step.contains("TECHOBOOT")));
        assert!(uf2.lead.contains("only verifies and downloads"));
    }

    #[test]
    fn exact_board_confirmation_is_required_only_for_same_chip_pair() {
        let heltec = board_target_by_slug("heltec-v4").expect("shipping board");
        let t_beam = board_target_by_slug("t-beam-supreme").expect("shipping board");
        let xiao = board_target_by_slug("xiao-esp32-c6").expect("shipping board");
        let t_echo = board_target_by_slug("t-echo").expect("shipping board");

        assert!(shares_esp32_s3_identity(heltec));
        assert!(shares_esp32_s3_identity(t_beam));
        assert!(!shares_esp32_s3_identity(xiao));
        assert!(!shares_esp32_s3_identity(t_echo));
    }
}
