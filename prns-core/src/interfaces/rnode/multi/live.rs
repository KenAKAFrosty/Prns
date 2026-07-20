use crate::interfaces::kiss_framing;
use crate::interfaces::PacketPhyStats;

use super::{
    ConfiguredRadio, DevicePlatform, PacketPhyState, RadioConfig, VPort, CMD_SELECT_INTERFACE,
    MAX_SUBINTERFACES,
};
use crate::interfaces::rnode::protocol;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HardwareError {
    RadioInitialization,
    Transmit,
    EepromLocked,
    Unknown(Option<u8>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveError {
    Hardware(HardwareError),
    Esp32Reset,
}

#[derive(Debug, PartialEq, Eq)]
pub enum LiveCommand<'a> {
    Data {
        vport: VPort,
        payload: &'a [u8],
        phy: PacketPhyStats,
    },
    AllRadiosReady,
    Consumed,
    Failed(LiveError),
}

pub struct LiveProtocol {
    selected: VPort,
    platform: Option<DevicePlatform>,
    radios: [Option<RadioConfig>; MAX_SUBINTERFACES],
    packet_phy: [PacketPhyState; MAX_SUBINTERFACES],
}

impl LiveProtocol {
    #[must_use]
    pub fn new(
        radios: impl IntoIterator<Item = ConfiguredRadio>,
        platform: Option<DevicePlatform>,
    ) -> Self {
        let mut configured = [None; MAX_SUBINTERFACES];
        for radio in radios {
            configured[radio.vport.index()] = Some(radio.radio);
        }
        Self {
            selected: VPort::ZERO,
            platform,
            radios: configured,
            packet_phy: ::core::array::from_fn(|_| PacketPhyState::default()),
        }
    }

    pub fn set_platform(&mut self, platform: Option<DevicePlatform>) {
        self.platform = platform;
    }

    pub fn apply<'a>(&mut self, command: u8, payload: &'a [u8]) -> LiveCommand<'a> {
        match command {
            CMD_SELECT_INTERFACE => {
                if let Some(vport) = payload.first().and_then(|value| VPort::new(*value)) {
                    self.selected = vport;
                }
                LiveCommand::Consumed
            }
            protocol::CMD_DATA if payload.is_empty() => LiveCommand::Consumed,
            protocol::CMD_DATA => LiveCommand::Data {
                vport: self.selected,
                payload,
                phy: self.packet_phy[self.selected.index()].take_for_data(),
            },
            kiss_framing::CMD_READY => LiveCommand::AllRadiosReady,
            protocol::CMD_PLATFORM => {
                self.platform = payload
                    .first()
                    .copied()
                    .map(DevicePlatform::from_device_report);
                LiveCommand::Consumed
            }
            protocol::CMD_ERROR => {
                LiveCommand::Failed(LiveError::Hardware(hardware_error(payload)))
            }
            protocol::CMD_RESET
                if self.platform == Some(DevicePlatform::Esp32)
                    && payload.first() == Some(&protocol::RESET_RESP) =>
            {
                LiveCommand::Failed(LiveError::Esp32Reset)
            }
            _ => {
                if let Some(radio) = self.radios[self.selected.index()] {
                    self.packet_phy[self.selected.index()].apply(command, payload, radio);
                }
                LiveCommand::Consumed
            }
        }
    }
}

fn hardware_error(payload: &[u8]) -> HardwareError {
    match payload.first().copied() {
        Some(protocol::ERROR_INIT_RADIO) => HardwareError::RadioInitialization,
        Some(protocol::ERROR_TX_FAILED) => HardwareError::Transmit,
        Some(protocol::ERROR_EEPROM_LOCKED) => HardwareError::EepromLocked,
        other => HardwareError::Unknown(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interfaces::rnode::multi::{RadioConfigInput, CMD_SELECT_INTERFACE};
    use crate::interfaces::RssiDbm;

    fn configured(vport: u8) -> ConfiguredRadio {
        ConfiguredRadio {
            vport: VPort::new(vport).unwrap(),
            radio: RadioConfig::new(RadioConfigInput {
                frequency_hz: 868_000_000,
                bandwidth_hz: 125_000,
                tx_power_dbm: 7,
                spreading_factor: 8,
                coding_rate: 5,
                airtime_limit_short_centi_percent: None,
                airtime_limit_long_centi_percent: None,
            })
            .unwrap(),
        }
    }

    #[test]
    fn selected_radio_owns_its_packet_telemetry() {
        let mut protocol = LiveProtocol::new([configured(0), configured(1)], None);
        let _ = protocol.apply(CMD_SELECT_INTERFACE, &[1]);
        let _ = protocol.apply(protocol::CMD_STAT_RSSI, &[100]);
        assert_eq!(
            protocol.apply(protocol::CMD_DATA, b"packet"),
            LiveCommand::Data {
                vport: VPort::new(1).unwrap(),
                payload: b"packet",
                phy: PacketPhyStats {
                    rssi: Some(RssiDbm::new(-57)),
                    ..PacketPhyStats::default()
                }
            }
        );
    }

    #[test]
    fn hardware_failures_and_esp32_resets_are_typed() {
        let mut protocol = LiveProtocol::new([configured(0)], Some(DevicePlatform::Esp32));
        assert_eq!(
            protocol.apply(protocol::CMD_ERROR, &[protocol::ERROR_TX_FAILED]),
            LiveCommand::Failed(LiveError::Hardware(HardwareError::Transmit))
        );
        assert_eq!(
            protocol.apply(protocol::CMD_RESET, &[protocol::RESET_RESP]),
            LiveCommand::Failed(LiveError::Esp32Reset)
        );
    }
}
