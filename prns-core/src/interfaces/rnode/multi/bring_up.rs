use ::core::time::Duration;
use alloc::vec::Vec;

use crate::units::{DurationMillis, InstantMillis};

use super::{
    DevicePlatform, DeviceReport, RadioConfig, RadioFrequency, RadioType, VPort,
    REQUIRED_FW_VERSION_MAJOR, REQUIRED_FW_VERSION_MINOR,
};
use crate::interfaces::rnode::FirmwareVersion;

const DETECT_TIMEOUT: Duration = Duration::from_secs(2);
const VALIDATION_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfigureDelay(Duration);

impl ConfigureDelay {
    #[must_use]
    pub const fn new(duration: Duration) -> Self {
        Self(duration)
    }

    #[must_use]
    pub const fn duration(self) -> Duration {
        self.0
    }
}

pub const DEFAULT_CONFIGURE_DELAY: ConfigureDelay = ConfigureDelay(Duration::from_secs(2));

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfiguredRadio {
    pub vport: VPort,
    pub radio: RadioConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BringUpError {
    DetectTimedOut,
    MissingInterfaceInventory,
    MissingFirmwareVersion,
    FirmwareTooOld {
        reported: FirmwareVersion,
        required: FirmwareVersion,
    },
    MissingVPort {
        vport: VPort,
        reported_radio_count: usize,
    },
    UnsupportedFrequency {
        vport: VPort,
        radio_type: RadioType,
        frequency: RadioFrequency,
    },
    RadioMismatch {
        vport: VPort,
    },
}

#[derive(Debug, PartialEq, Eq)]
pub enum BringUpAction {
    WriteDetect([u8; 16]),
    WriteRadioConfiguration { vport: VPort, bytes: Vec<u8> },
    SleepUntil(InstantMillis),
    ReadUntil(InstantMillis),
    Complete(Option<DevicePlatform>),
    Failed(BringUpError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Start,
    AwaitingIdentity {
        deadline: InstantMillis,
    },
    ConfigureDelay {
        index: usize,
        deadline: InstantMillis,
    },
    ConfigureRadio {
        index: usize,
    },
    AwaitingRadio {
        index: usize,
        deadline: InstantMillis,
    },
    Complete,
    Failed(BringUpError),
}

pub struct BringUp {
    radios: Vec<ConfiguredRadio>,
    configure_delay: ConfigureDelay,
    report: DeviceReport,
    phase: Phase,
}

impl BringUp {
    #[must_use]
    pub fn new(radios: Vec<ConfiguredRadio>, configure_delay: ConfigureDelay) -> Self {
        Self {
            radios,
            configure_delay,
            report: DeviceReport::default(),
            phase: Phase::Start,
        }
    }

    pub fn next_action(&mut self, now: InstantMillis) -> BringUpAction {
        match self.phase {
            Phase::Start => {
                self.phase = Phase::AwaitingIdentity {
                    deadline: deadline_after(now, DETECT_TIMEOUT),
                };
                BringUpAction::WriteDetect(super::detect_frames())
            }
            Phase::AwaitingIdentity { deadline } | Phase::AwaitingRadio { deadline, .. } => {
                BringUpAction::ReadUntil(deadline)
            }
            Phase::ConfigureDelay { deadline, .. } => BringUpAction::SleepUntil(deadline),
            Phase::ConfigureRadio { index } => {
                let configured = self.radios[index];
                self.phase = Phase::AwaitingRadio {
                    index,
                    deadline: deadline_after(now, VALIDATION_TIMEOUT),
                };
                BringUpAction::WriteRadioConfiguration {
                    vport: configured.vport,
                    bytes: configured.radio.init_command_bytes(configured.vport),
                }
            }
            Phase::Complete => BringUpAction::Complete(self.report.platform()),
            Phase::Failed(error) => BringUpAction::Failed(error),
        }
    }

    pub fn apply_command(&mut self, command: u8, payload: &[u8], now: InstantMillis) {
        self.report.apply(command, payload);
        match self.phase {
            Phase::AwaitingIdentity { .. } if self.identity_report_complete() => {
                match self.validate_identity() {
                    Ok(()) => self.advance_to_radio(0, now),
                    Err(error) => self.phase = Phase::Failed(error),
                }
            }
            Phase::AwaitingRadio { index, .. }
                if self
                    .report
                    .radio(self.radios[index].vport)
                    .all_validated_params_present() =>
            {
                self.conclude_radio(index, now);
            }
            _ => {}
        }
    }

    pub fn deadline_elapsed(&mut self, now: InstantMillis) {
        match self.phase {
            Phase::AwaitingIdentity { deadline } if now >= deadline => {
                let error = if !self.report.detected() {
                    BringUpError::DetectTimedOut
                } else if self.report.interfaces().is_empty() {
                    BringUpError::MissingInterfaceInventory
                } else {
                    BringUpError::MissingFirmwareVersion
                };
                self.phase = Phase::Failed(error);
            }
            Phase::ConfigureDelay { index, deadline } if now >= deadline => {
                self.phase = Phase::ConfigureRadio { index };
            }
            Phase::AwaitingRadio { index, deadline } if now >= deadline => {
                self.conclude_radio(index, now);
            }
            _ => {}
        }
    }

    fn identity_report_complete(&self) -> bool {
        self.report.detected()
            && !self.report.interfaces().is_empty()
            && self.report.firmware_version().is_some()
    }

    fn validate_identity(&self) -> Result<(), BringUpError> {
        if self.report.firmware_ok() != Some(true) {
            let reported = self
                .report
                .firmware_version()
                .unwrap_or(FirmwareVersion { major: 0, minor: 0 });
            return Err(BringUpError::FirmwareTooOld {
                reported,
                required: FirmwareVersion {
                    major: REQUIRED_FW_VERSION_MAJOR,
                    minor: REQUIRED_FW_VERSION_MINOR,
                },
            });
        }
        for configured in &self.radios {
            let radio_type = self
                .report
                .interfaces()
                .radio_type(configured.vport)
                .ok_or(BringUpError::MissingVPort {
                    vport: configured.vport,
                    reported_radio_count: self.report.interfaces().len(),
                })?;
            if !radio_type.supports(configured.radio.frequency()) {
                return Err(BringUpError::UnsupportedFrequency {
                    vport: configured.vport,
                    radio_type,
                    frequency: configured.radio.frequency(),
                });
            }
        }
        Ok(())
    }

    fn conclude_radio(&mut self, index: usize, now: InstantMillis) {
        let configured = self.radios[index];
        if !self
            .report
            .radio(configured.vport)
            .validates(configured.radio)
        {
            self.phase = Phase::Failed(BringUpError::RadioMismatch {
                vport: configured.vport,
            });
            return;
        }
        self.advance_to_radio(index + 1, now);
    }

    fn advance_to_radio(&mut self, index: usize, now: InstantMillis) {
        if index == self.radios.len() {
            self.phase = Phase::Complete;
        } else if self.configure_delay.duration().is_zero() {
            self.phase = Phase::ConfigureRadio { index };
        } else {
            self.phase = Phase::ConfigureDelay {
                index,
                deadline: deadline_after(now, self.configure_delay.duration()),
            };
        }
    }
}

fn deadline_after(now: InstantMillis, duration: Duration) -> InstantMillis {
    now.saturating_add(DurationMillis::from_duration_saturating(duration))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interfaces::rnode::multi::RadioConfigInput;
    use crate::interfaces::rnode::protocol;

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
    fn inventory_firmware_and_each_radio_form_one_progression() {
        let configured = configured(0);
        let mut bring_up =
            BringUp::new(Vec::from([configured]), ConfigureDelay::new(Duration::ZERO));
        assert!(matches!(
            bring_up.next_action(InstantMillis(0)),
            BringUpAction::WriteDetect(_)
        ));
        bring_up.apply_command(
            protocol::CMD_DETECT,
            &[protocol::DETECT_RESP],
            InstantMillis(10),
        );
        bring_up.apply_command(protocol::CMD_FW_VERSION, &[1, 74], InstantMillis(10));
        bring_up.apply_command(super::super::CMD_INTERFACES, &[0, 0x10], InstantMillis(10));
        assert!(matches!(
            bring_up.next_action(InstantMillis(10)),
            BringUpAction::WriteRadioConfiguration {
                vport: VPort::ZERO,
                ..
            }
        ));
        bring_up.apply_command(super::super::CMD_SELECT_INTERFACE, &[0], InstantMillis(20));
        bring_up.apply_command(
            protocol::CMD_FREQUENCY,
            &868_000_000u32.to_be_bytes(),
            InstantMillis(20),
        );
        bring_up.apply_command(
            protocol::CMD_BANDWIDTH,
            &125_000u32.to_be_bytes(),
            InstantMillis(20),
        );
        bring_up.apply_command(protocol::CMD_TXPOWER, &[7], InstantMillis(20));
        bring_up.apply_command(protocol::CMD_SF, &[8], InstantMillis(20));
        bring_up.apply_command(
            protocol::CMD_RADIO_STATE,
            &[protocol::RADIO_STATE_ON],
            InstantMillis(20),
        );
        assert_eq!(
            bring_up.next_action(InstantMillis(20)),
            BringUpAction::Complete(None)
        );
    }

    #[test]
    fn reported_hardware_must_contain_and_support_every_configured_vport() {
        let mut bring_up = BringUp::new(
            Vec::from([configured(1)]),
            ConfigureDelay::new(Duration::ZERO),
        );
        let _ = bring_up.next_action(InstantMillis(0));
        bring_up.apply_command(
            protocol::CMD_DETECT,
            &[protocol::DETECT_RESP],
            InstantMillis(1),
        );
        bring_up.apply_command(protocol::CMD_FW_VERSION, &[1, 74], InstantMillis(1));
        bring_up.apply_command(super::super::CMD_INTERFACES, &[0, 0x10], InstantMillis(1));
        assert_eq!(
            bring_up.next_action(InstantMillis(1)),
            BringUpAction::Failed(BringUpError::MissingVPort {
                vport: VPort::new(1).unwrap(),
                reported_radio_count: 1,
            })
        );
    }
}
