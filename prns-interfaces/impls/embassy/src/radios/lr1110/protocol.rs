use prns_core::interfaces::lora::{
    CodingRate as ProfileCodingRate, LoRaNetwork, LoraBandwidth as ProfileBandwidth,
    Modulation as ProfileModulation, RadioProfile, SpreadingFactor as ProfileSpreadingFactor,
};

use super::Error;

pub(super) mod op {
    pub const GET_VERSION: u16 = 0x0101;
    pub const WRITE_BUFFER8: u16 = 0x0109;
    pub const READ_BUFFER8: u16 = 0x010a;
    pub const CLEAR_ERRORS: u16 = 0x010e;
    pub const CALIBRATE: u16 = 0x010f;
    pub const SET_REG_MODE: u16 = 0x0110;
    pub const SET_DIO_AS_RF_SWITCH: u16 = 0x0112;
    pub const SET_DIO_IRQ_PARAMS: u16 = 0x0113;
    pub const CLEAR_IRQ: u16 = 0x0114;
    pub const SET_TCXO_MODE: u16 = 0x0117;
    pub const SET_STANDBY: u16 = 0x011c;
    pub const GET_RX_BUFFER_STATUS: u16 = 0x0203;
    pub const GET_PACKET_STATUS: u16 = 0x0204;
    pub const GET_RSSI_INSTANTANEOUS: u16 = 0x0205;
    pub const SET_LORA_PUBLIC_NETWORK: u16 = 0x0208;
    pub const SET_RX: u16 = 0x0209;
    pub const SET_TX: u16 = 0x020a;
    pub const SET_RF_FREQUENCY: u16 = 0x020b;
    pub const SET_PACKET_TYPE: u16 = 0x020e;
    pub const SET_MODULATION_PARAMS: u16 = 0x020f;
    pub const SET_PACKET_PARAMS: u16 = 0x0210;
    pub const SET_TX_PARAMS: u16 = 0x0211;
    pub const SET_PA_CONFIG: u16 = 0x0215;
    pub const SET_RX_BOOSTED: u16 = 0x0227;
    pub const SET_LORA_SYNC_WORD: u16 = 0x022b;
}

pub(super) mod irq {
    pub const TX_DONE: u32 = 1 << 2;
    pub const RX_DONE: u32 = 1 << 3;
    pub const PREAMBLE_DETECTED: u32 = 1 << 4;
    pub const HEADER_VALID: u32 = 1 << 5;
    pub const HEADER_ERROR: u32 = 1 << 6;
    pub const CRC_ERROR: u32 = 1 << 7;
    pub const TIMEOUT: u32 = 1 << 10;
    pub const COMMAND_ERROR: u32 = 1 << 22;
    pub const ERROR: u32 = 1 << 23;
    pub const RADIO_EVENTS: u32 = TX_DONE
        | RX_DONE
        | PREAMBLE_DETECTED
        | HEADER_VALID
        | HEADER_ERROR
        | CRC_ERROR
        | TIMEOUT
        | COMMAND_ERROR
        | ERROR;
    pub const HARDWARE_ERRORS: u32 = COMMAND_ERROR | ERROR;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub(super) enum SpreadingFactor {
    Sf5 = 0x05,
    Sf6 = 0x06,
    Sf7 = 0x07,
    Sf8 = 0x08,
    Sf9 = 0x09,
    Sf10 = 0x0a,
    Sf11 = 0x0b,
    Sf12 = 0x0c,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub(super) enum Bandwidth {
    Bw125 = 0x04,
    Bw250 = 0x05,
    Bw500 = 0x06,
}

impl Bandwidth {
    fn khz(self) -> u32 {
        match self {
            Self::Bw125 => 125,
            Self::Bw250 => 250,
            Self::Bw500 => 500,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub(super) enum CodingRate {
    Cr4_5 = 0x01,
    Cr4_6 = 0x02,
    Cr4_7 = 0x03,
    Cr4_8 = 0x04,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct LoraModulation {
    pub spreading_factor: SpreadingFactor,
    pub bandwidth: Bandwidth,
    pub coding_rate: CodingRate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub(super) enum HeaderMode {
    Explicit = 0x00,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub(super) enum PayloadCrc {
    Enabled = 0x01,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub(super) enum InvertIq {
    Standard = 0x00,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct LoraPacket {
    pub preamble_symbols: u16,
    pub header: HeaderMode,
    pub crc: PayloadCrc,
    pub invert_iq: InvertIq,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct RadioConfig {
    pub frequency_hz: u32,
    pub modulation: LoraModulation,
    pub packet: LoraPacket,
    pub network: LoRaNetwork,
    pub tx_power_dbm: i8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct FirmwareVersion(pub u16);

impl FirmwareVersion {
    pub const MODERN_SYNC_WORD_MINIMUM: Self = Self(0x0303);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum IrqEventKind {
    PreambleDetected,
    HeaderValid,
    Frame,
    HeaderError,
    CrcError,
    Timeout,
    SpuriousInterrupt,
}

pub(super) fn classify_receive_irq(flags: u32) -> Result<IrqEventKind, Error> {
    if flags & irq::HARDWARE_ERRORS != 0 {
        return Err(Error::CommandRejected);
    }
    if flags & irq::RX_DONE != 0 {
        return if flags & irq::CRC_ERROR != 0 {
            Ok(IrqEventKind::CrcError)
        } else {
            Ok(IrqEventKind::Frame)
        };
    }
    if flags & irq::HEADER_ERROR != 0 {
        return Ok(IrqEventKind::HeaderError);
    }
    if flags & irq::HEADER_VALID != 0 {
        return Ok(IrqEventKind::HeaderValid);
    }
    if flags & irq::PREAMBLE_DETECTED != 0 {
        return Ok(IrqEventKind::PreambleDetected);
    }
    if flags & irq::CRC_ERROR != 0 {
        return Ok(IrqEventKind::CrcError);
    }
    if flags & irq::TIMEOUT != 0 {
        return Ok(IrqEventKind::Timeout);
    }
    Ok(IrqEventKind::SpuriousInterrupt)
}

pub(super) const fn opcode(value: u16) -> [u8; 2] {
    value.to_be_bytes()
}

pub(super) fn command_with_u8(operation: u16, value: u8) -> [u8; 3] {
    [opcode(operation)[0], opcode(operation)[1], value]
}

pub(super) fn command_with_u32(operation: u16, value: [u8; 4]) -> [u8; 6] {
    [
        opcode(operation)[0],
        opcode(operation)[1],
        value[0],
        value[1],
        value[2],
        value[3],
    ]
}

pub(super) fn radio_config(profile: RadioProfile) -> RadioConfig {
    let ProfileModulation::Lora {
        spreading_factor,
        bandwidth,
        coding_rate,
    } = profile.modulation;
    let spreading_factor = match spreading_factor {
        ProfileSpreadingFactor::Sf5 => SpreadingFactor::Sf5,
        ProfileSpreadingFactor::Sf6 => SpreadingFactor::Sf6,
        ProfileSpreadingFactor::Sf7 => SpreadingFactor::Sf7,
        ProfileSpreadingFactor::Sf8 => SpreadingFactor::Sf8,
        ProfileSpreadingFactor::Sf9 => SpreadingFactor::Sf9,
        ProfileSpreadingFactor::Sf10 => SpreadingFactor::Sf10,
        ProfileSpreadingFactor::Sf11 => SpreadingFactor::Sf11,
        ProfileSpreadingFactor::Sf12 => SpreadingFactor::Sf12,
    };
    let bandwidth = match bandwidth {
        ProfileBandwidth::Bw125kHz => Bandwidth::Bw125,
        ProfileBandwidth::Bw250kHz => Bandwidth::Bw250,
        ProfileBandwidth::Bw500kHz => Bandwidth::Bw500,
    };
    let coding_rate = match coding_rate {
        ProfileCodingRate::Cr45 => CodingRate::Cr4_5,
        ProfileCodingRate::Cr46 => CodingRate::Cr4_6,
        ProfileCodingRate::Cr47 => CodingRate::Cr4_7,
        ProfileCodingRate::Cr48 => CodingRate::Cr4_8,
    };
    RadioConfig {
        frequency_hz: profile.frequency.hz(),
        modulation: LoraModulation {
            spreading_factor,
            bandwidth,
            coding_rate,
        },
        packet: LoraPacket {
            preamble_symbols: profile.preamble.count(),
            header: HeaderMode::Explicit,
            crc: PayloadCrc::Enabled,
            invert_iq: InvertIq::Standard,
        },
        network: LoRaNetwork::Reticulum,
        tx_power_dbm: profile.tx_power.dbm(),
    }
}

pub(super) fn lora_ldro(spreading_factor: SpreadingFactor, bandwidth: Bandwidth) -> u8 {
    u8::from((1u32 << spreading_factor as u32) > 16 * bandwidth.khz())
}

fn decode_rssi_dbm(encoded: u8) -> i16 {
    -i16::from(encoded >> 1)
}

pub(super) fn antenna_referred_rssi_dbm(encoded: u8, external_receive_gain_db: u8) -> i16 {
    decode_rssi_dbm(encoded).saturating_sub(i16::from(external_receive_gain_db))
}
