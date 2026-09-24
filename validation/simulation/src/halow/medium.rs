use super::backend::{Datagram, RadioControl};
use super::*;
use crate::{AdvanceError, AdvanceReport, MediumSchedule, SimulationDurationInTicks};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, MutexGuard},
};
use tokio::sync::{mpsc, watch};

struct Endpoint {
    mac: PeerMac,
    inbound: mpsc::Sender<Datagram>,
    control: watch::Sender<RadioControl>,
}
struct Delivery {
    ordinal: u64,
    to: RadioId,
    source: PeerMac,
    copy: u8,
    bytes: Vec<u8>,
}
struct State {
    limits: HaLowMediumLimits,
    now: SimulationTick,
    next_radio: u64,
    next_transmission: u64,
    next_delivery: u64,
    radios: BTreeMap<RadioId, Endpoint>,
    paths: BTreeMap<(RadioId, RadioId), PathState>,
    pending: BTreeMap<(SimulationTick, u64), Delivery>,
    faults: Vec<NextDatagramFault>,
    events: Vec<HaLowEvent>,
    peak_queued: usize,
    peak_pending: usize,
    omitted_events: u64,
}
impl State {
    fn record(&mut self, event: HaLowEvent) {
        if self.events.len() == self.limits.trace_events.get() {
            self.omitted_events = self.omitted_events.saturating_add(1);
            return;
        }
        self.events.push(event);
    }
    fn queued(&self) -> usize {
        self.radios
            .values()
            .map(|radio| radio.inbound.max_capacity() - radio.inbound.capacity())
            .sum()
    }
    fn deliver(&mut self, delivery: Delivery) -> DeliveryOutcome {
        let outcome = match self.radios.get(&delivery.to) {
            None => DeliveryOutcome::Detached,
            Some(radio) => match radio.inbound.try_send(Datagram {
                source: delivery.source,
                bytes: delivery.bytes,
            }) {
                Ok(()) => DeliveryOutcome::Queued,
                Err(mpsc::error::TrySendError::Full(_)) => DeliveryOutcome::ReceiveQueueFull,
                Err(mpsc::error::TrySendError::Closed(_)) => DeliveryOutcome::Detached,
            },
        };
        self.peak_queued = self.peak_queued.max(self.queued());
        self.record(HaLowEvent::Delivery {
            ordinal: delivery.ordinal,
            to: delivery.to,
            copy: delivery.copy,
            at: self.now,
            outcome,
        });
        outcome
    }
}

#[derive(Clone)]
pub struct VirtualHaLowMedium {
    state: Arc<Mutex<State>>,
}
impl VirtualHaLowMedium {
    pub fn new(limits: HaLowMediumLimits) -> Self {
        Self {
            state: Arc::new(Mutex::new(State {
                limits,
                now: SimulationTick::ZERO,
                next_radio: 0,
                next_transmission: 0,
                next_delivery: 0,
                radios: BTreeMap::new(),
                paths: BTreeMap::new(),
                pending: BTreeMap::new(),
                faults: Vec::new(),
                events: Vec::new(),
                peak_queued: 0,
                peak_pending: 0,
                omitted_events: 0,
            })),
        }
    }
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().expect("HaLoW medium lock")
    }
    pub fn attach(&self, mac: PeerMac) -> Result<VirtualHaLowRadio, AttachError> {
        let mut state = self.lock();
        if state.radios.len() == state.limits.radios.get() {
            return Err(AttachError::Capacity);
        }
        if state.radios.values().any(|radio| radio.mac == mac) {
            return Err(AttachError::AddressInUse);
        }
        let id = RadioId(state.next_radio);
        state.next_radio = state
            .next_radio
            .checked_add(1)
            .ok_or(AttachError::IdsExhausted)?;
        let (inbound, receiver) = mpsc::channel(state.limits.receive_datagrams.get());
        let (control, changes) = watch::channel(RadioControl {
            send: SendBehavior::Ready,
            receive: ReceiveBehavior::Ready,
        });
        state.radios.insert(
            id,
            Endpoint {
                mac,
                inbound,
                control,
            },
        );
        state.record(HaLowEvent::Attached { radio: id, mac });
        Ok(VirtualHaLowRadio::new(self.clone(), id, receiver, changes))
    }
    pub(crate) fn detach(&self, id: RadioId) {
        let mut state = self.lock();
        if state.radios.remove(&id).is_none() {
            return;
        }
        state.paths.retain(|(from, to), _| *from != id && *to != id);
        state.faults.retain(|fault| fault.from != id);
        state.record(HaLowEvent::Detached { radio: id });
    }
    pub fn set_path(&self, from: RadioId, to: RadioId, path: PathState) {
        let mut state = self.lock();
        assert!(
            from != to && state.radios.contains_key(&from) && state.radios.contains_key(&to),
            "live distinct radio path"
        );
        let changed = match path {
            PathState::Reachable | PathState::BroadcastOnly => {
                state.paths.insert((from, to), path) != Some(path)
            }
            PathState::Isolated => state.paths.remove(&(from, to)).is_some(),
        };
        if changed {
            let at = state.now;
            state.record(HaLowEvent::Path {
                from,
                to,
                state: path,
                at,
            });
        }
    }
    pub fn set_send_behavior(&self, radio: RadioId, behavior: SendBehavior) {
        let mut state = self.lock();
        state
            .radios
            .get(&radio)
            .expect("live radio")
            .control
            .send_modify(|control| control.send = behavior);
        let at = state.now;
        state.record(HaLowEvent::SendBehavior {
            radio,
            behavior,
            at,
        });
    }
    pub fn set_receive_behavior(&self, radio: RadioId, behavior: ReceiveBehavior) {
        let mut state = self.lock();
        state
            .radios
            .get(&radio)
            .expect("live radio")
            .control
            .send_modify(|control| control.receive = behavior);
        let at = state.now;
        state.record(HaLowEvent::ReceiveBehavior {
            radio,
            behavior,
            at,
        });
    }
    pub fn arm_next(&self, fault: NextDatagramFault) -> Result<(), ArmFaultError> {
        let mut state = self.lock();
        if state.faults.len() == state.limits.armed_faults.get() {
            return Err(ArmFaultError::Capacity);
        }
        if !state.radios.contains_key(&fault.from) {
            return Err(ArmFaultError::Detached);
        }
        if matches!(fault.action, TransmissionAction::Duplicate { first_after, second_after } if first_after > second_after)
        {
            return Err(ArmFaultError::NoncanonicalDuplicate);
        }
        state.faults.push(fault);
        let at = state.now;
        state.record(HaLowEvent::FaultArmed { fault, at });
        Ok(())
    }
    pub(crate) fn transmit(
        &self,
        from: RadioId,
        destination: Destination,
        bytes: &[u8],
    ) -> std::io::Result<()> {
        if bytes.len() > personal_rns::interfaces::wifi_halow::DATAGRAM_MTU {
            return Err(std::io::ErrorKind::InvalidInput.into());
        }
        let mut state = self.lock();
        let source = state
            .radios
            .get(&from)
            .ok_or(std::io::ErrorKind::BrokenPipe)?
            .mac;
        let recipients: Vec<_> = state
            .radios
            .iter()
            .filter(
                |(id, endpoint)| match (state.paths.get(&(from, **id)), destination) {
                    (
                        Some(PathState::Reachable | PathState::BroadcastOnly),
                        Destination::Broadcast,
                    ) => true,
                    (Some(PathState::Reachable), Destination::Peer(peer)) => endpoint.mac == peer,
                    (
                        Some(PathState::BroadcastOnly | PathState::Isolated) | None,
                        Destination::Peer(_),
                    )
                    | (Some(PathState::Isolated) | None, Destination::Broadcast) => false,
                },
            )
            .map(|(id, _)| *id)
            .collect();
        let ordinal = state.next_transmission;
        state.next_transmission = ordinal.checked_add(1).expect("transmission ordinal budget");
        let at = state.now;
        state.record(HaLowEvent::Transmitted {
            ordinal,
            from,
            destination,
            at,
            bytes: bytes.to_vec(),
        });
        let action = state
            .faults
            .iter()
            .position(|fault| fault.from == from && fault.destination.matches(destination))
            .map(|index| state.faults.remove(index).action);
        let copies = match action {
            Some(TransmissionAction::Drop) => {
                for to in recipients {
                    state.record(HaLowEvent::Delivery {
                        ordinal,
                        to,
                        copy: 0,
                        at,
                        outcome: DeliveryOutcome::FaultLoss,
                    });
                }
                return Ok(());
            }
            Some(TransmissionAction::Delay { by }) => vec![(0, by)],
            Some(TransmissionAction::Duplicate {
                first_after,
                second_after,
            }) => vec![(0, first_after), (1, second_after)],
            None => vec![(0, SimulationDurationInTicks::ZERO)],
        };
        for to in recipients {
            for (copy, delay) in &copies {
                let due = at.checked_add(*delay).expect("delivery timeline budget");
                let delivery = Delivery {
                    ordinal,
                    to,
                    source,
                    copy: *copy,
                    bytes: bytes.to_vec(),
                };
                if due == at {
                    state.deliver(delivery);
                    continue;
                }
                if state.pending.len() == state.limits.pending_deliveries.get() {
                    state.record(HaLowEvent::Delivery {
                        ordinal,
                        to,
                        copy: *copy,
                        at,
                        outcome: DeliveryOutcome::PendingCapacity,
                    });
                    continue;
                }
                let sequence = state.next_delivery;
                state.next_delivery = sequence.checked_add(1).expect("delivery ordinal budget");
                state.pending.insert((due, sequence), delivery);
                state.peak_pending = state.peak_pending.max(state.pending.len());
                state.record(HaLowEvent::Scheduled {
                    ordinal,
                    to,
                    copy: *copy,
                    at: due,
                });
            }
        }
        Ok(())
    }
    pub fn inject(&self, to: RadioId, from: PeerMac, bytes: &[u8]) -> DeliveryOutcome {
        assert!(
            bytes.len() <= personal_rns::interfaces::wifi_halow::DATAGRAM_MTU,
            "bounded injected datagram"
        );
        let mut state = self.lock();
        let outcome = match state.radios.get(&to) {
            None => DeliveryOutcome::Detached,
            Some(radio) => match radio.inbound.try_send(Datagram {
                source: from,
                bytes: bytes.to_vec(),
            }) {
                Ok(()) => DeliveryOutcome::Queued,
                Err(mpsc::error::TrySendError::Full(_)) => DeliveryOutcome::ReceiveQueueFull,
                Err(mpsc::error::TrySendError::Closed(_)) => DeliveryOutcome::Detached,
            },
        };
        state.peak_queued = state.peak_queued.max(state.queued());
        let at = state.now;
        state.record(HaLowEvent::Injected {
            from,
            to,
            at,
            bytes: bytes.to_vec(),
            outcome,
        });
        outcome
    }
    pub fn now(&self) -> SimulationTick {
        self.lock().now
    }
    pub fn schedule(&self) -> MediumSchedule {
        let state = self.lock();
        MediumSchedule {
            now: state.now,
            next_event_at: state.pending.first_key_value().map(|((at, _), _)| *at),
        }
    }
    pub fn advance_to_next_event(
        &self,
        not_after: SimulationTick,
    ) -> Result<AdvanceReport, AdvanceError> {
        let mut state = self.lock();
        if not_after < state.now {
            return Err(AdvanceError::BeforeCurrent {
                current: state.now,
                requested: not_after,
            });
        }
        let to = state
            .pending
            .first_key_value()
            .map_or(not_after, |((at, _), _)| (*at).min(not_after));
        let mut report = AdvanceReport {
            from: state.now,
            to,
            receptions_queued: 0,
            receptions_dropped: 0,
        };
        state.now = to;
        while state
            .pending
            .first_key_value()
            .is_some_and(|((at, _), _)| *at <= to)
        {
            let (_, delivery) = state.pending.pop_first().expect("due delivery");
            match state.deliver(delivery) {
                DeliveryOutcome::Queued => report.receptions_queued += 1,
                _ => report.receptions_dropped += 1,
            }
        }
        Ok(report)
    }
    pub fn snapshot(&self) -> HaLowSnapshot {
        let state = self.lock();
        HaLowSnapshot {
            radios: state.radios.len(),
            queued: state.queued(),
            pending: state.pending.len(),
            armed_faults: state.faults.len(),
            peak_queued: state.peak_queued,
            peak_pending: state.peak_pending,
            events: state.events.clone(),
            retention: if state.omitted_events == 0 {
                TraceRetention::Complete
            } else {
                TraceRetention::Exhausted {
                    omitted_events: state.omitted_events,
                }
            },
        }
    }
}
