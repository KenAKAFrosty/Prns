use alloc::collections::VecDeque;
use alloc::vec::Vec;
use core::time::Duration;

use crate::units::InstantMillis;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadyCommandFlowControl {
    Disabled,
    WaitForReady,
    WaitForReadyOrTimeout(ReadyTimeout),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadyTimeout(Duration);

impl ReadyTimeout {
    #[must_use]
    pub const fn new(duration: Duration) -> Self {
        Self(duration)
    }

    #[must_use]
    pub const fn duration(self) -> Duration {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StationIdInterval(Duration);

impl StationIdInterval {
    #[must_use]
    pub const fn new(duration: Duration) -> Self {
        Self(duration)
    }

    #[must_use]
    pub const fn duration(self) -> Duration {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StationIdWireFormat {
    Exact,
    KissPadded,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StationIdentification {
    payload: Vec<u8>,
    interval: StationIdInterval,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EmptyStationIdentification;

impl StationIdentification {
    pub fn new(
        payload: &[u8],
        interval: StationIdInterval,
        wire_format: StationIdWireFormat,
    ) -> Result<Self, EmptyStationIdentification> {
        if payload.is_empty() {
            return Err(EmptyStationIdentification);
        }
        let mut payload = payload.to_vec();
        if matches!(wire_format, StationIdWireFormat::KissPadded) {
            payload.resize(payload.len().max(15), 0);
        }
        Ok(Self { payload, interval })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TransmissionKind {
    Packet,
    StationIdentification,
}

pub struct Transmission {
    payload: Vec<u8>,
    kind: TransmissionKind,
}

impl Transmission {
    #[must_use]
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    #[must_use]
    pub const fn is_packet(&self) -> bool {
        matches!(self.kind, TransmissionKind::Packet)
    }
}

pub struct KissTransmissionControl {
    flow_control: ReadyCommandFlowControl,
    locked_at: Option<InstantMillis>,
    queue: VecDeque<Transmission>,
    station_identification: Option<StationIdentification>,
    first_packet_transmitted_at: Option<InstantMillis>,
    station_identification_queued: bool,
}

impl KissTransmissionControl {
    #[must_use]
    pub fn new(
        flow_control: ReadyCommandFlowControl,
        station_identification: Option<StationIdentification>,
    ) -> Self {
        Self {
            flow_control,
            locked_at: None,
            queue: VecDeque::new(),
            station_identification,
            first_packet_transmitted_at: None,
            station_identification_queued: false,
        }
    }

    pub fn connection_opened(&mut self) {
        self.locked_at = None;
    }

    pub fn accept_packet(&mut self, payload: &[u8], now: InstantMillis) -> Option<Transmission> {
        self.accept(
            Transmission {
                payload: payload.to_vec(),
                kind: TransmissionKind::Packet,
            },
            now,
        )
    }

    pub fn ready_received(&mut self, now: InstantMillis) -> Option<Transmission> {
        self.locked_at = None;
        self.next_queued(now)
    }

    pub fn flow_timeout_elapsed(&mut self, now: InstantMillis) -> Option<Transmission> {
        if self
            .flow_timeout_deadline()
            .is_none_or(|deadline| deadline > now)
        {
            return None;
        }
        self.ready_received(now)
    }

    pub fn next_queued(&mut self, now: InstantMillis) -> Option<Transmission> {
        if self.locked_at.is_some() {
            return None;
        }
        let transmission = self.queue.pop_front()?;
        self.lock(now);
        Some(transmission)
    }

    #[must_use]
    pub fn flow_timeout_deadline(&self) -> Option<InstantMillis> {
        let ReadyCommandFlowControl::WaitForReadyOrTimeout(timeout) = self.flow_control else {
            return None;
        };
        self.locked_at
            .map(|locked_at| deadline_after(locked_at, timeout.duration()))
    }

    #[must_use]
    pub fn station_identification_deadline(&self) -> Option<InstantMillis> {
        if self.station_identification_queued {
            return None;
        }
        let station = self.station_identification.as_ref()?;
        self.first_packet_transmitted_at
            .map(|first| deadline_after(first, station.interval.duration()))
    }

    pub fn arm_station_identification(&mut self, now: InstantMillis) {
        if self.station_identification.is_some() {
            self.first_packet_transmitted_at.get_or_insert(now);
        }
    }

    pub fn station_identification_elapsed(&mut self, now: InstantMillis) -> Option<Transmission> {
        if self
            .station_identification_deadline()
            .is_none_or(|deadline| deadline > now)
        {
            return None;
        }
        let station = self.station_identification.as_ref()?;
        self.station_identification_queued = true;
        self.accept(
            Transmission {
                payload: station.payload.clone(),
                kind: TransmissionKind::StationIdentification,
            },
            now,
        )
    }

    pub fn transmitted(&mut self, transmission: &Transmission, now: InstantMillis) {
        match transmission.kind {
            TransmissionKind::Packet => {
                self.first_packet_transmitted_at.get_or_insert(now);
            }
            TransmissionKind::StationIdentification => {
                self.first_packet_transmitted_at = None;
                self.station_identification_queued = false;
            }
        }
    }

    fn accept(&mut self, transmission: Transmission, now: InstantMillis) -> Option<Transmission> {
        if self.locked_at.is_none() {
            self.lock(now);
            Some(transmission)
        } else {
            self.queue.push_back(transmission);
            None
        }
    }

    fn lock(&mut self, now: InstantMillis) {
        if !matches!(self.flow_control, ReadyCommandFlowControl::Disabled) {
            self.locked_at = Some(now);
        }
    }
}

fn deadline_after(now: InstantMillis, duration: Duration) -> InstantMillis {
    let milliseconds = u64::try_from(duration.as_millis()).unwrap_or(u64::MAX);
    InstantMillis(now.0.saturating_add(milliseconds))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ready_and_timeout_release_one_queued_transmission_at_a_time() {
        let mut control = KissTransmissionControl::new(
            ReadyCommandFlowControl::WaitForReadyOrTimeout(ReadyTimeout::new(Duration::from_secs(
                5,
            ))),
            None,
        );
        let first = control.accept_packet(b"one", InstantMillis(0)).unwrap();
        assert_eq!(first.payload(), b"one");
        assert!(control.accept_packet(b"two", InstantMillis(0)).is_none());
        assert!(control.accept_packet(b"three", InstantMillis(0)).is_none());
        assert_eq!(control.flow_timeout_deadline(), Some(InstantMillis(5_000)));
        assert!(control.flow_timeout_elapsed(InstantMillis(4_999)).is_none());
        let second = control.ready_received(InstantMillis(1_000)).unwrap();
        assert_eq!(second.payload(), b"two");
        assert!(control.next_queued(InstantMillis(1_000)).is_none());
        assert!(control.flow_timeout_elapsed(InstantMillis(5_999)).is_none());
        let third = control.flow_timeout_elapsed(InstantMillis(6_000)).unwrap();
        assert_eq!(third.payload(), b"three");
    }

    #[test]
    fn station_identification_is_padded_once_and_rearmed_by_normal_traffic() {
        let station = StationIdentification::new(
            b"N0CALL",
            StationIdInterval::new(Duration::from_secs(60)),
            StationIdWireFormat::KissPadded,
        )
        .unwrap();
        let mut control =
            KissTransmissionControl::new(ReadyCommandFlowControl::Disabled, Some(station));
        let packet = control.accept_packet(b"packet", InstantMillis(0)).unwrap();
        control.transmitted(&packet, InstantMillis(0));
        assert_eq!(
            control.station_identification_deadline(),
            Some(InstantMillis(60_000))
        );
        assert!(control
            .station_identification_elapsed(InstantMillis(59_999))
            .is_none());
        let station = control
            .station_identification_elapsed(InstantMillis(60_000))
            .unwrap();
        assert_eq!(station.payload().len(), 15);
        assert_eq!(&station.payload()[..6], b"N0CALL");
        control.transmitted(&station, InstantMillis(60_000));
        assert_eq!(control.station_identification_deadline(), None);
    }
}
