use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};

use dioxus::prelude::{Signal, WritableExt};
use prns_flash_manifest::{
    FlashPartKind, SoftdeviceIdentity, Uf2BoardIdMatch, Uf2BootloaderIdentity,
};
use serde::Serialize;

use crate::platforms::{
    BoardFlashTarget, BoardTarget, PreparationProfile, QUALIFICATION_BOARD_TARGETS,
    SHIPPING_BOARD_TARGETS,
};

use super::contract::BridgePhase;

#[derive(Clone, Copy, PartialEq)]
pub(super) enum WifiAction {
    Preserve,
    Configure,
    Clear,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum InstallMode {
    PreserveData,
    EraseAll,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum DestructiveConfirmation {
    Unconfirmed,
    Confirmed,
}

#[derive(Clone)]
pub(super) enum ReleaseCompatibility {
    Esp,
    Uf2(SoftdeviceIdentity),
    NrfSerialDfu(NrfSerialDfuEntry),
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum NrfSerialDfuEntry {
    TouchApplicationOrBootloader,
    ManagedApplication,
}

pub(super) const WEB_SERIAL_PROBE_SUPPORTED: &str = "supported";
pub(super) const WEB_SERIAL_PROBE_ANDROID_BLUETOOTH_ONLY: &str = "android-bluetooth-only";
pub(super) const WEB_USB_PROBE_SUPPORTED: &str = "supported";

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum WebSerialCapability {
    Checking,
    Supported,
    AndroidBluetoothOnly,
    Unavailable,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum WebUsbCapability {
    Checking,
    Supported,
    Unavailable,
}

impl WebUsbCapability {
    pub(super) fn from_probe(probe: &str) -> Self {
        match probe {
            WEB_USB_PROBE_SUPPORTED => Self::Supported,
            _ => Self::Unavailable,
        }
    }
}

impl WebSerialCapability {
    pub(super) const fn permits_usb_serial_flash(self) -> bool {
        matches!(self, Self::Supported)
    }

    pub(super) fn from_probe(probe: &str) -> Self {
        match probe {
            WEB_SERIAL_PROBE_SUPPORTED => Self::Supported,
            WEB_SERIAL_PROBE_ANDROID_BLUETOOTH_ONLY => Self::AndroidBluetoothOnly,
            _ => Self::Unavailable,
        }
    }

    pub(super) const fn blocked_explanation(self) -> Option<&'static str> {
        match self {
            Self::Checking | Self::Supported => None,
            Self::AndroidBluetoothOnly => Some(
                "Web Serial on this Android browser reaches Bluetooth serial devices only, so a USB-connected board never appears in the port picker. Use desktop Chrome, Edge, or Firefox 151 or later, or the standalone CLI.",
            ),
            Self::Unavailable => Some(
                "Web Serial is unavailable in this browser or context. Open this page over HTTPS in current desktop Chrome, Edge, or Firefox 151 or later, or use the standalone CLI.",
            ),
        }
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

    pub(super) const fn for_install_mode(install_mode: InstallMode) -> Self {
        match install_mode {
            InstallMode::PreserveData => Self::Preserve,
            InstallMode::EraseAll => Self::Clear,
        }
    }
}

impl InstallMode {
    pub(super) const fn wire(self) -> &'static str {
        match self {
            Self::PreserveData => "preserve-data",
            Self::EraseAll => "erase-all",
        }
    }
}

impl DestructiveConfirmation {
    pub(super) const fn permits(self, install_mode: InstallMode) -> bool {
        match install_mode {
            InstallMode::PreserveData => matches!(self, Self::Unconfirmed),
            InstallMode::EraseAll => matches!(self, Self::Confirmed),
        }
    }

    pub(super) const fn is_confirmed(self) -> bool {
        matches!(self, Self::Confirmed)
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
    pub(super) install_mode: Signal<InstallMode>,
    pub(super) destructive_confirmation: Signal<DestructiveConfirmation>,
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
    nrf_recovery: bool,
) -> PreparationGuide {
    match profile {
        PreparationProfile::EspUsbBoot => PreparationGuide {
            lead: "The flasher will try the board's cataloged automatic reset strategy first.",
            steps: vec![
                "Use a USB data cable connected directly to this computer, and close serial monitors using the board.".to_string(),
                "When asked, choose this board's serial port. Port names come from the chip rather than the board, and different boards can share the same chip, so if you're not sure which port is which, unplug everything else first.".to_string(),
                "If automatic connection fails, hold BOOT, tap RESET, release BOOT, then restart the complete connect-and-flash step.".to_string(),
            ],
        },
        PreparationProfile::TechoUf2 | PreparationProfile::T114Uf2 => {
            uf2_preparation_guide(target)
        }
        PreparationProfile::T096Uf2 => uf2_preparation_guide(target),
        PreparationProfile::T1000eNrfSerialDfu => {
            t1000e_preparation_guide(target, nrf_recovery)
        }
    }
}

fn t1000e_preparation_guide(target: BoardFlashTarget, recovery: bool) -> PreparationGuide {
    let BoardFlashTarget::NrfSerialDfu {
        recovery_mount_label,
        ..
    } = target
    else {
        unreachable!("the T1000-E profile requires a cataloged Nordic serial DFU target")
    };
    if recovery {
        return PreparationGuide {
            lead: "Recovery UF2 is the durable fallback when direct browser or CLI DFU is unavailable. The large button is not a standalone RESET control under Personal Hopspot.",
            steps: vec![
                "For a tracker still running Seeed or Meshtastic firmware, disconnect its USB data cable, then press and keep holding the single large button."
                    .to_string(),
                "While holding the button, quickly plug in, unplug, and plug in the cable again. Keep holding throughout; this stock-firmware sequence may take several attempts."
                    .to_string(),
                format!(
                    "Release the button when the green LED stays solid and the {recovery_mount_label} drive appears."
                ),
                format!(
                    "Select INFO_UF2.TXT from {recovery_mount_label}. It is parsed only in this browser and is not uploaded or retained."
                ),
                format!(
                    "Prepare the signed recovery image, download it, and copy it to {recovery_mount_label}. The drive disappears when the tracker reboots."
                ),
            ],
        };
    }
    PreparationGuide {
        lead: "The browser uses the exact entry path for the firmware currently running, then transfers the verified application through Nordic serial DFU. Recovery UF2 remains available below.",
        steps: vec![
            "Choose whether the tracker currently runs Seeed/Meshtastic firmware or Personal Hopspot. This selects an exact USB entry contract; it does not guess across ambiguous devices."
                .to_string(),
            "Prepare the signed release. The application and init packet are downloaded, hash-checked, and validated together in the Rust DFU core before device access."
                .to_string(),
            "Connect with a USB data cable and use the requested device picker. Personal Hopspot enters its bootloader through WebUSB; stock firmware and the serial bootloader use Web Serial."
                .to_string(),
            "If this browser has not previously been granted the bootloader serial port, use the one bounded Continue step when it appears."
                .to_string(),
            "The Rust core owns framing, acknowledgements, waits, progress, and the three-attempt retry limit. Success is reported only after the bootloader accepts the complete transfer."
                .to_string(),
        ],
    }
}

fn uf2_preparation_guide(target: BoardFlashTarget) -> PreparationGuide {
    PreparationGuide {
        lead: "This board uses its UF2 bootloader; the website reads its local descriptor and downloads the matching verified UF2 file.",
        steps: match target {
            BoardFlashTarget::Uf2MassStorage { mount_label, .. } => vec![
                format!(
                    "Connect with a USB data cable and double-press RESET until the {mount_label} drive appears."
                ),
                format!(
                    "Select INFO_UF2.TXT from {mount_label}. The file is parsed only in this browser and is not uploaded or retained."
                ),
                "Prepare the signed release after the detected SoftDevice foundation appears."
                    .to_string(),
                format!(
                    "Copy the downloaded UF2 to {mount_label} and wait for the copy to finish. The drive disappears when the device reboots."
                ),
            ],
            BoardFlashTarget::EspSerial { .. } => {
                unreachable!("the UF2 preparation profile requires a cataloged UF2 target")
            }
            BoardFlashTarget::NrfSerialDfu { .. } => {
                unreachable!("the UF2 preparation profile requires a cataloged UF2 target")
            }
        },
    }
}

pub(super) const fn guided_steps(
    target: BoardFlashTarget,
    install_mode: InstallMode,
    nrf_recovery: bool,
) -> &'static [&'static str] {
    match (target, install_mode) {
        (BoardFlashTarget::Uf2MassStorage { .. }, _) => &[
            "Confirm the exact board pictured above.",
            "Select INFO_UF2.TXT so the browser can resolve the exact SoftDevice foundation without uploading the file.",
            "Prepare the matching release artifact; its Minisign signature, UF2 structure, byte count, and SHA-256 are checked locally.",
            "Download the verified UF2, follow the board preparation instructions, and copy it to the bootloader drive.",
            "The bootloader drive disappears when the device reboots.",
        ],
        (BoardFlashTarget::NrfSerialDfu { .. }, _) if nrf_recovery => &[
            "Confirm the exact tracker and enter its recovery UF2 bootloader.",
            "Select INFO_UF2.TXT so the browser can resolve the exact SoftDevice foundation without uploading the file.",
            "Prepare the matching recovery artifact; its Minisign signature, UF2 structure, byte count, and SHA-256 are checked locally.",
            "Download the verified UF2 and copy it to the bootloader drive.",
            "The bootloader drive disappears when the tracker reboots.",
        ],
        (BoardFlashTarget::NrfSerialDfu { .. }, _) => &[
            "Confirm the exact tracker and select the entry path matching its current firmware.",
            "Prepare the release. The signed application and init packet are verified before device access.",
            "Choose the exact USB device and let the browser enter or reconnect to its serial bootloader.",
            "Transfer the application. Rust validates every acknowledgement and bounds each frame to three attempts.",
            "Success is reported only after the bootloader accepts the complete application.",
        ],
        (BoardFlashTarget::EspSerial { .. }, InstallMode::PreserveData) => &[
            "Confirm the exact board pictured above.",
            "Prepare the release. Every part is downloaded and verified before the flasher touches your device.",
            "Connect with a USB data cable and choose the board's serial port.",
            "Flash. Each part is verified again on the device, then the flasher requests a board restart.",
        ],
        (BoardFlashTarget::EspSerial { .. }, InstallMode::EraseAll) => &[
            "Confirm the exact board and the full-chip erase warning.",
            "Prepare the release. Every replacement part is downloaded and verified before the flasher touches your device.",
            "Connect with a USB data cable and choose the board's serial port.",
            "Erase and flash. Success is reported only after every replacement part verifies on the device and the final reset request completes.",
        ],
    }
}

pub(super) fn parse_uf2_selection(
    bytes: &[u8],
    target: BoardFlashTarget,
) -> Result<Uf2BootloaderIdentity, String> {
    let (board_id_match_kind, board_id) = match target {
        BoardFlashTarget::Uf2MassStorage {
            board_id_match_kind,
            board_id,
            ..
        } => (board_id_match_kind, board_id),
        BoardFlashTarget::NrfSerialDfu {
            recovery_board_id_match_kind,
            recovery_board_id,
            ..
        } => (recovery_board_id_match_kind, recovery_board_id),
        BoardFlashTarget::EspSerial { .. } => {
            return Err("An ESP target cannot use a UF2 bootloader descriptor.".to_string())
        }
    };
    let identity = Uf2BootloaderIdentity::parse(bytes).map_err(|error| error.to_string())?;
    let board_id_match = Uf2BoardIdMatch::parse(board_id_match_kind, board_id.to_string())
        .map_err(|error| error.to_string())?;
    if !identity.matches_board(&board_id_match) {
        return Err(format!(
            "INFO_UF2.TXT reports Board-ID {:?}, which does not match the selected board.",
            identity.board_id()
        ));
    }
    Ok(identity)
}

pub(super) const fn initial_status(target: BoardFlashTarget) -> &'static str {
    match target {
        BoardFlashTarget::EspSerial { .. } => {
            "Confirm the exact board before preparing its sparse serial flash plan."
        }
        BoardFlashTarget::Uf2MassStorage { .. } => {
            "Confirm the exact board before preparing its verified UF2 download."
        }
        BoardFlashTarget::NrfSerialDfu { .. } => {
            "Confirm the exact tracker and its current firmware before preparing direct Nordic DFU."
        }
    }
}

pub(super) const fn part_kind(kind: FlashPartKind) -> &'static str {
    match kind {
        FlashPartKind::Bootloader => "bootloader",
        FlashPartKind::PartitionTable => "partition-table",
        FlashPartKind::Application => "application",
        FlashPartKind::Uf2 => "uf2",
        FlashPartKind::DfuApplication => "dfu-application",
        FlashPartKind::DfuInitPacket => "dfu-init-packet",
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
        .chain(QUALIFICATION_BOARD_TARGETS.iter())
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
            false,
        );
        assert!(esp.steps.iter().any(|step| step.contains("hold BOOT")));
        assert!(esp.steps.iter().any(|step| step.contains("tap RESET")));

        let uf2 = preparation_guide(
            t_echo.preparation_profile.expect("flashable profile"),
            t_echo.flash_target.expect("flash target"),
            false,
        );
        assert!(uf2
            .steps
            .iter()
            .any(|step| step.contains("double-press RESET")));
        assert!(uf2.steps.iter().any(|step| step.contains("TECHOBOOT")));
        assert!(uf2.lead.contains("local descriptor"));
    }

    #[test]
    fn generated_catalog_owns_transport_provisioning_and_same_chip_confirmation() {
        let heltec = board_target_by_slug("heltec-v4").expect("shipping board");
        let e290 = board_target_by_slug("heltec-e290").expect("qualification board");
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
        assert_eq!(
            shares_serial_chip_identity(e290),
            cfg!(feature = "local-dev-flasher")
        );
        assert!(shares_serial_chip_identity(t_beam));
        assert!(!shares_serial_chip_identity(xiao));
        assert!(!shares_serial_chip_identity(t_echo));
    }

    #[test]
    fn t1000e_direct_and_recovery_guides_are_distinct() {
        let t1000e = board_target_by_slug("t1000-e").expect("shipping board");
        let direct = preparation_guide(
            t1000e.preparation_profile.expect("flashable profile"),
            t1000e.flash_target.expect("flash target"),
            false,
        );
        let recovery = preparation_guide(
            t1000e.preparation_profile.expect("flashable profile"),
            t1000e.flash_target.expect("flash target"),
            true,
        );

        assert!(direct.lead.contains("Nordic serial DFU"));
        assert!(direct
            .steps
            .iter()
            .any(|step| step.contains("Personal Hopspot enters its bootloader")));
        assert!(direct
            .steps
            .iter()
            .any(|step| step.contains("three-attempt retry limit")));

        assert!(recovery.lead.contains("durable fallback"));
        assert!(recovery.lead.contains("not a standalone RESET control"));
        assert!(recovery
            .steps
            .iter()
            .any(|step| step.contains("keep holding")));
        assert!(recovery
            .steps
            .iter()
            .any(|step| step.contains("plug in, unplug, and plug in")));
        assert!(recovery.steps.iter().any(|step| step.contains("green LED")));
        assert!(recovery.steps.iter().any(|step| step.contains("T1000-E")));
        assert!(!recovery
            .steps
            .iter()
            .any(|step| step.contains("double-press RESET")));
    }

    #[test]
    fn uf2_selection_requires_a_matching_board_and_supported_descriptor_shape() {
        let t_echo = board_target_by_slug("t-echo").expect("shipping board");
        let target = t_echo.flash_target.expect("flash target");
        for version in ["6.1.1", "7.3.0"] {
            let descriptor = format!(
                "UF2 Bootloader 0.6.1\r\nBoard-ID: nRF52840-TEcho-v1\r\nSoftDevice: S140 version {version}\r\n"
            );
            let identity =
                parse_uf2_selection(descriptor.as_bytes(), target).expect("valid identity");
            assert_eq!(identity.softdevice().version().as_str(), version);
        }

        let wrong_board =
            b"UF2 Bootloader 0.6.1\nBoard-ID: nRF52840-Other-v1\nSoftDevice: S140 version 7.3.0\n";
        assert!(parse_uf2_selection(wrong_board, target).is_err());
        assert!(parse_uf2_selection(b"not a descriptor", target).is_err());

        let esp = board_target_by_slug("heltec-v4").expect("shipping board");
        assert!(parse_uf2_selection(
            b"UF2 Bootloader 0.6.1\nBoard-ID: nRF52840-TEcho-v1\nSoftDevice: S140 version 7.3.0\n",
            esp.flash_target.expect("flash target"),
        )
        .is_err());
    }

    #[test]
    fn web_serial_capability_fails_closed_until_support_is_proven() {
        assert!(!WebSerialCapability::Checking.permits_usb_serial_flash());
        assert!(!WebSerialCapability::AndroidBluetoothOnly.permits_usb_serial_flash());
        assert!(!WebSerialCapability::Unavailable.permits_usb_serial_flash());
        assert!(WebSerialCapability::Supported.permits_usb_serial_flash());
    }

    #[test]
    fn install_mode_owns_confirmation_and_wifi_defaults() {
        assert!(DestructiveConfirmation::Unconfirmed.permits(InstallMode::PreserveData));
        assert!(!DestructiveConfirmation::Confirmed.permits(InstallMode::PreserveData));
        assert!(!DestructiveConfirmation::Unconfirmed.permits(InstallMode::EraseAll));
        assert!(DestructiveConfirmation::Confirmed.permits(InstallMode::EraseAll));
        assert!(WifiAction::for_install_mode(InstallMode::PreserveData) == WifiAction::Preserve);
        assert!(WifiAction::for_install_mode(InstallMode::EraseAll) == WifiAction::Clear);
        assert_eq!(InstallMode::PreserveData.wire(), "preserve-data");
        assert_eq!(InstallMode::EraseAll.wire(), "erase-all");
    }

    #[test]
    fn web_serial_probe_spellings_parse_and_unknown_probes_fail_closed() {
        assert!(matches!(
            WebSerialCapability::from_probe(WEB_SERIAL_PROBE_SUPPORTED),
            WebSerialCapability::Supported
        ));
        assert!(matches!(
            WebSerialCapability::from_probe(WEB_SERIAL_PROBE_ANDROID_BLUETOOTH_ONLY),
            WebSerialCapability::AndroidBluetoothOnly
        ));
        assert!(matches!(
            WebSerialCapability::from_probe("invented"),
            WebSerialCapability::Unavailable
        ));
        assert!(matches!(
            WebSerialCapability::from_probe(""),
            WebSerialCapability::Unavailable
        ));
        assert!(matches!(
            WebUsbCapability::from_probe(WEB_USB_PROBE_SUPPORTED),
            WebUsbCapability::Supported
        ));
        assert!(matches!(
            WebUsbCapability::from_probe("invented"),
            WebUsbCapability::Unavailable
        ));
    }

    #[test]
    fn blocked_capabilities_explain_themselves_and_working_states_stay_silent() {
        assert!(WebSerialCapability::Checking
            .blocked_explanation()
            .is_none());
        assert!(WebSerialCapability::Supported
            .blocked_explanation()
            .is_none());

        let android = WebSerialCapability::AndroidBluetoothOnly
            .blocked_explanation()
            .expect("the Android capability explains itself");
        assert!(android.contains("Bluetooth serial devices only"));
        assert!(android.contains("USB"));
        assert!(android.contains("CLI"));

        let unavailable = WebSerialCapability::Unavailable
            .blocked_explanation()
            .expect("the unavailable capability explains itself");
        assert!(unavailable.contains("Chrome, Edge, or Firefox"));
        assert!(unavailable.contains("CLI"));
    }
}
