use alloc::vec::Vec;

use super::FirmwareVersion;
use crate::interfaces::kiss_framing::{self, KissCommandDecoder, FEND};
use crate::interfaces::lora::SpreadingFactor;
use crate::interfaces::rnode::policy::{nominal_bitrate_bps, RNODE_HW_MTU};
use crate::interfaces::{PacketPhyStats, RssiDbm, SnrQuarterDb};

/// RNS `RNodeInterface.HW_MTU`, including the reference read loop's strict data-frame bound.
pub const READ_BUF_LEN: usize = 256;
pub const RNODE_FRAME_LEN: usize = RNODE_HW_MTU + crate::interfaces::IFAC_MAX_SIZE;
pub const FRAMED_LEN: usize = kiss_framing::max_encoded_len(RNODE_FRAME_LEN);
pub type CommandDecoder = KissCommandDecoder<RNODE_FRAME_LEN>;

// RNode shares KISS framing but has a different command namespace; `0x01` is frequency here, not TNC TX-delay.
pub const CMD_DATA: u8 = 0x00;
pub const CMD_FREQUENCY: u8 = 0x01;
pub const CMD_BANDWIDTH: u8 = 0x02;
pub const CMD_TXPOWER: u8 = 0x03;
pub const CMD_SF: u8 = 0x04;
pub const CMD_CR: u8 = 0x05;
pub const CMD_RADIO_STATE: u8 = 0x06;
pub const CMD_DETECT: u8 = 0x08;
pub const CMD_ST_ALOCK: u8 = 0x0B;
pub const CMD_LT_ALOCK: u8 = 0x0C;
pub const CMD_STAT_RSSI: u8 = 0x23;
pub const CMD_STAT_SNR: u8 = 0x24;
pub const CMD_FW_VERSION: u8 = 0x50;
pub const CMD_RESET: u8 = 0x55;
pub const CMD_ERROR: u8 = 0x90;
pub const CMD_PLATFORM: u8 = 0x48;
pub const CMD_MCU: u8 = 0x49;

pub const DETECT_REQ: u8 = 0x73;
pub const DETECT_RESP: u8 = 0x46;

pub const ERROR_INIT_RADIO: u8 = 0x01;
pub const ERROR_TX_FAILED: u8 = 0x02;
pub const ERROR_EEPROM_LOCKED: u8 = 0x03;
pub const RESET_RESP: u8 = 0xf8;

pub const RADIO_STATE_OFF: u8 = 0x00;
pub const RADIO_STATE_ON: u8 = 0x01;

/// RNS panics below this firmware version; the host interface warns and continues.
pub const REQUIRED_FW_VER_MAJ: u8 = 1;
pub const REQUIRED_FW_VER_MIN: u8 = 52;

const RSSI_OFFSET: i16 = 157;

// RNS relies on device echo-back validation, but values outside this radio envelope are rejected before narrowing or transmission.
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
pub const AIRTIME_LIMIT_CENTI_PERCENT_MAX: u16 = 10_000;

const FRAME_SCRATCH: usize = kiss_framing::max_encoded_len(4);

/// Frequency and bandwidth are whole hertz, transmit power is in dBm, coding rate is the `4/n` denominator, and airtime locks remain pre-scaled as RNS's wire `int(percent * 100)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RadioConfig {
    frequency_hz: u32,
    bandwidth_hz: u32,
    tx_power_dbm: u8,
    spreading_factor: u8,
    coding_rate: u8,
    airtime_limit_short_centi_percent: Option<u16>,
    airtime_limit_long_centi_percent: Option<u16>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RadioConfigInput {
    pub frequency_hz: u64,
    pub bandwidth_hz: u32,
    pub tx_power_dbm: i16,
    pub spreading_factor: u8,
    pub coding_rate: u8,
    pub airtime_limit_short_centi_percent: Option<u16>,
    pub airtime_limit_long_centi_percent: Option<u16>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RadioConfigError {
    Frequency(u64),
    Bandwidth(u32),
    TxPower(i16),
    SpreadingFactor(u8),
    CodingRate(u8),
    ShortAirtimeLimit(u16),
    LongAirtimeLimit(u16),
}

impl RadioConfig {
    /// Frequency arrives as `u64` so it is validated before narrowing; transmit power arrives as `i16` so a negative value is rejected rather than wrapped.
    pub fn new(input: RadioConfigInput) -> Result<Self, RadioConfigError> {
        let RadioConfigInput {
            frequency_hz,
            bandwidth_hz,
            tx_power_dbm,
            spreading_factor,
            coding_rate,
            airtime_limit_short_centi_percent,
            airtime_limit_long_centi_percent,
        } = input;
        if !(FREQUENCY_HZ_MIN..=FREQUENCY_HZ_MAX).contains(&frequency_hz) {
            return Err(RadioConfigError::Frequency(frequency_hz));
        }
        if !(BANDWIDTH_HZ_MIN..=BANDWIDTH_HZ_MAX).contains(&bandwidth_hz) {
            return Err(RadioConfigError::Bandwidth(bandwidth_hz));
        }
        if !(TXPOWER_DBM_MIN..=TXPOWER_DBM_MAX).contains(&tx_power_dbm) {
            return Err(RadioConfigError::TxPower(tx_power_dbm));
        }
        if !(SPREADING_FACTOR_MIN..=SPREADING_FACTOR_MAX).contains(&spreading_factor) {
            return Err(RadioConfigError::SpreadingFactor(spreading_factor));
        }
        if !(CODING_RATE_MIN..=CODING_RATE_MAX).contains(&coding_rate) {
            return Err(RadioConfigError::CodingRate(coding_rate));
        }
        if let Some(limit) = airtime_limit_short_centi_percent {
            if limit > AIRTIME_LIMIT_CENTI_PERCENT_MAX {
                return Err(RadioConfigError::ShortAirtimeLimit(limit));
            }
        }
        if let Some(limit) = airtime_limit_long_centi_percent {
            if limit > AIRTIME_LIMIT_CENTI_PERCENT_MAX {
                return Err(RadioConfigError::LongAirtimeLimit(limit));
            }
        }
        Ok(Self {
            frequency_hz: frequency_hz as u32,
            bandwidth_hz,
            tx_power_dbm: tx_power_dbm as u8,
            spreading_factor,
            coding_rate,
            airtime_limit_short_centi_percent,
            airtime_limit_long_centi_percent,
        })
    }

    #[must_use]
    pub const fn frequency_hz(&self) -> u32 {
        self.frequency_hz
    }

    #[must_use]
    pub const fn bandwidth_hz(&self) -> u32 {
        self.bandwidth_hz
    }

    #[must_use]
    pub const fn tx_power_dbm(&self) -> u8 {
        self.tx_power_dbm
    }

    #[must_use]
    pub const fn spreading_factor(&self) -> u8 {
        self.spreading_factor
    }

    #[must_use]
    pub const fn coding_rate(&self) -> u8 {
        self.coding_rate
    }

    #[must_use]
    pub const fn airtime_limit_short_centi_percent(&self) -> Option<u16> {
        self.airtime_limit_short_centi_percent
    }

    #[must_use]
    pub const fn airtime_limit_long_centi_percent(&self) -> Option<u16> {
        self.airtime_limit_long_centi_percent
    }

    #[must_use]
    pub const fn nominal_bitrate_bps(&self) -> u32 {
        nominal_bitrate_bps(self.spreading_factor, self.coding_rate, self.bandwidth_hz)
    }

    /// Emits the exact RNS `initRadio` order: frequency, bandwidth, transmit power, spreading factor, coding rate, optional airtime locks, then radio state on.
    #[must_use]
    pub fn init_command_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        push_command(&mut out, CMD_FREQUENCY, &self.frequency_hz.to_be_bytes());
        push_command(&mut out, CMD_BANDWIDTH, &self.bandwidth_hz.to_be_bytes());
        push_command(&mut out, CMD_TXPOWER, &[self.tx_power_dbm]);
        push_command(&mut out, CMD_SF, &[self.spreading_factor]);
        push_command(&mut out, CMD_CR, &[self.coding_rate]);
        if let Some(short_centi) = self.airtime_limit_short_centi_percent {
            push_command(&mut out, CMD_ST_ALOCK, &short_centi.to_be_bytes());
        }
        if let Some(long_centi) = self.airtime_limit_long_centi_percent {
            push_command(&mut out, CMD_LT_ALOCK, &long_centi.to_be_bytes());
        }
        push_command(&mut out, CMD_RADIO_STATE, &[RADIO_STATE_ON]);
        out
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PacketPhyState {
    pending: PacketPhyStats,
}

impl PacketPhyState {
    pub fn apply(&mut self, command: u8, payload: &[u8], radio: &RadioConfig) {
        let Some(&byte) = payload.first() else {
            return;
        };
        match command {
            CMD_STAT_RSSI => {
                self.pending.rssi = Some(RssiDbm::new(i16::from(byte) - RSSI_OFFSET));
            }
            CMD_STAT_SNR => {
                let snr = SnrQuarterDb::new(i16::from(i8::from_be_bytes([byte])));
                self.pending.snr = Some(snr);
                self.pending.quality = SpreadingFactor::from_number(radio.spreading_factor)
                    .and_then(|spreading_factor| spreading_factor.signal_quality(snr));
            }
            _ => {}
        }
    }

    #[must_use]
    pub fn take_for_data(&mut self) -> PacketPhyStats {
        core::mem::take(&mut self.pending)
    }
}

fn push_command(out: &mut Vec<u8>, command: u8, payload: &[u8]) {
    let mut scratch = [0u8; FRAME_SCRATCH];
    if let Ok(n) = kiss_framing::encode_with_command(command, payload, &mut scratch) {
        out.extend_from_slice(&scratch[..n]);
    }
}

/// The batched RNS hardware-detect query: detect request, firmware, platform, and MCU.
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

#[must_use]
pub const fn detect_request_frame() -> [u8; 4] {
    [FEND, CMD_DETECT, DETECT_REQ, FEND]
}

pub fn encode_data_frame(
    payload: &[u8],
    output: &mut [u8],
) -> Result<usize, kiss_framing::EncodeError> {
    kiss_framing::encode_with_command(CMD_DATA, payload, output)
}

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

    pub(super) fn clear_radio_parameters(&mut self) {
        self.r_frequency = None;
        self.r_bandwidth = None;
        self.r_txpower = None;
        self.r_sf = None;
        self.r_cr = None;
        self.r_state = None;
    }

    #[must_use]
    pub fn all_radio_params_present(&self) -> bool {
        self.r_frequency.is_some()
            && self.r_bandwidth.is_some()
            && self.r_txpower.is_some()
            && self.r_sf.is_some()
            && self.r_state.is_some()
    }

    /// RNS permits a missing frequency report but otherwise requires frequency within 100 Hz, exact bandwidth, TX power and spreading factor, and the radio powered on.
    #[must_use]
    pub fn radio_validated(&self, config: &RadioConfig) -> bool {
        if let Some(reported) = self.r_frequency {
            if (i64::from(config.frequency_hz) - i64::from(reported)).abs() > 100 {
                return false;
            }
        }
        self.r_bandwidth == Some(config.bandwidth_hz)
            && self.r_txpower == Some(config.tx_power_dbm)
            && self.r_sf == Some(config.spreading_factor)
            && self.r_state == Some(RADIO_STATE_ON)
    }

    /// RNS panics on an old reported firmware; the host interface only warns.
    #[must_use]
    pub fn firmware_ok(&self) -> Option<bool> {
        let (maj, min) = (self.fw_maj?, self.fw_min?);
        Some(
            maj > REQUIRED_FW_VER_MAJ || (maj == REQUIRED_FW_VER_MAJ && min >= REQUIRED_FW_VER_MIN),
        )
    }

    #[must_use]
    pub const fn firmware_version(&self) -> Option<FirmwareVersion> {
        match (self.fw_maj, self.fw_min) {
            (Some(major), Some(minor)) => Some(FirmwareVersion { major, minor }),
            _ => None,
        }
    }
}

fn be_u32(payload: &[u8]) -> Option<u32> {
    if payload.len() >= 4 {
        Some(u32::from_be_bytes([
            payload[0], payload[1], payload[2], payload[3],
        ]))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interfaces::rnode::policy::{descriptor, policy_for_bitrate};
    use crate::interfaces::InterfaceId;
    use crate::interfaces::SignalQualityTenthsPercent;

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

    fn sample_input() -> RadioConfigInput {
        RadioConfigInput {
            frequency_hz: 868_000_000,
            bandwidth_hz: 125_000,
            tx_power_dbm: 7,
            spreading_factor: 8,
            coding_rate: 5,
            airtime_limit_short_centi_percent: None,
            airtime_limit_long_centi_percent: None,
        }
    }

    fn sample_radio() -> RadioConfig {
        RadioConfig::new(sample_input()).expect("a valid radio config")
    }

    #[test]
    fn the_bitrate_matches_the_reference_formula() {
        assert_eq!(nominal_bitrate_bps(8, 5, 125_000), 3125);
        assert_eq!(nominal_bitrate_bps(7, 5, 500_000), 21875);
        assert_eq!(sample_radio().nominal_bitrate_bps(), 3125);
    }

    #[test]
    fn a_valid_config_is_accepted_and_stored_narrowed() {
        let radio = RadioConfig::new(RadioConfigInput {
            airtime_limit_short_centi_percent: Some(150),
            airtime_limit_long_centi_percent: Some(500),
            ..sample_input()
        })
        .expect("valid config");
        assert_eq!(radio.frequency_hz(), 868_000_000);
        assert_eq!(radio.tx_power_dbm(), 7);
        assert_eq!(radio.airtime_limit_short_centi_percent(), Some(150));
    }

    #[test]
    fn each_out_of_range_field_is_rejected_with_its_value() {
        assert_eq!(
            RadioConfig::new(RadioConfigInput {
                frequency_hz: 50_000_000,
                ..sample_input()
            }),
            Err(RadioConfigError::Frequency(50_000_000))
        );
        assert_eq!(
            RadioConfig::new(RadioConfigInput {
                bandwidth_hz: 5_000,
                ..sample_input()
            }),
            Err(RadioConfigError::Bandwidth(5_000))
        );
        assert_eq!(
            RadioConfig::new(RadioConfigInput {
                tx_power_dbm: -1,
                ..sample_input()
            }),
            Err(RadioConfigError::TxPower(-1))
        );
        assert_eq!(
            RadioConfig::new(RadioConfigInput {
                spreading_factor: 4,
                ..sample_input()
            }),
            Err(RadioConfigError::SpreadingFactor(4))
        );
        assert_eq!(
            RadioConfig::new(RadioConfigInput {
                coding_rate: 9,
                ..sample_input()
            }),
            Err(RadioConfigError::CodingRate(9))
        );
        assert_eq!(
            RadioConfig::new(RadioConfigInput {
                airtime_limit_short_centi_percent: Some(10_001),
                ..sample_input()
            }),
            Err(RadioConfigError::ShortAirtimeLimit(10_001))
        );
        assert_eq!(
            RadioConfig::new(RadioConfigInput {
                airtime_limit_long_centi_percent: Some(10_001),
                ..sample_input()
            }),
            Err(RadioConfigError::LongAirtimeLimit(10_001))
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
        let radio = RadioConfig::new(RadioConfigInput {
            airtime_limit_short_centi_percent: Some(150),
            airtime_limit_long_centi_percent: Some(500),
            ..sample_input()
        })
        .expect("valid config");
        let decoded = decode_commands(&radio.init_command_bytes());
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
    fn live_wire_frames_are_owned_by_the_rnode_codec() {
        assert_eq!(detect_request_frame(), [FEND, CMD_DETECT, DETECT_REQ, FEND]);
        let mut output = [0; 7];
        assert_eq!(
            encode_data_frame(&[FEND, kiss_framing::FESC], &mut output),
            Ok(7)
        );
        assert_eq!(
            output,
            [
                FEND,
                CMD_DATA,
                kiss_framing::FESC,
                kiss_framing::TFEND,
                kiss_framing::FESC,
                kiss_framing::TFESC,
                FEND
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
    fn packet_phy_state_binds_radio_stats_to_one_data_frame() {
        let mut state = PacketPhyState::default();
        state.apply(CMD_STAT_RSSI, &[74], &sample_radio());
        state.apply(CMD_STAT_SNR, &[0xf7], &sample_radio());

        assert_eq!(
            state.take_for_data(),
            PacketPhyStats {
                rssi: Some(RssiDbm::new(-83)),
                snr: Some(SnrQuarterDb::new(-9)),
                quality: SignalQualityTenthsPercent::new(515),
            }
        );
        assert_eq!(state.take_for_data(), PacketPhyStats::default());
    }

    #[test]
    fn packet_quality_clamps_at_the_rnode_snr_bounds() {
        let radio = sample_radio();
        let mut state = PacketPhyState::default();

        state.apply(CMD_STAT_SNR, &[0x80], &radio);
        assert_eq!(
            state.take_for_data().quality,
            SignalQualityTenthsPercent::new(0)
        );
        state.apply(CMD_STAT_SNR, &[0x7f], &radio);
        assert_eq!(
            state.take_for_data().quality,
            SignalQualityTenthsPercent::new(1_000)
        );
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
        use crate::interfaces::{
            BitrateBps, EgressCapability, InterfaceMode, TransportCapability, INTERFACE_ID_LEN,
        };
        let bitrate = BitrateBps::guess(3125);
        let d = descriptor(
            InterfaceId::new([0x5C; INTERFACE_ID_LEN]),
            policy_for_bitrate(bitrate),
        );
        assert!(matches!(d.mode, InterfaceMode::Full));
        assert_eq!(
            d.capabilities.egress,
            EgressCapability::Enabled(TransportCapability::SameInterfaceRepeat)
        );
        assert_eq!(d.hardware_mtu, Some(RNODE_HW_MTU));
        assert_eq!(d.bitrate, bitrate);
    }
}
