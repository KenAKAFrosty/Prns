use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard};

use tokio::sync::mpsc;

use crate::config::VirtualMediumConfig;
use crate::fault::{FaultPlan, TransmissionAction, TransmissionOrdinal};
use crate::interface::VirtualInterface;
use crate::time::{AdvanceError, AdvanceReport, SimulationDurationInTicks, SimulationTick};
use crate::topology::Topology;
use crate::trace::{DeliveryCopy, MediumEvent, ReceptionDropReason, TraceBuffer, TraceSnapshot};
use crate::{Reachability, TopologyError, TopologyMutation};

const MAX_CHANNEL_TAG_BYTES: usize = 128;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EndpointId(pub(crate) u16);

impl EndpointId {
    #[must_use]
    pub const fn get(self) -> u16 {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachError {
    EmptyChannelTag,
    ChannelTagTooLong { length: usize, maximum: usize },
    DuplicateChannelTag,
    EndpointCapacityReached,
    EndpointIdsExhausted,
}

impl fmt::Display for AttachError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyChannelTag => formatter.write_str("virtual channel tag must not be empty"),
            Self::ChannelTagTooLong { length, maximum } => write!(
                formatter,
                "virtual channel tag is {length} bytes; maximum is {maximum}",
            ),
            Self::DuplicateChannelTag => {
                formatter.write_str("virtual channel tag is already attached")
            }
            Self::EndpointCapacityReached => {
                formatter.write_str("virtual medium endpoint capacity reached")
            }
            Self::EndpointIdsExhausted => {
                formatter.write_str("virtual medium endpoint identifiers exhausted")
            }
        }
    }
}

impl std::error::Error for AttachError {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TransmitError {
    DetachedEndpoint,
    TransmissionOrdinalsExhausted,
    DeliveryOrdinalsExhausted,
    TimelineExhausted,
}

struct Endpoint {
    channel_tag: Vec<u8>,
    inbound: mpsc::Sender<Vec<u8>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct PendingKey {
    at: SimulationTick,
    sequence: u64,
}

struct PendingDelivery {
    transmission: TransmissionOrdinal,
    to: EndpointId,
    copy: DeliveryCopy,
    frame: Vec<u8>,
}

struct MediumState {
    topology: Topology<EndpointId>,
    max_endpoints: usize,
    endpoint_receive_queue: usize,
    pending_capacity: usize,
    now: SimulationTick,
    next_endpoint: Option<EndpointId>,
    next_transmission: Option<TransmissionOrdinal>,
    next_delivery: Option<u64>,
    endpoints: BTreeMap<EndpointId, Endpoint>,
    channel_tags: BTreeSet<Vec<u8>>,
    pending: BTreeMap<PendingKey, PendingDelivery>,
    fault_plan: FaultPlan,
    trace: TraceBuffer,
}

#[derive(Clone)]
pub struct VirtualMedium {
    state: Arc<Mutex<MediumState>>,
}

impl VirtualMedium {
    #[must_use]
    pub fn new(config: VirtualMediumConfig) -> Self {
        Self {
            state: Arc::new(Mutex::new(MediumState {
                topology: Topology::new(config.topology),
                max_endpoints: config.max_endpoints,
                endpoint_receive_queue: config.endpoint_receive_queue,
                pending_capacity: config.pending_deliveries,
                now: SimulationTick::ZERO,
                next_endpoint: Some(EndpointId(0)),
                next_transmission: Some(TransmissionOrdinal(0)),
                next_delivery: Some(0),
                endpoints: BTreeMap::new(),
                channel_tags: BTreeSet::new(),
                pending: BTreeMap::new(),
                fault_plan: config.fault_plan,
                trace: TraceBuffer::new(config.trace_capacity),
            })),
        }
    }

    pub fn attach(&self, channel_tag: &[u8]) -> Result<VirtualInterface, AttachError> {
        if channel_tag.is_empty() {
            return Err(AttachError::EmptyChannelTag);
        }
        if channel_tag.len() > MAX_CHANNEL_TAG_BYTES {
            return Err(AttachError::ChannelTagTooLong {
                length: channel_tag.len(),
                maximum: MAX_CHANNEL_TAG_BYTES,
            });
        }

        let mut state = self.lock_state();
        if state.endpoints.len() == state.max_endpoints {
            return Err(AttachError::EndpointCapacityReached);
        }
        if state.channel_tags.contains(channel_tag) {
            return Err(AttachError::DuplicateChannelTag);
        }
        let endpoint = state
            .next_endpoint
            .ok_or(AttachError::EndpointIdsExhausted)?;
        state.next_endpoint = endpoint.0.checked_add(1).map(EndpointId);
        let channel_tag = channel_tag.to_vec();
        let (inbound, receiver) = mpsc::channel(state.endpoint_receive_queue);
        let _ = state.channel_tags.insert(channel_tag.clone());
        let replaced = state.endpoints.insert(
            endpoint,
            Endpoint {
                channel_tag: channel_tag.clone(),
                inbound,
            },
        );
        debug_assert!(replaced.is_none(), "fresh endpoint id must be vacant");
        state.topology.attach(endpoint);
        state.trace.push(MediumEvent::EndpointAttached {
            endpoint,
            channel_tag: channel_tag.clone(),
        });
        drop(state);

        Ok(VirtualInterface::new(
            self.clone(),
            endpoint,
            channel_tag,
            receiver,
        ))
    }

    #[must_use]
    pub fn now(&self) -> SimulationTick {
        self.lock_state().now
    }

    /// Reachability is sampled on transmission; already scheduled frames remain in flight.
    pub fn set_reachability(
        &self,
        first: EndpointId,
        second: EndpointId,
        reachability: Reachability,
    ) -> Result<TopologyMutation, TopologyError<EndpointId>> {
        let mut state = self.lock_state();
        let mutation = state
            .topology
            .set_reachability(first, second, reachability)?;
        if mutation == TopologyMutation::Applied {
            let at = state.now;
            state.trace.push(MediumEvent::ReachabilityChanged {
                first,
                second,
                reachability,
                at,
            });
        }
        Ok(mutation)
    }

    #[must_use]
    pub fn pending_delivery_count(&self) -> usize {
        self.lock_state().pending.len()
    }

    pub fn advance_to(&self, requested: SimulationTick) -> Result<AdvanceReport, AdvanceError> {
        advance_locked(&mut self.lock_state(), requested)
    }

    pub fn advance_by(&self, by: SimulationDurationInTicks) -> Result<AdvanceReport, AdvanceError> {
        let mut state = self.lock_state();
        let requested = state.now.checked_add(by).ok_or(AdvanceError::Overflow {
            current: state.now,
            by,
        })?;
        advance_locked(&mut state, requested)
    }

    #[must_use]
    pub fn trace(&self) -> TraceSnapshot {
        self.lock_state().trace.snapshot()
    }

    pub(crate) fn transmit(&self, from: EndpointId, frame: Vec<u8>) -> Result<(), TransmitError> {
        let mut state = self.lock_state();
        if !state.endpoints.contains_key(&from) {
            return Err(TransmitError::DetachedEndpoint);
        }
        let ordinal = state
            .next_transmission
            .ok_or(TransmitError::TransmissionOrdinalsExhausted)?;
        let deliveries = planned_deliveries(&state, ordinal)?;
        let recipients: Vec<_> = state.topology.neighbors(from).collect();
        let delayed_per_recipient = deliveries.iter().filter(|(_, at)| *at > state.now).count();
        let delayed_count = recipients.len().saturating_mul(delayed_per_recipient);
        let has_pending_capacity = state
            .pending
            .len()
            .checked_add(delayed_count)
            .is_some_and(|needed| needed <= state.pending_capacity);
        if has_pending_capacity && !delivery_ordinals_fit(state.next_delivery, delayed_count) {
            return Err(TransmitError::DeliveryOrdinalsExhausted);
        }

        state.next_transmission = ordinal.0.checked_add(1).map(TransmissionOrdinal);
        let now = state.now;
        state.trace.push(MediumEvent::TransmissionAccepted {
            ordinal,
            from,
            at: now,
            frame: frame.clone(),
        });
        if matches!(
            state.fault_plan.action_for(ordinal),
            Some(TransmissionAction::Drop)
        ) {
            for to in recipients {
                state.trace.push(MediumEvent::ReceptionDropped {
                    ordinal,
                    to,
                    copy: DeliveryCopy::Original,
                    at: now,
                    intended_for: now,
                    reason: ReceptionDropReason::ScheduledFault,
                });
            }
            return Ok(());
        }

        for to in recipients {
            for (copy, at) in &deliveries {
                if *at == state.now {
                    let event = reception_event(&state, ordinal, to, *copy, *at, frame.clone());
                    state.trace.push(event);
                } else if !has_pending_capacity {
                    state.trace.push(MediumEvent::ReceptionDropped {
                        ordinal,
                        to,
                        copy: *copy,
                        at: now,
                        intended_for: *at,
                        reason: ReceptionDropReason::PendingCapacityReached,
                    });
                } else {
                    let sequence = state
                        .next_delivery
                        .ok_or(TransmitError::DeliveryOrdinalsExhausted)?;
                    state.next_delivery = sequence.checked_add(1);
                    let replaced = state.pending.insert(
                        PendingKey { at: *at, sequence },
                        PendingDelivery {
                            transmission: ordinal,
                            to,
                            copy: *copy,
                            frame: frame.clone(),
                        },
                    );
                    debug_assert!(replaced.is_none(), "fresh delivery key must be vacant");
                    state.trace.push(MediumEvent::ReceptionScheduled {
                        ordinal,
                        to,
                        copy: *copy,
                        deliver_at: *at,
                    });
                }
            }
        }
        Ok(())
    }

    pub(crate) fn detach(&self, endpoint: EndpointId) {
        let mut state = self.lock_state();
        let Some(detached) = state.endpoints.remove(&endpoint) else {
            return;
        };
        let _ = state.channel_tags.remove(&detached.channel_tag);
        state.topology.detach(endpoint);
        state.trace.push(MediumEvent::EndpointDetached { endpoint });
    }

    fn lock_state(&self) -> MutexGuard<'_, MediumState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

fn planned_deliveries(
    state: &MediumState,
    ordinal: TransmissionOrdinal,
) -> Result<Vec<(DeliveryCopy, SimulationTick)>, TransmitError> {
    let at = |after| {
        state
            .now
            .checked_add(after)
            .ok_or(TransmitError::TimelineExhausted)
    };
    match state.fault_plan.action_for(ordinal) {
        None => Ok(vec![(DeliveryCopy::Original, state.now)]),
        Some(TransmissionAction::Drop) => Ok(Vec::new()),
        Some(TransmissionAction::Delay { by }) => Ok(vec![(DeliveryCopy::Original, at(by)?)]),
        Some(TransmissionAction::Duplicate {
            first_after,
            second_after,
        }) => Ok(vec![
            (DeliveryCopy::Original, at(first_after)?),
            (DeliveryCopy::Duplicate, at(second_after)?),
        ]),
    }
}

fn delivery_ordinals_fit(next: Option<u64>, count: usize) -> bool {
    if count == 0 {
        return true;
    }
    let Ok(last_offset) = u64::try_from(count - 1) else {
        return false;
    };
    next.and_then(|first| first.checked_add(last_offset))
        .is_some()
}

fn advance_locked(
    state: &mut MediumState,
    requested: SimulationTick,
) -> Result<AdvanceReport, AdvanceError> {
    if requested < state.now {
        return Err(AdvanceError::BeforeCurrent {
            current: state.now,
            requested,
        });
    }
    let from = state.now;
    if requested > from {
        state.now = requested;
        state.trace.push(MediumEvent::TimeAdvanced {
            from,
            to: requested,
        });
    }
    let mut report = AdvanceReport {
        from,
        to: requested,
        receptions_queued: 0,
        receptions_dropped: 0,
    };
    while let Some(key) = state.pending.first_key_value().map(|(key, _)| *key) {
        if key.at > requested {
            break;
        }
        let Some(delivery) = state.pending.remove(&key) else {
            continue;
        };
        let event = reception_event(
            state,
            delivery.transmission,
            delivery.to,
            delivery.copy,
            key.at,
            delivery.frame,
        );
        match &event {
            MediumEvent::ReceptionQueued { .. } => {
                report.receptions_queued = report.receptions_queued.saturating_add(1);
            }
            MediumEvent::ReceptionDropped { .. } => {
                report.receptions_dropped = report.receptions_dropped.saturating_add(1);
            }
            _ => debug_assert!(false, "reception settlement must be queued or dropped"),
        }
        state.trace.push(event);
    }
    Ok(report)
}

fn reception_event(
    state: &MediumState,
    ordinal: TransmissionOrdinal,
    to: EndpointId,
    copy: DeliveryCopy,
    at: SimulationTick,
    frame: Vec<u8>,
) -> MediumEvent {
    let outcome = state
        .endpoints
        .get(&to)
        .map(|endpoint| endpoint.inbound.try_send(frame));
    match outcome {
        Some(Ok(())) => MediumEvent::ReceptionQueued {
            ordinal,
            to,
            copy,
            at,
        },
        Some(Err(mpsc::error::TrySendError::Full(_))) => MediumEvent::ReceptionDropped {
            ordinal,
            to,
            copy,
            at,
            intended_for: at,
            reason: ReceptionDropReason::ReceiveQueueFull,
        },
        Some(Err(mpsc::error::TrySendError::Closed(_))) | None => MediumEvent::ReceptionDropped {
            ordinal,
            to,
            copy,
            at,
            intended_for: at,
            reason: ReceptionDropReason::EndpointClosed,
        },
    }
}
