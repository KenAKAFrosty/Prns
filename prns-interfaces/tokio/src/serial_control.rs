use std::collections::VecDeque;
use std::time::Duration;

use tokio::time::Instant;

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

pub(crate) struct Transmission {
    payload: Vec<u8>,
    kind: TransmissionKind,
}

impl Transmission {
    pub(crate) fn payload(&self) -> &[u8] {
        &self.payload
    }

    pub(crate) const fn is_packet(&self) -> bool {
        matches!(self.kind, TransmissionKind::Packet)
    }
}

pub(crate) struct SerialControl {
    flow_control: ReadyCommandFlowControl,
    locked_at: Option<Instant>,
    queue: VecDeque<Transmission>,
    station_identification: Option<StationIdentification>,
    first_packet_transmitted_at: Option<Instant>,
    station_identification_queued: bool,
}

impl SerialControl {
    pub(crate) fn new(
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

    pub(crate) fn connection_opened(&mut self) {
        self.locked_at = None;
    }

    pub(crate) fn accept_packet(&mut self, payload: &[u8], now: Instant) -> Option<Transmission> {
        self.accept(
            Transmission {
                payload: payload.to_vec(),
                kind: TransmissionKind::Packet,
            },
            now,
        )
    }

    pub(crate) fn ready(&mut self, now: Instant) -> Option<Transmission> {
        self.locked_at = None;
        self.take_queued(now)
    }

    pub(crate) fn take_queued(&mut self, now: Instant) -> Option<Transmission> {
        if self.locked_at.is_some() {
            return None;
        }
        let transmission = self.queue.pop_front()?;
        self.lock(now);
        Some(transmission)
    }

    pub(crate) fn flow_timeout_deadline(&self) -> Option<Instant> {
        let ReadyCommandFlowControl::WaitForReadyOrTimeout(timeout) = self.flow_control else {
            return None;
        };
        self.locked_at
            .map(|locked_at| locked_at + timeout.duration())
    }

    pub(crate) fn station_identification_deadline(&self) -> Option<Instant> {
        if self.station_identification_queued {
            return None;
        }
        let station = self.station_identification.as_ref()?;
        self.first_packet_transmitted_at
            .map(|first| first + station.interval.duration())
    }

    pub(crate) fn arm_station_identification(&mut self, now: Instant) {
        if self.station_identification.is_some() {
            self.first_packet_transmitted_at.get_or_insert(now);
        }
    }

    pub(crate) fn station_identification_due(&mut self, now: Instant) -> Option<Transmission> {
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

    pub(crate) fn transmitted(&mut self, transmission: &Transmission, now: Instant) {
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

    fn accept(&mut self, transmission: Transmission, now: Instant) -> Option<Transmission> {
        if self.locked_at.is_none() {
            self.lock(now);
            Some(transmission)
        } else {
            self.queue.push_back(transmission);
            None
        }
    }

    fn lock(&mut self, now: Instant) {
        if !matches!(self.flow_control, ReadyCommandFlowControl::Disabled) {
            self.locked_at = Some(now);
        }
    }
}

pub(crate) async fn wait_for_deadline(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ready_and_timeout_release_one_queued_transmission_at_a_time() {
        let now = Instant::now();
        let mut control = SerialControl::new(
            ReadyCommandFlowControl::WaitForReadyOrTimeout(ReadyTimeout::new(Duration::from_secs(
                5,
            ))),
            None,
        );
        let first = control.accept_packet(b"one", now).unwrap();
        assert_eq!(first.payload(), b"one");
        assert!(control.accept_packet(b"two", now).is_none());
        assert!(control.accept_packet(b"three", now).is_none());
        assert_eq!(
            control.flow_timeout_deadline(),
            Some(now + Duration::from_secs(5))
        );
        let second = control.ready(now + Duration::from_secs(1)).unwrap();
        assert_eq!(second.payload(), b"two");
        assert!(control.take_queued(now + Duration::from_secs(1)).is_none());
        let third = control.ready(now + Duration::from_secs(6)).unwrap();
        assert_eq!(third.payload(), b"three");
    }

    #[test]
    fn station_identification_is_padded_once_and_rearmed_by_normal_traffic() {
        let now = Instant::now();
        let station = StationIdentification::new(
            b"N0CALL",
            StationIdInterval::new(Duration::from_secs(60)),
            StationIdWireFormat::KissPadded,
        )
        .unwrap();
        let mut control = SerialControl::new(ReadyCommandFlowControl::Disabled, Some(station));
        let packet = control.accept_packet(b"packet", now).unwrap();
        control.transmitted(&packet, now);
        assert_eq!(
            control.station_identification_deadline(),
            Some(now + Duration::from_secs(60))
        );
        let station = control
            .station_identification_due(now + Duration::from_secs(60))
            .unwrap();
        assert_eq!(station.payload().len(), 15);
        assert_eq!(&station.payload()[..6], b"N0CALL");
        control.transmitted(&station, now + Duration::from_secs(60));
        assert_eq!(control.station_identification_deadline(), None);
    }
}
