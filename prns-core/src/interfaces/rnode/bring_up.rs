use ::core::time::Duration;
use alloc::vec::Vec;

use crate::units::{DurationMillis, InstantMillis};

use super::core::{self, DeviceReport, RadioConfig};
use super::FirmwareVersion;

const VALIDATION_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DetectTimeout(Duration);

impl DetectTimeout {
    #[must_use]
    pub const fn new(duration: Duration) -> Option<Self> {
        if duration.is_zero() {
            None
        } else {
            Some(Self(duration))
        }
    }

    #[must_use]
    pub const fn duration(self) -> Duration {
        self.0
    }
}

pub const DEFAULT_DETECT_TIMEOUT: DetectTimeout = DetectTimeout(Duration::from_secs(2));
pub const REMOTE_DETECT_TIMEOUT: DetectTimeout = DetectTimeout(Duration::from_secs(5));

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BringUpError {
    DetectTimedOut,
    RadioMismatch,
}

#[derive(Debug, PartialEq, Eq)]
pub enum BringUpAction {
    WriteDetect([u8; 13]),
    WriteRadioConfiguration {
        bytes: Vec<u8>,
        outdated_firmware: Option<FirmwareVersion>,
    },
    ReadUntil(InstantMillis),
    Complete,
    Failed(BringUpError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Start,
    AwaitingDetect { deadline: InstantMillis },
    ConfigureRadio,
    AwaitingValidation { deadline: InstantMillis },
    Complete,
    Failed(BringUpError),
}

pub struct BringUp {
    radio: RadioConfig,
    detect_timeout: DetectTimeout,
    report: DeviceReport,
    phase: Phase,
}

impl BringUp {
    #[must_use]
    pub fn new(radio: RadioConfig, detect_timeout: DetectTimeout) -> Self {
        Self {
            radio,
            detect_timeout,
            report: DeviceReport::default(),
            phase: Phase::Start,
        }
    }

    pub fn next_action(&mut self, now: InstantMillis) -> BringUpAction {
        match self.phase {
            Phase::Start => {
                self.phase = Phase::AwaitingDetect {
                    deadline: deadline_after(now, self.detect_timeout.duration()),
                };
                BringUpAction::WriteDetect(core::detect_frames())
            }
            Phase::AwaitingDetect { deadline } | Phase::AwaitingValidation { deadline } => {
                BringUpAction::ReadUntil(deadline)
            }
            Phase::ConfigureRadio => {
                self.phase = Phase::AwaitingValidation {
                    deadline: deadline_after(now, VALIDATION_TIMEOUT),
                };
                BringUpAction::WriteRadioConfiguration {
                    bytes: self.radio.init_command_bytes(),
                    outdated_firmware: self.outdated_firmware(),
                }
            }
            Phase::Complete => BringUpAction::Complete,
            Phase::Failed(error) => BringUpAction::Failed(error),
        }
    }

    pub fn apply_command(&mut self, command: u8, payload: &[u8]) {
        self.report.apply(command, payload);
        match self.phase {
            Phase::AwaitingDetect { .. } if self.report.detected => {
                self.phase = Phase::ConfigureRadio;
            }
            Phase::AwaitingValidation { .. } if self.report.all_radio_params_present() => {
                self.conclude_validation();
            }
            _ => {}
        }
    }

    pub fn deadline_elapsed(&mut self, now: InstantMillis) {
        match self.phase {
            Phase::AwaitingDetect { deadline } if now >= deadline => {
                self.phase = Phase::Failed(BringUpError::DetectTimedOut);
            }
            Phase::AwaitingValidation { deadline } if now >= deadline => {
                self.conclude_validation();
            }
            _ => {}
        }
    }

    fn conclude_validation(&mut self) {
        self.phase = if self.report.radio_validated(&self.radio) {
            Phase::Complete
        } else {
            Phase::Failed(BringUpError::RadioMismatch)
        };
    }

    fn outdated_firmware(&self) -> Option<FirmwareVersion> {
        if self.report.firmware_ok() != Some(false) {
            return None;
        }
        self.report.firmware_version()
    }
}

fn deadline_after(now: InstantMillis, duration: Duration) -> InstantMillis {
    now.saturating_add(DurationMillis::from_duration_saturating(duration))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interfaces::rnode::core::{RadioConfigInput, DETECT_RESP, RADIO_STATE_ON};

    fn radio() -> RadioConfig {
        RadioConfig::new(RadioConfigInput {
            frequency_hz: 868_000_000,
            bandwidth_hz: 125_000,
            txpower_dbm: 7,
            spreading_factor: 8,
            coding_rate: 5,
            airtime_limit_short_centi_percent: None,
            airtime_limit_long_centi_percent: None,
        })
        .unwrap()
    }

    #[test]
    fn detection_and_validation_are_one_typed_progression() {
        let mut bring_up = BringUp::new(radio(), DEFAULT_DETECT_TIMEOUT);
        assert!(matches!(
            bring_up.next_action(InstantMillis(100)),
            BringUpAction::WriteDetect(_)
        ));
        assert_eq!(
            bring_up.next_action(InstantMillis(100)),
            BringUpAction::ReadUntil(InstantMillis(2_100))
        );
        bring_up.apply_command(core::CMD_FW_VERSION, &[1, 51]);
        bring_up.apply_command(core::CMD_DETECT, &[DETECT_RESP]);
        assert!(matches!(
            bring_up.next_action(InstantMillis(200)),
            BringUpAction::WriteRadioConfiguration {
                outdated_firmware: Some(FirmwareVersion {
                    major: 1,
                    minor: 51
                }),
                ..
            }
        ));
        bring_up.apply_command(core::CMD_FREQUENCY, &868_000_000u32.to_be_bytes());
        bring_up.apply_command(core::CMD_BANDWIDTH, &125_000u32.to_be_bytes());
        bring_up.apply_command(core::CMD_TXPOWER, &[7]);
        bring_up.apply_command(core::CMD_SF, &[8]);
        bring_up.apply_command(core::CMD_RADIO_STATE, &[RADIO_STATE_ON]);
        assert_eq!(
            bring_up.next_action(InstantMillis(300)),
            BringUpAction::Complete
        );
    }

    #[test]
    fn deadlines_select_phase_specific_failures() {
        let mut bring_up = BringUp::new(radio(), DEFAULT_DETECT_TIMEOUT);
        let _ = bring_up.next_action(InstantMillis(0));
        bring_up.deadline_elapsed(InstantMillis(1_999));
        assert_eq!(
            bring_up.next_action(InstantMillis(1_999)),
            BringUpAction::ReadUntil(InstantMillis(2_000))
        );
        bring_up.deadline_elapsed(InstantMillis(2_000));
        assert_eq!(
            bring_up.next_action(InstantMillis(2_000)),
            BringUpAction::Failed(BringUpError::DetectTimedOut)
        );
    }
}
