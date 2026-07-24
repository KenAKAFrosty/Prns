use ::core::time::Duration;

use crate::interfaces::kiss_framing;
use crate::interfaces::PacketPhyStats;
use crate::units::{DurationMillis, InstantMillis};

use super::protocol::{self, PacketPhyState, RadioConfig};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeepaliveInterval(Duration);

impl KeepaliveInterval {
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Keepalive {
    #[default]
    Disabled,
    Detect(KeepaliveInterval),
}

pub const TCP_KEEPALIVE: Keepalive =
    Keepalive::Detect(KeepaliveInterval(Duration::from_millis(3_500)));

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeepaliveSchedule {
    interval: Option<KeepaliveInterval>,
    deadline: Option<InstantMillis>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeepaliveTransmission([u8; 4]);

impl KeepaliveTransmission {
    #[must_use]
    pub const fn wire_bytes(&self) -> &[u8; 4] {
        &self.0
    }
}

impl KeepaliveSchedule {
    #[must_use]
    pub fn new(keepalive: Keepalive, now: InstantMillis) -> Self {
        let interval = match keepalive {
            Keepalive::Disabled => None,
            Keepalive::Detect(interval) => Some(interval),
        };
        Self {
            interval,
            deadline: interval.map(|interval| deadline_after(now, interval.duration())),
        }
    }

    #[must_use]
    pub const fn deadline(self) -> Option<InstantMillis> {
        self.deadline
    }

    #[must_use]
    pub fn due(self, now: InstantMillis) -> Option<KeepaliveTransmission> {
        self.deadline
            .is_some_and(|deadline| now >= deadline)
            .then(|| KeepaliveTransmission(protocol::detect_request_frame()))
    }

    pub fn wrote(&mut self, now: InstantMillis) {
        self.deadline = self
            .interval
            .map(|interval| deadline_after(now, interval.duration()));
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum LiveCommand<'a> {
    Data {
        payload: &'a [u8],
        phy: PacketPhyStats,
    },
    Ready,
    Consumed,
}

#[derive(Debug, Default)]
pub struct LiveProtocol {
    packet_phy: PacketPhyState,
}

impl LiveProtocol {
    pub fn apply<'a>(
        &mut self,
        command: u8,
        payload: &'a [u8],
        radio: &RadioConfig,
    ) -> LiveCommand<'a> {
        match command {
            protocol::CMD_DATA if payload.is_empty() => LiveCommand::Consumed,
            protocol::CMD_DATA => LiveCommand::Data {
                payload,
                phy: self.packet_phy.take_for_data(),
            },
            kiss_framing::CMD_READY => LiveCommand::Ready,
            _ => {
                self.packet_phy.apply(command, payload, radio);
                LiveCommand::Consumed
            }
        }
    }
}

fn deadline_after(now: InstantMillis, duration: Duration) -> InstantMillis {
    now.saturating_add(DurationMillis::from_duration_saturating(duration))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interfaces::rnode::protocol::{RadioConfigInput, CMD_STAT_RSSI};
    use crate::interfaces::RssiDbm;

    fn radio() -> RadioConfig {
        RadioConfig::new(RadioConfigInput {
            frequency_hz: 868_000_000,
            bandwidth_hz: 125_000,
            tx_power_dbm: 7,
            spreading_factor: 8,
            coding_rate: 5,
            airtime_limit_short_centi_percent: None,
            airtime_limit_long_centi_percent: None,
        })
        .unwrap()
    }

    #[test]
    fn telemetry_is_attached_to_exactly_the_next_data_frame() {
        let mut protocol = LiveProtocol::default();
        assert_eq!(
            protocol.apply(CMD_STAT_RSSI, &[100], &radio()),
            LiveCommand::Consumed
        );
        assert_eq!(
            protocol.apply(protocol::CMD_DATA, b"first", &radio()),
            LiveCommand::Data {
                payload: b"first",
                phy: PacketPhyStats {
                    rssi: Some(RssiDbm::new(-57)),
                    ..PacketPhyStats::default()
                }
            }
        );
        assert_eq!(
            protocol.apply(protocol::CMD_DATA, b"second", &radio()),
            LiveCommand::Data {
                payload: b"second",
                phy: PacketPhyStats::default()
            }
        );
    }

    #[test]
    fn keepalive_deadlines_rearm_only_after_a_write() {
        let mut schedule = KeepaliveSchedule::new(TCP_KEEPALIVE, InstantMillis(100));
        assert_eq!(schedule.deadline(), Some(InstantMillis(3_600)));
        assert_eq!(schedule.due(InstantMillis(3_599)), None);
        assert_eq!(
            schedule
                .due(InstantMillis(3_600))
                .map(|transmission| *transmission.wire_bytes()),
            Some([0xc0, protocol::CMD_DETECT, protocol::DETECT_REQ, 0xc0])
        );
        schedule.wrote(InstantMillis(4_000));
        assert_eq!(schedule.deadline(), Some(InstantMillis(7_500)));
    }
}
