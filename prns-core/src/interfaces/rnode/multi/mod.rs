use alloc::vec::Vec;

use crate::interfaces::kiss_framing::{FEND, FESC, TFEND, TFESC};
use crate::interfaces::lora::SpreadingFactor;
use crate::interfaces::{PacketPhyStats, RssiDbm, SnrQuarterDb};

use super::{policy, protocol, FirmwareVersion};

pub mod bring_up;
pub mod live;

pub use bring_up::ConfiguredRadio;

pub const MAX_SUBINTERFACES: usize = 11;
pub const REQUIRED_FW_VERSION_MAJOR: u8 = 1;
pub const REQUIRED_FW_VERSION_MINOR: u8 = 74;
pub const CMD_SELECT_INTERFACE: u8 = 0x1f;
pub const CMD_INTERFACES: u8 = 0x71;

pub const LOW_FREQUENCY_MIN_HZ: u32 = 137_000_000;
pub const LOW_FREQUENCY_MAX_HZ: u32 = 1_000_000_000;
pub const HIGH_FREQUENCY_MIN_HZ: u32 = 2_200_000_000;
pub const HIGH_FREQUENCY_MAX_HZ: u32 = 2_600_000_000;
pub const TX_POWER_MIN_DBM: i16 = -9;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DevicePlatform {
    Avr,
    Esp32,
    Nrf52,
    Other(u8),
}

impl DevicePlatform {
    #[must_use]
    pub const fn from_device_report(value: u8) -> Self {
        match value {
            0x90 => Self::Avr,
            0x80 => Self::Esp32,
            0x70 => Self::Nrf52,
            other => Self::Other(other),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VPort(u8);

impl VPort {
    pub const ZERO: Self = Self(0);

    #[must_use]
    pub const fn new(value: u8) -> Option<Self> {
        if (value as usize) < MAX_SUBINTERFACES {
            Some(Self(value))
        } else {
            None
        }
    }

    #[must_use]
    pub const fn get(self) -> u8 {
        self.0
    }

    #[must_use]
    const fn index(self) -> usize {
        self.0 as usize
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RadioType {
    Sx126x,
    Sx127x,
    Sx128x,
}

impl RadioType {
    #[must_use]
    pub const fn from_device_report(value: u8) -> Self {
        match value {
            0x10 | 0x11 => Self::Sx126x,
            0x20 | 0x21 => Self::Sx128x,
            _ => Self::Sx127x,
        }
    }

    #[must_use]
    pub const fn supports(self, frequency: RadioFrequency) -> bool {
        matches!(
            (self, frequency.band()),
            (Self::Sx126x | Self::Sx127x, RadioBand::Low) | (Self::Sx128x, RadioBand::High)
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RadioBand {
    Low,
    High,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RadioFrequency {
    hz: u32,
    band: RadioBand,
}

impl RadioFrequency {
    #[must_use]
    pub const fn new(hz: u64) -> Option<Self> {
        if hz >= LOW_FREQUENCY_MIN_HZ as u64 && hz <= LOW_FREQUENCY_MAX_HZ as u64 {
            Some(Self {
                hz: hz as u32,
                band: RadioBand::Low,
            })
        } else if hz >= HIGH_FREQUENCY_MIN_HZ as u64 && hz <= HIGH_FREQUENCY_MAX_HZ as u64 {
            Some(Self {
                hz: hz as u32,
                band: RadioBand::High,
            })
        } else {
            None
        }
    }

    #[must_use]
    pub const fn hz(self) -> u32 {
        self.hz
    }

    #[must_use]
    pub const fn band(self) -> RadioBand {
        self.band
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RadioConfig {
    frequency: RadioFrequency,
    bandwidth_hz: u32,
    tx_power_dbm: i8,
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
    pub fn new(input: RadioConfigInput) -> Result<Self, RadioConfigError> {
        let frequency = RadioFrequency::new(input.frequency_hz)
            .ok_or(RadioConfigError::Frequency(input.frequency_hz))?;
        if !(protocol::BANDWIDTH_HZ_MIN..=protocol::BANDWIDTH_HZ_MAX).contains(&input.bandwidth_hz)
        {
            return Err(RadioConfigError::Bandwidth(input.bandwidth_hz));
        }
        if !(TX_POWER_MIN_DBM..=protocol::TXPOWER_DBM_MAX).contains(&input.tx_power_dbm) {
            return Err(RadioConfigError::TxPower(input.tx_power_dbm));
        }
        if !(protocol::SPREADING_FACTOR_MIN..=protocol::SPREADING_FACTOR_MAX)
            .contains(&input.spreading_factor)
        {
            return Err(RadioConfigError::SpreadingFactor(input.spreading_factor));
        }
        if !(protocol::CODING_RATE_MIN..=protocol::CODING_RATE_MAX).contains(&input.coding_rate) {
            return Err(RadioConfigError::CodingRate(input.coding_rate));
        }
        if let Some(value) = input.airtime_limit_short_centi_percent {
            if value > protocol::AIRTIME_LIMIT_CENTI_PERCENT_MAX {
                return Err(RadioConfigError::ShortAirtimeLimit(value));
            }
        }
        if let Some(value) = input.airtime_limit_long_centi_percent {
            if value > protocol::AIRTIME_LIMIT_CENTI_PERCENT_MAX {
                return Err(RadioConfigError::LongAirtimeLimit(value));
            }
        }
        Ok(Self {
            frequency,
            bandwidth_hz: input.bandwidth_hz,
            tx_power_dbm: input.tx_power_dbm as i8,
            spreading_factor: input.spreading_factor,
            coding_rate: input.coding_rate,
            airtime_limit_short_centi_percent: input.airtime_limit_short_centi_percent,
            airtime_limit_long_centi_percent: input.airtime_limit_long_centi_percent,
        })
    }

    #[must_use]
    pub const fn frequency(self) -> RadioFrequency {
        self.frequency
    }

    #[must_use]
    pub const fn bandwidth_hz(self) -> u32 {
        self.bandwidth_hz
    }

    #[must_use]
    pub const fn tx_power_dbm(self) -> i8 {
        self.tx_power_dbm
    }

    #[must_use]
    pub const fn spreading_factor(self) -> u8 {
        self.spreading_factor
    }

    #[must_use]
    pub const fn coding_rate(self) -> u8 {
        self.coding_rate
    }

    #[must_use]
    pub const fn airtime_limit_short_centi_percent(self) -> Option<u16> {
        self.airtime_limit_short_centi_percent
    }

    #[must_use]
    pub const fn airtime_limit_long_centi_percent(self) -> Option<u16> {
        self.airtime_limit_long_centi_percent
    }

    #[must_use]
    pub const fn nominal_bitrate_bps(self) -> u32 {
        policy::nominal_bitrate_bps(self.spreading_factor, self.coding_rate, self.bandwidth_hz)
    }

    #[must_use]
    pub fn init_command_bytes(self, vport: VPort) -> Vec<u8> {
        let mut output = Vec::new();
        append_selected_command(
            &mut output,
            vport,
            protocol::CMD_FREQUENCY,
            &self.frequency.hz().to_be_bytes(),
        );
        append_selected_command(
            &mut output,
            vport,
            protocol::CMD_BANDWIDTH,
            &self.bandwidth_hz.to_be_bytes(),
        );
        append_selected_command(
            &mut output,
            vport,
            protocol::CMD_TXPOWER,
            &[self.tx_power_dbm as u8],
        );
        append_selected_command(
            &mut output,
            vport,
            protocol::CMD_SF,
            &[self.spreading_factor],
        );
        append_selected_command(&mut output, vport, protocol::CMD_CR, &[self.coding_rate]);
        if let Some(value) = self.airtime_limit_short_centi_percent {
            append_selected_command(
                &mut output,
                vport,
                protocol::CMD_ST_ALOCK,
                &value.to_be_bytes(),
            );
        }
        if let Some(value) = self.airtime_limit_long_centi_percent {
            append_selected_command(
                &mut output,
                vport,
                protocol::CMD_LT_ALOCK,
                &value.to_be_bytes(),
            );
        }
        append_selected_command(
            &mut output,
            vport,
            protocol::CMD_RADIO_STATE,
            &[protocol::RADIO_STATE_ON],
        );
        output
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataFrameError {
    PayloadTooLarge(usize),
}

pub fn data_frame(vport: VPort, payload: &[u8]) -> Result<Vec<u8>, DataFrameError> {
    if payload.len() > protocol::RNODE_FRAME_LEN {
        return Err(DataFrameError::PayloadTooLarge(payload.len()));
    }
    let mut output = Vec::new();
    append_command(&mut output, CMD_SELECT_INTERFACE, &[vport.get()]);
    append_command(&mut output, protocol::CMD_DATA, payload);
    Ok(output)
}

#[must_use]
pub const fn detect_frames() -> [u8; 16] {
    [
        FEND,
        protocol::CMD_DETECT,
        protocol::DETECT_REQ,
        FEND,
        protocol::CMD_FW_VERSION,
        0,
        FEND,
        protocol::CMD_PLATFORM,
        0,
        FEND,
        protocol::CMD_MCU,
        0,
        FEND,
        CMD_INTERFACES,
        0,
        FEND,
    ]
}

fn append_selected_command(output: &mut Vec<u8>, vport: VPort, command: u8, payload: &[u8]) {
    append_command(output, CMD_SELECT_INTERFACE, &[vport.get()]);
    append_command(output, command, payload);
}

fn append_command(output: &mut Vec<u8>, command: u8, payload: &[u8]) {
    output.push(FEND);
    output.push(command);
    for byte in payload {
        match *byte {
            FEND => output.extend_from_slice(&[FESC, TFEND]),
            FESC => output.extend_from_slice(&[FESC, TFESC]),
            value => output.push(value),
        }
    }
    output.push(FEND);
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReportedInterfaces(Vec<RadioType>);

impl ReportedInterfaces {
    pub fn apply(&mut self, payload: &[u8]) {
        let (reports, _) = payload.as_chunks::<2>();
        for pair in reports {
            if self.0.len() == MAX_SUBINTERFACES {
                return;
            }
            self.0.push(RadioType::from_device_report(pair[1]));
        }
    }

    #[must_use]
    pub fn radio_type(&self, vport: VPort) -> Option<RadioType> {
        self.0.get(vport.index()).copied()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct RadioReport {
    frequency_hz: Option<u32>,
    bandwidth_hz: Option<u32>,
    tx_power_dbm: Option<i8>,
    spreading_factor: Option<u8>,
    coding_rate: Option<u8>,
    radio_state: Option<u8>,
}

impl RadioReport {
    fn apply(&mut self, command: u8, payload: &[u8]) {
        match command {
            protocol::CMD_FREQUENCY => self.frequency_hz = be_u32(payload),
            protocol::CMD_BANDWIDTH => self.bandwidth_hz = be_u32(payload),
            protocol::CMD_TXPOWER => {
                self.tx_power_dbm = payload.first().map(|value| i8::from_be_bytes([*value]));
            }
            protocol::CMD_SF => self.spreading_factor = payload.first().copied(),
            protocol::CMD_CR => self.coding_rate = payload.first().copied(),
            protocol::CMD_RADIO_STATE => self.radio_state = payload.first().copied(),
            _ => {}
        }
    }

    #[must_use]
    pub fn all_validated_params_present(self) -> bool {
        self.frequency_hz.is_some()
            && self.bandwidth_hz.is_some()
            && self.tx_power_dbm.is_some()
            && self.spreading_factor.is_some()
            && self.radio_state.is_some()
    }

    #[must_use]
    pub fn validates(self, config: RadioConfig) -> bool {
        if let Some(reported) = self.frequency_hz {
            if (i64::from(config.frequency().hz()) - i64::from(reported)).abs() > 100 {
                return false;
            }
        }
        self.bandwidth_hz == Some(config.bandwidth_hz())
            && self.tx_power_dbm == Some(config.tx_power_dbm())
            && self.spreading_factor == Some(config.spreading_factor())
            && self.radio_state == Some(protocol::RADIO_STATE_ON)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceReport {
    selected: VPort,
    detected: bool,
    platform: Option<DevicePlatform>,
    firmware_major: Option<u8>,
    firmware_minor: Option<u8>,
    interfaces: ReportedInterfaces,
    radios: [RadioReport; MAX_SUBINTERFACES],
}

impl Default for DeviceReport {
    fn default() -> Self {
        Self {
            selected: VPort::ZERO,
            detected: false,
            platform: None,
            firmware_major: None,
            firmware_minor: None,
            interfaces: ReportedInterfaces::default(),
            radios: [RadioReport::default(); MAX_SUBINTERFACES],
        }
    }
}

impl DeviceReport {
    pub fn apply(&mut self, command: u8, payload: &[u8]) {
        match command {
            protocol::CMD_DETECT => {
                self.detected = payload.first() == Some(&protocol::DETECT_RESP);
            }
            protocol::CMD_FW_VERSION if payload.len() >= 2 => {
                self.firmware_major = Some(payload[0]);
                self.firmware_minor = Some(payload[1]);
            }
            protocol::CMD_PLATFORM => {
                self.platform = payload
                    .first()
                    .copied()
                    .map(DevicePlatform::from_device_report);
            }
            CMD_INTERFACES => self.interfaces.apply(payload),
            CMD_SELECT_INTERFACE => {
                if let Some(vport) = payload.first().and_then(|value| VPort::new(*value)) {
                    self.selected = vport;
                }
            }
            _ => self.radios[self.selected.index()].apply(command, payload),
        }
    }

    #[must_use]
    pub const fn detected(&self) -> bool {
        self.detected
    }

    #[must_use]
    pub const fn selected(&self) -> VPort {
        self.selected
    }

    #[must_use]
    pub const fn platform(&self) -> Option<DevicePlatform> {
        self.platform
    }

    #[must_use]
    pub fn interfaces(&self) -> &ReportedInterfaces {
        &self.interfaces
    }

    #[must_use]
    pub const fn radio(&self, vport: VPort) -> RadioReport {
        self.radios[vport.index()]
    }

    #[must_use]
    pub const fn firmware_ok(&self) -> Option<bool> {
        match (self.firmware_major, self.firmware_minor) {
            (Some(major), Some(minor)) => {
                Some(major >= REQUIRED_FW_VERSION_MAJOR && minor >= REQUIRED_FW_VERSION_MINOR)
            }
            _ => None,
        }
    }

    #[must_use]
    pub const fn firmware_version(&self) -> Option<FirmwareVersion> {
        match (self.firmware_major, self.firmware_minor) {
            (Some(major), Some(minor)) => Some(FirmwareVersion { major, minor }),
            _ => None,
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PacketPhyState {
    pending: PacketPhyStats,
}

impl PacketPhyState {
    pub fn apply(&mut self, command: u8, payload: &[u8], radio: RadioConfig) {
        let Some(&byte) = payload.first() else {
            return;
        };
        match command {
            protocol::CMD_STAT_RSSI => {
                self.pending.rssi = Some(RssiDbm::new(i16::from(byte) - 157));
            }
            protocol::CMD_STAT_SNR => {
                let snr = SnrQuarterDb::new(i16::from(i8::from_be_bytes([byte])));
                self.pending.snr = Some(snr);
                self.pending.quality = SpreadingFactor::from_number(radio.spreading_factor())
                    .and_then(|spreading_factor| spreading_factor.signal_quality(snr));
            }
            _ => {}
        }
    }

    #[must_use]
    pub fn take_for_data(&mut self) -> PacketPhyStats {
        ::core::mem::take(&mut self.pending)
    }
}

fn be_u32(payload: &[u8]) -> Option<u32> {
    payload
        .get(..4)
        .and_then(|value| <[u8; 4]>::try_from(value).ok())
        .map(u32::from_be_bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interfaces::kiss_framing::KissCommandDecoder;

    fn sample_input() -> RadioConfigInput {
        RadioConfigInput {
            frequency_hz: 868_000_000,
            bandwidth_hz: 125_000,
            tx_power_dbm: -4,
            spreading_factor: 8,
            coding_rate: 5,
            airtime_limit_short_centi_percent: Some(150),
            airtime_limit_long_centi_percent: Some(500),
        }
    }

    fn sample_radio() -> RadioConfig {
        RadioConfig::new(sample_input()).expect("the sample is valid")
    }

    fn decode(bytes: &[u8]) -> std::vec::Vec<(u8, std::vec::Vec<u8>)> {
        let mut decoder: KissCommandDecoder<{ protocol::RNODE_FRAME_LEN }> =
            KissCommandDecoder::new();
        let mut frames = std::vec::Vec::new();
        for &byte in bytes {
            if let Ok(Some((command, payload))) = decoder.feed(byte) {
                frames.push((command, payload.to_vec()));
            }
        }
        frames
    }

    #[test]
    fn vports_are_exactly_the_reference_slot_range() {
        assert_eq!(VPort::new(0), Some(VPort::ZERO));
        assert_eq!(VPort::new(10).map(VPort::get), Some(10));
        assert_eq!(VPort::new(11), None);
    }

    #[test]
    fn reported_radio_types_match_the_reference_mapping_and_fallback() {
        assert_eq!(RadioType::from_device_report(0x00), RadioType::Sx127x);
        assert_eq!(RadioType::from_device_report(0x01), RadioType::Sx127x);
        assert_eq!(RadioType::from_device_report(0x02), RadioType::Sx127x);
        assert_eq!(RadioType::from_device_report(0x10), RadioType::Sx126x);
        assert_eq!(RadioType::from_device_report(0x11), RadioType::Sx126x);
        assert_eq!(RadioType::from_device_report(0x20), RadioType::Sx128x);
        assert_eq!(RadioType::from_device_report(0x21), RadioType::Sx128x);
        assert_eq!(RadioType::from_device_report(0xff), RadioType::Sx127x);
    }

    #[test]
    fn reported_platforms_preserve_the_reference_reset_distinction() {
        let mut report = DeviceReport::default();
        report.apply(protocol::CMD_PLATFORM, &[0x80]);
        assert_eq!(report.platform(), Some(DevicePlatform::Esp32));
        report.apply(protocol::CMD_PLATFORM, &[0x70]);
        assert_eq!(report.platform(), Some(DevicePlatform::Nrf52));
        report.apply(protocol::CMD_PLATFORM, &[0xff]);
        assert_eq!(report.platform(), Some(DevicePlatform::Other(0xff)));
    }

    #[test]
    fn frequencies_carry_their_hardware_band() {
        let low = RadioFrequency::new(868_000_000).expect("low-band frequency");
        let high = RadioFrequency::new(2_400_000_000).expect("high-band frequency");
        assert!(RadioType::Sx126x.supports(low));
        assert!(RadioType::Sx127x.supports(low));
        assert!(!RadioType::Sx128x.supports(low));
        assert!(RadioType::Sx128x.supports(high));
        assert!(!RadioType::Sx126x.supports(high));
        assert!(RadioFrequency::new(1_500_000_000).is_none());
    }

    #[test]
    fn radio_validation_accepts_signed_power_and_rejects_each_invalid_range() {
        assert_eq!(sample_radio().tx_power_dbm(), -4);
        assert_eq!(
            RadioConfig::new(RadioConfigInput {
                tx_power_dbm: -10,
                ..sample_input()
            }),
            Err(RadioConfigError::TxPower(-10))
        );
        assert_eq!(
            RadioConfig::new(RadioConfigInput {
                frequency_hz: 1_500_000_000,
                ..sample_input()
            }),
            Err(RadioConfigError::Frequency(1_500_000_000))
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
    fn detect_query_adds_the_reference_interface_inventory_command() {
        assert_eq!(
            detect_frames(),
            [
                0xc0, 0x08, 0x73, 0xc0, 0x50, 0x00, 0xc0, 0x48, 0x00, 0xc0, 0x49, 0x00, 0xc0, 0x71,
                0x00, 0xc0,
            ]
        );
    }

    #[test]
    fn firmware_version_is_exposed_for_actionable_runtime_errors() {
        let mut report = DeviceReport::default();
        report.apply(protocol::CMD_FW_VERSION, &[1, 73]);
        assert_eq!(report.firmware_ok(), Some(false));
        report.apply(protocol::CMD_FW_VERSION, &[1, 74]);
        assert_eq!(report.firmware_ok(), Some(true));
        assert_eq!(
            report.firmware_version(),
            Some(FirmwareVersion {
                major: 1,
                minor: 74
            })
        );
    }

    #[test]
    fn every_radio_command_is_preceded_by_its_vport_selection() {
        let vport = VPort::new(3).expect("valid vport");
        let frames = decode(&sample_radio().init_command_bytes(vport));
        let commands = frames
            .iter()
            .map(|(command, _)| *command)
            .collect::<std::vec::Vec<_>>();
        assert_eq!(
            commands,
            [
                CMD_SELECT_INTERFACE,
                protocol::CMD_FREQUENCY,
                CMD_SELECT_INTERFACE,
                protocol::CMD_BANDWIDTH,
                CMD_SELECT_INTERFACE,
                protocol::CMD_TXPOWER,
                CMD_SELECT_INTERFACE,
                protocol::CMD_SF,
                CMD_SELECT_INTERFACE,
                protocol::CMD_CR,
                CMD_SELECT_INTERFACE,
                protocol::CMD_ST_ALOCK,
                CMD_SELECT_INTERFACE,
                protocol::CMD_LT_ALOCK,
                CMD_SELECT_INTERFACE,
                protocol::CMD_RADIO_STATE,
            ]
        );
        assert_eq!(frames[4], (CMD_SELECT_INTERFACE, std::vec![3]));
        assert_eq!(frames[5], (protocol::CMD_TXPOWER, std::vec![0xfc]));
    }

    #[test]
    fn data_frames_select_the_vport_then_escape_the_packet() {
        let bytes = data_frame(VPort::new(7).expect("valid vport"), &[0xc0, 0xdb])
            .expect("the packet fits");
        assert_eq!(
            bytes,
            [0xc0, 0x1f, 0x07, 0xc0, 0xc0, 0x00, 0xdb, 0xdc, 0xdb, 0xdd, 0xc0]
        );
    }

    #[test]
    fn interface_inventory_uses_each_report_pairs_second_byte_in_vport_order() {
        let mut report = DeviceReport::default();
        report.apply(CMD_INTERFACES, &[0x00, 0x11, 0x01, 0x21, 0x02, 0x02]);
        assert_eq!(report.interfaces().len(), 3);
        assert_eq!(
            report.interfaces().radio_type(VPort::ZERO),
            Some(RadioType::Sx126x)
        );
        assert_eq!(
            report
                .interfaces()
                .radio_type(VPort::new(1).expect("valid vport")),
            Some(RadioType::Sx128x)
        );
        assert_eq!(
            report
                .interfaces()
                .radio_type(VPort::new(2).expect("valid vport")),
            Some(RadioType::Sx127x)
        );
    }

    #[test]
    fn interface_inventory_ignores_an_incomplete_trailing_report() {
        let mut interfaces = ReportedInterfaces::default();
        interfaces.apply(&[0x00, 0x11, 0x01]);
        assert_eq!(interfaces, ReportedInterfaces(vec![RadioType::Sx126x]));
    }

    #[test]
    fn radio_reports_follow_the_last_selected_vport() {
        let vport = VPort::new(4).expect("valid vport");
        let radio = sample_radio();
        let mut report = DeviceReport::default();
        report.apply(CMD_SELECT_INTERFACE, &[vport.get()]);
        report.apply(
            protocol::CMD_FREQUENCY,
            &radio.frequency().hz().to_be_bytes(),
        );
        report.apply(protocol::CMD_BANDWIDTH, &radio.bandwidth_hz().to_be_bytes());
        report.apply(protocol::CMD_TXPOWER, &[radio.tx_power_dbm() as u8]);
        report.apply(protocol::CMD_SF, &[radio.spreading_factor()]);
        report.apply(protocol::CMD_RADIO_STATE, &[protocol::RADIO_STATE_ON]);
        assert_eq!(report.selected(), vport);
        assert!(report.radio(vport).all_validated_params_present());
        assert!(report.radio(vport).validates(radio));
        assert!(!report.radio(VPort::ZERO).validates(radio));
    }

    #[test]
    fn coding_rate_is_reported_but_not_part_of_reference_readback_validation() {
        let radio = sample_radio();
        let mut report = DeviceReport::default();
        report.apply(
            protocol::CMD_FREQUENCY,
            &radio.frequency().hz().to_be_bytes(),
        );
        report.apply(protocol::CMD_BANDWIDTH, &radio.bandwidth_hz().to_be_bytes());
        report.apply(protocol::CMD_TXPOWER, &[radio.tx_power_dbm() as u8]);
        report.apply(protocol::CMD_SF, &[radio.spreading_factor()]);
        report.apply(protocol::CMD_CR, &[radio.coding_rate().saturating_add(1)]);
        report.apply(protocol::CMD_RADIO_STATE, &[protocol::RADIO_STATE_ON]);
        assert!(report.radio(VPort::ZERO).validates(radio));
    }

    #[test]
    fn firmware_check_matches_the_reference_comparison() {
        let mut report = DeviceReport::default();
        assert_eq!(report.firmware_ok(), None);
        report.apply(protocol::CMD_FW_VERSION, &[1, 73]);
        assert_eq!(report.firmware_ok(), Some(false));
        report.apply(protocol::CMD_FW_VERSION, &[1, 74]);
        assert_eq!(report.firmware_ok(), Some(true));
        report.apply(protocol::CMD_FW_VERSION, &[2, 0]);
        assert_eq!(report.firmware_ok(), Some(false));
    }
}
