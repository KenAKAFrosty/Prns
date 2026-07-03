//! The host-agnostic core of the RNode interface: the radio configuration the host writes,
//! the command codec for the RNode KISS dialect, the bring-up read-back model, and the
//! descriptor. RNode is a LoRa radio driven over a USB-serial KISS link: a host configures a
//! separate modem and pumps packets through it as `CMD_DATA` frames, unlike the embedded
//! [`lora`](crate::interfaces::lora) sibling, which *is* the modem. RNode uses the *whole*
//! command byte (radio-config echoes, telemetry, detect/version), so this rides the
//! command-aware [`KissCommandDecoder`], not the data-only KISS path. Reference: RNS
//! `RNodeInterface.py` <https://github.com/markqvist/Reticulum/blob/1.3.5/RNS/Interfaces/RNodeInterface.py>

use crate::interfaces::kiss_framing::{self, KissCommandDecoder, FEND};
use crate::interfaces::{
    AnnounceBandwidthCap, EgressCapability, IngressCapability, InterfaceCapabilities,
    InterfaceConfig, InterfaceId, InterfaceMode, TransportCapability,
};

/// RNS `RNodeInterface.HW_MTU` — the device's on-air payload ceiling and the read loop's data-frame
/// bound (`len(data_buffer) < self.HW_MTU`).
pub const RNODE_HW_MTU: usize = 508;
pub const READ_BUF_LEN: usize = 256;
/// The deframer's payload ceiling: the hardware MTU plus the access tag a frame may carry.
pub const RNODE_FRAME_LEN: usize = RNODE_HW_MTU + crate::interfaces::ifac::IFAC_MAX_SIZE;
/// The outbound scratch ceiling: a full frame, KISS-escaped worst case.
pub const FRAMED_LEN: usize = kiss_framing::max_encoded_len(RNODE_FRAME_LEN);
pub type CommandDecoder = KissCommandDecoder<RNODE_FRAME_LEN>;

// The RNode KISS command bytes. These share the framing of the KISS TNC commands but a *different*
// command namespace (here `0x01` is FREQUENCY, not the TNC's TX-delay), so they live with the RNode
// interface rather than in `kiss_framing`. Reference: `RNodeInterface.KISS`.
/// A frame carrying a Reticulum packet — the only command whose body reaches the stack.
pub const CMD_DATA: u8 = 0x00;
/// Set/echo the radio centre frequency (4-byte big-endian Hz).
pub const CMD_FREQUENCY: u8 = 0x01;
/// Set/echo the radio bandwidth (4-byte big-endian Hz).
pub const CMD_BANDWIDTH: u8 = 0x02;
/// Set/echo the radio TX power (one signed-magnitude dBm byte).
pub const CMD_TXPOWER: u8 = 0x03;
/// Set/echo the spreading factor (one byte).
pub const CMD_SF: u8 = 0x04;
/// Set/echo the coding rate denominator (one byte, `5..=8` for 4/5..4/8).
pub const CMD_CR: u8 = 0x05;
/// Set/echo the radio power state ([`RADIO_STATE_ON`]/[`RADIO_STATE_OFF`]).
pub const CMD_RADIO_STATE: u8 = 0x06;
/// Hardware-detect handshake: the host sends [`DETECT_REQ`], a real RNode answers [`DETECT_RESP`].
pub const CMD_DETECT: u8 = 0x08;
/// Short-term airtime lock (2-byte big-endian, `int(percent * 100)`).
pub const CMD_ST_ALOCK: u8 = 0x0B;
/// Long-term airtime lock (2-byte big-endian, `int(percent * 100)`).
pub const CMD_LT_ALOCK: u8 = 0x0C;
/// Firmware version response (`major`, `minor`).
pub const CMD_FW_VERSION: u8 = 0x50;
/// Hardware platform response (queried during detect; consumed, not acted on in v1).
pub const CMD_PLATFORM: u8 = 0x48;
/// MCU type response (queried during detect; consumed, not acted on in v1).
pub const CMD_MCU: u8 = 0x49;

/// Detect query payload byte the host sends under [`CMD_DETECT`].
pub const DETECT_REQ: u8 = 0x73;
/// Detect response payload byte a genuine RNode answers with.
pub const DETECT_RESP: u8 = 0x46;

/// Radio powered down.
pub const RADIO_STATE_OFF: u8 = 0x00;
/// Radio powered up and ready to carry traffic — the state bring-up drives the device to.
pub const RADIO_STATE_ON: u8 = 0x01;

/// RNS `RNodeInterface.REQUIRED_FW_VER_MAJ`/`_MIN`: the minimum firmware RNS demands. RNS panics
/// below this; we warn and carry on (firmware enforcement is deferred from v1).
pub const REQUIRED_FW_VER_MAJ: u8 = 1;
pub const REQUIRED_FW_VER_MIN: u8 = 52;

// Construction-time radio limits: the RNode/SX127x-SX126x operating envelope. RNS does not
// range-check (it relies on device echo-back validation), but a config outside these bounds
// is certainly a typo the device would reject, so we fail fast with a precise error.
pub const FREQUENCY_HZ_MIN: u64 = 137_000_000;
pub const FREQUENCY_HZ_MAX: u64 = 3_000_000_000;
pub const BANDWIDTH_HZ_MIN: u32 = 7_800;
pub const BANDWIDTH_HZ_MAX: u32 = 1_625_000;
pub const TXPOWER_DBM_MIN: i16 = 0;
pub const TXPOWER_DBM_MAX: i16 = 37;
pub const SPREADING_FACTOR_MIN: u8 = 5;
pub const SPREADING_FACTOR_MAX: u8 = 12;
pub const CODING_RATE_MIN: u8 = 5;
pub const CODING_RATE_MAX: u8 = 8;

/// The largest KISS frame [`push_command`] ever emits: a four-byte radio value, every byte escaped.
const FRAME_SCRATCH: usize = kiss_framing::max_encoded_len(4);

/// The on-air bitrate of a LoRa link, RNS `updateBitrate`:
/// `sf * ((4/cr) / (2^sf / (bw/1000))) * 1000`, which reduces to `(sf * 4 * bw) / (cr * 2^sf)`.
/// Returns 0 for a degenerate `sf`/`cr` of zero (validation rules those out for a real config).
#[must_use]
pub const fn nominal_bitrate_bps(spreading_factor: u8, coding_rate: u8, bandwidth_hz: u32) -> u32 {
    let sf = spreading_factor as u64;
    let cr = coding_rate as u64;
    let bw = bandwidth_hz as u64;
    if sf == 0 || cr == 0 {
        return 0;
    }
    ((sf * bw * 4) / ((1u64 << sf) * cr)) as u32
}

/// A radio configuration past construction-time range validation, ready to write to a device.
/// Frequency and bandwidth are whole Hz, TX power dBm, coding rate the `4/n` denominator; the
/// airtime locks stay pre-scaled as RNS's wire `int(percent * 100)` so encode is integer-only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RadioConfig {
    pub frequency_hz: u32,
    pub bandwidth_hz: u32,
    pub txpower_dbm: u8,
    pub spreading_factor: u8,
    pub coding_rate: u8,
    pub airtime_limit_short_centi: Option<u16>,
    pub airtime_limit_long_centi: Option<u16>,
}

/// The rejected field carries the out-of-range value the operator gave.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RadioConfigError {
    Frequency(u64),
    Bandwidth(u32),
    TxPower(i16),
    SpreadingFactor(u8),
    CodingRate(u8),
}

impl RadioConfig {
    /// Validate raw planner values into a `RadioConfig`, or report the first out-of-range
    /// field. Frequency arrives as `u64` (a stock RNS `frequency` can exceed `u32` Hz) and TX
    /// power as `i16` (a negative typo is caught, not wrapped); airtime locks arrive pre-scaled.
    pub fn new(
        frequency_hz: u64,
        bandwidth_hz: u32,
        txpower_dbm: i16,
        spreading_factor: u8,
        coding_rate: u8,
        airtime_limit_short_centi: Option<u16>,
        airtime_limit_long_centi: Option<u16>,
    ) -> Result<Self, RadioConfigError> {
        if !(FREQUENCY_HZ_MIN..=FREQUENCY_HZ_MAX).contains(&frequency_hz) {
            return Err(RadioConfigError::Frequency(frequency_hz));
        }
        if !(BANDWIDTH_HZ_MIN..=BANDWIDTH_HZ_MAX).contains(&bandwidth_hz) {
            return Err(RadioConfigError::Bandwidth(bandwidth_hz));
        }
        if !(TXPOWER_DBM_MIN..=TXPOWER_DBM_MAX).contains(&txpower_dbm) {
            return Err(RadioConfigError::TxPower(txpower_dbm));
        }
        if !(SPREADING_FACTOR_MIN..=SPREADING_FACTOR_MAX).contains(&spreading_factor) {
            return Err(RadioConfigError::SpreadingFactor(spreading_factor));
        }
        if !(CODING_RATE_MIN..=CODING_RATE_MAX).contains(&coding_rate) {
            return Err(RadioConfigError::CodingRate(coding_rate));
        }
        Ok(Self {
            frequency_hz: frequency_hz as u32,
            bandwidth_hz,
            txpower_dbm: txpower_dbm as u8,
            spreading_factor,
            coding_rate,
            airtime_limit_short_centi,
            airtime_limit_long_centi,
        })
    }

    /// The on-air bitrate the descriptor and airtime ledger reason from; after bring-up the
    /// device reports the same parameters back, so this matches what RNS computes.
    #[must_use]
    pub const fn nominal_bitrate_bps(&self) -> u32 {
        nominal_bitrate_bps(self.spreading_factor, self.coding_rate, self.bandwidth_hz)
    }

    /// The radio-config command stream RNS `initRadio` writes after detect: frequency, bandwidth, TX
    /// power, spreading factor, coding rate, the optional airtime locks, then radio state ON — each a
    /// KISS frame under its command byte, in this exact order.
    #[must_use]
    pub fn init_command_bytes(&self) -> std::vec::Vec<u8> {
        let mut out = std::vec::Vec::new();
        push_command(&mut out, CMD_FREQUENCY, &self.frequency_hz.to_be_bytes());
        push_command(&mut out, CMD_BANDWIDTH, &self.bandwidth_hz.to_be_bytes());
        push_command(&mut out, CMD_TXPOWER, &[self.txpower_dbm]);
        push_command(&mut out, CMD_SF, &[self.spreading_factor]);
        push_command(&mut out, CMD_CR, &[self.coding_rate]);
        if let Some(short_centi) = self.airtime_limit_short_centi {
            push_command(&mut out, CMD_ST_ALOCK, &short_centi.to_be_bytes());
        }
        if let Some(long_centi) = self.airtime_limit_long_centi {
            push_command(&mut out, CMD_LT_ALOCK, &long_centi.to_be_bytes());
        }
        push_command(&mut out, CMD_RADIO_STATE, &[RADIO_STATE_ON]);
        out
    }
}

/// Encode `payload` under `command` as a KISS frame appended to `out`. The buffer is sized to
/// the framing's worst case, so a too-small buffer drops the frame rather than panicking.
fn push_command(out: &mut std::vec::Vec<u8>, command: u8, payload: &[u8]) {
    let mut scratch = [0u8; FRAME_SCRATCH];
    if let Ok(n) = kiss_framing::encode_with_command(command, payload, &mut scratch) {
        out.extend_from_slice(&scratch[..n]);
    }
}

/// The batched hardware-detect query RNS `detect` writes: detect request, then firmware,
/// platform, and MCU queries, each its own single-`FEND`-separated frame; the thirteen bytes contain no delimiter.
#[must_use]
pub const fn detect_frames() -> [u8; 13] {
    [
        FEND,
        CMD_DETECT,
        DETECT_REQ,
        FEND,
        CMD_FW_VERSION,
        0x00,
        FEND,
        CMD_PLATFORM,
        0x00,
        FEND,
        CMD_MCU,
        0x00,
        FEND,
    ]
}

/// What the device reported during bring-up: the async rendering of RNS `readLoop`'s side
/// effects. The serve loop feeds each decoded `(command, payload)` to [`apply`](Self::apply).
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct DeviceReport {
    pub detected: bool,
    pub r_frequency: Option<u32>,
    pub r_bandwidth: Option<u32>,
    pub r_txpower: Option<u8>,
    pub r_sf: Option<u8>,
    pub r_cr: Option<u8>,
    pub r_state: Option<u8>,
    pub fw_maj: Option<u8>,
    pub fw_min: Option<u8>,
}

impl DeviceReport {
    /// Fold one decoded device frame into the report, mirroring RNS `readLoop`: frequency and
    /// bandwidth are the first four payload bytes big-endian, the firmware version two bytes,
    /// scalar radio parameters one byte. Unmodeled commands are consumed and ignored.
    pub fn apply(&mut self, command: u8, payload: &[u8]) {
        match command {
            CMD_DETECT => {
                if payload.first() == Some(&DETECT_RESP) {
                    self.detected = true;
                }
            }
            CMD_FREQUENCY => {
                if let Some(value) = be_u32(payload) {
                    self.r_frequency = Some(value);
                }
            }
            CMD_BANDWIDTH => {
                if let Some(value) = be_u32(payload) {
                    self.r_bandwidth = Some(value);
                }
            }
            CMD_TXPOWER => {
                if let Some(&byte) = payload.first() {
                    self.r_txpower = Some(byte);
                }
            }
            CMD_SF => {
                if let Some(&byte) = payload.first() {
                    self.r_sf = Some(byte);
                }
            }
            CMD_CR => {
                if let Some(&byte) = payload.first() {
                    self.r_cr = Some(byte);
                }
            }
            CMD_RADIO_STATE => {
                if let Some(&byte) = payload.first() {
                    self.r_state = Some(byte);
                }
            }
            CMD_FW_VERSION if payload.len() >= 2 => {
                self.fw_maj = Some(payload[0]);
                self.fw_min = Some(payload[1]);
            }
            _ => {}
        }
    }

    /// Whether every validated parameter has been reported: the read-back window can stop early.
    #[must_use]
    pub fn all_radio_params_present(&self) -> bool {
        self.r_frequency.is_some()
            && self.r_bandwidth.is_some()
            && self.r_txpower.is_some()
            && self.r_sf.is_some()
            && self.r_state.is_some()
    }

    /// RNS `validateRadioState`: the device-reported parameters must match the configuration —
    /// frequency within 100 Hz, bandwidth/TX-power/spreading-factor exact, and the radio powered on.
    /// As in RNS, a frequency that was never reported is not itself a mismatch, but a missing
    /// bandwidth/power/SF/state is (a `None` never equals the configured value).
    #[must_use]
    pub fn radio_validated(&self, config: &RadioConfig) -> bool {
        if let Some(reported) = self.r_frequency {
            if (i64::from(config.frequency_hz) - i64::from(reported)).abs() > 100 {
                return false;
            }
        }
        self.r_bandwidth == Some(config.bandwidth_hz)
            && self.r_txpower == Some(config.txpower_dbm)
            && self.r_sf == Some(config.spreading_factor)
            && self.r_state == Some(RADIO_STATE_ON)
    }

    /// Whether the reported firmware meets RNS's minimum, or `None` if the device never reported a
    /// version. RNS panics when this is false; the host interface only warns.
    #[must_use]
    pub fn firmware_ok(&self) -> Option<bool> {
        let (maj, min) = (self.fw_maj?, self.fw_min?);
        Some(
            maj > REQUIRED_FW_VER_MAJ || (maj == REQUIRED_FW_VER_MAJ && min >= REQUIRED_FW_VER_MIN),
        )
    }
}

/// The first four bytes of `payload` big-endian, matching RNS `b0<<24 | b1<<16 | b2<<8 | b3`;
/// `None` if fewer than four bytes have arrived.
fn be_u32(payload: &[u8]) -> Option<u32> {
    if payload.len() >= 4 {
        Some(u32::from_be_bytes([
            payload[0], payload[1], payload[2], payload[3],
        ]))
    } else {
        None
    }
}

/// The engine's view of an RNode link: a full-duplex LoRa radio that can repeat traffic out
/// its own interface, carrying its computed on-air bitrate and the 508-byte hardware MTU.
pub fn descriptor(id: InterfaceId, bitrate_bps: u32) -> InterfaceConfig {
    InterfaceConfig {
        id,
        capabilities: InterfaceCapabilities {
            ingress: IngressCapability::Enabled,
            egress: EgressCapability::Enabled(TransportCapability::SameInterfaceRepeat),
        },
        mode: InterfaceMode::Full,
        bitrate_bps: Some(bitrate_bps),
        hardware_mtu: Some(RNODE_HW_MTU),
        announce_rate_limit: None,
        announce_bandwidth_cap: AnnounceBandwidthCap::RNS_DEFAULT,
        airtime_duty_cycle: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_FRAME_CAP: usize = RNODE_FRAME_LEN;

    fn decode_commands(bytes: &[u8]) -> std::vec::Vec<(u8, std::vec::Vec<u8>)> {
        let mut decoder: KissCommandDecoder<TEST_FRAME_CAP> = KissCommandDecoder::new();
        let mut frames = std::vec::Vec::new();
        for &b in bytes {
            if let Ok(Some((command, payload))) = decoder.feed(b) {
                frames.push((command, payload.to_vec()));
            }
        }
        frames
    }

    fn sample_radio() -> RadioConfig {
        RadioConfig::new(868_000_000, 125_000, 7, 8, 5, None, None).expect("a valid radio config")
    }

    #[test]
    fn the_bitrate_matches_the_reference_formula() {
        // sf8 / cr4-5 / bw125k is the canonical RNode default: 3125 bps.
        assert_eq!(nominal_bitrate_bps(8, 5, 125_000), 3125);
        // sf7 / cr4-5 / bw500k, the fast end.
        assert_eq!(nominal_bitrate_bps(7, 5, 500_000), 21875);
        assert_eq!(sample_radio().nominal_bitrate_bps(), 3125);
    }

    #[test]
    fn a_valid_config_is_accepted_and_stored_narrowed() {
        let radio = RadioConfig::new(868_000_000, 125_000, 7, 8, 5, Some(150), Some(500))
            .expect("valid config");
        assert_eq!(radio.frequency_hz, 868_000_000);
        assert_eq!(radio.txpower_dbm, 7);
        assert_eq!(radio.airtime_limit_short_centi, Some(150));
    }

    #[test]
    fn each_out_of_range_field_is_rejected_with_its_value() {
        assert_eq!(
            RadioConfig::new(50_000_000, 125_000, 7, 8, 5, None, None),
            Err(RadioConfigError::Frequency(50_000_000))
        );
        assert_eq!(
            RadioConfig::new(868_000_000, 5_000, 7, 8, 5, None, None),
            Err(RadioConfigError::Bandwidth(5_000))
        );
        assert_eq!(
            RadioConfig::new(868_000_000, 125_000, -1, 8, 5, None, None),
            Err(RadioConfigError::TxPower(-1))
        );
        assert_eq!(
            RadioConfig::new(868_000_000, 125_000, 7, 4, 5, None, None),
            Err(RadioConfigError::SpreadingFactor(4))
        );
        assert_eq!(
            RadioConfig::new(868_000_000, 125_000, 7, 8, 9, None, None),
            Err(RadioConfigError::CodingRate(9))
        );
    }

    #[test]
    fn the_init_sequence_is_the_reference_order_of_config_commands() {
        let radio = sample_radio();
        let decoded = decode_commands(&radio.init_command_bytes());
        assert_eq!(
            decoded,
            std::vec![
                (CMD_FREQUENCY, 868_000_000u32.to_be_bytes().to_vec()),
                (CMD_BANDWIDTH, 125_000u32.to_be_bytes().to_vec()),
                (CMD_TXPOWER, std::vec![7]),
                (CMD_SF, std::vec![8]),
                (CMD_CR, std::vec![5]),
                (CMD_RADIO_STATE, std::vec![RADIO_STATE_ON]),
            ]
        );
    }

    #[test]
    fn the_airtime_locks_slot_in_before_the_radio_state_when_configured() {
        // 1.5% and 5.0% pre-scaled to centi-percent (150, 500) as the planner hands them over.
        let radio = RadioConfig::new(868_000_000, 125_000, 7, 8, 5, Some(150), Some(500))
            .expect("valid config");
        let decoded = decode_commands(&radio.init_command_bytes());
        // Each two-byte big-endian, sitting after CR and before the radio state.
        assert_eq!(decoded[5], (CMD_ST_ALOCK, 150u16.to_be_bytes().to_vec()));
        assert_eq!(decoded[6], (CMD_LT_ALOCK, 500u16.to_be_bytes().to_vec()));
        assert_eq!(decoded[7].0, CMD_RADIO_STATE);
    }

    #[test]
    fn the_detect_query_decodes_to_the_four_detect_frames() {
        assert_eq!(
            decode_commands(&detect_frames()),
            std::vec![
                (CMD_DETECT, std::vec![DETECT_REQ]),
                (CMD_FW_VERSION, std::vec![0x00]),
                (CMD_PLATFORM, std::vec![0x00]),
                (CMD_MCU, std::vec![0x00]),
            ]
        );
    }

    #[test]
    fn the_report_folds_device_echoes_into_its_radio_picture() {
        let mut report = DeviceReport::default();
        report.apply(CMD_DETECT, &[DETECT_RESP]);
        report.apply(CMD_FW_VERSION, &[1, 80]);
        report.apply(CMD_FREQUENCY, &868_000_000u32.to_be_bytes());
        report.apply(CMD_BANDWIDTH, &125_000u32.to_be_bytes());
        report.apply(CMD_TXPOWER, &[7]);
        report.apply(CMD_SF, &[8]);
        report.apply(CMD_CR, &[5]);
        report.apply(CMD_RADIO_STATE, &[RADIO_STATE_ON]);

        assert!(report.detected);
        assert_eq!(report.r_frequency, Some(868_000_000));
        assert_eq!(report.r_bandwidth, Some(125_000));
        assert_eq!(report.r_sf, Some(8));
        assert!(report.all_radio_params_present());
        assert_eq!(report.firmware_ok(), Some(true));
        assert!(report.radio_validated(&sample_radio()));
    }

    #[test]
    fn validation_tolerates_small_frequency_drift_but_not_a_real_mismatch() {
        let radio = sample_radio();
        let mut report = DeviceReport::default();
        report.apply(CMD_FREQUENCY, &(868_000_000u32 + 80).to_be_bytes());
        report.apply(CMD_BANDWIDTH, &125_000u32.to_be_bytes());
        report.apply(CMD_TXPOWER, &[7]);
        report.apply(CMD_SF, &[8]);
        report.apply(CMD_RADIO_STATE, &[RADIO_STATE_ON]);
        assert!(
            report.radio_validated(&radio),
            "80 Hz drift is within tolerance"
        );

        let mut wrong_sf = report;
        wrong_sf.apply(CMD_SF, &[9]);
        assert!(!wrong_sf.radio_validated(&radio));

        let mut off = report;
        off.apply(CMD_RADIO_STATE, &[RADIO_STATE_OFF]);
        assert!(!off.radio_validated(&radio));

        let mut far = DeviceReport::default();
        far.apply(CMD_FREQUENCY, &(868_000_000u32 + 200).to_be_bytes());
        far.apply(CMD_BANDWIDTH, &125_000u32.to_be_bytes());
        far.apply(CMD_TXPOWER, &[7]);
        far.apply(CMD_SF, &[8]);
        far.apply(CMD_RADIO_STATE, &[RADIO_STATE_ON]);
        assert!(!far.radio_validated(&radio));
    }

    #[test]
    fn outdated_firmware_is_flagged_but_unknown_firmware_is_not_a_verdict() {
        let mut old = DeviceReport::default();
        old.apply(CMD_FW_VERSION, &[1, 40]);
        assert_eq!(old.firmware_ok(), Some(false));
        assert_eq!(DeviceReport::default().firmware_ok(), None);
    }

    #[test]
    fn the_descriptor_is_a_repeating_full_radio_at_the_rnode_mtu() {
        use crate::interfaces::INTERFACE_ID_LEN;
        let d = descriptor(InterfaceId::new([0x5C; INTERFACE_ID_LEN]), 3125);
        assert!(matches!(d.mode, InterfaceMode::Full));
        assert_eq!(
            d.capabilities.egress,
            EgressCapability::Enabled(TransportCapability::SameInterfaceRepeat)
        );
        assert_eq!(d.hardware_mtu, Some(RNODE_HW_MTU));
        assert_eq!(d.bitrate_bps, Some(3125));
    }
}
