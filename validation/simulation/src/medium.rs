use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard};

use tokio::sync::mpsc;

use crate::config::VirtualMediumConfig;
use crate::fault::{FaultPlan, TransmissionOrdinal};
use crate::interface::VirtualInterface;
use crate::trace::{MediumEvent, ReceptionDropReason, TraceBuffer, TraceSnapshot};

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
}

struct Endpoint {
    channel_tag: Vec<u8>,
    inbound: mpsc::Sender<Vec<u8>>,
}

struct MediumState {
    max_endpoints: usize,
    endpoint_receive_queue: usize,
    next_endpoint: Option<EndpointId>,
    next_transmission: Option<TransmissionOrdinal>,
    endpoints: BTreeMap<EndpointId, Endpoint>,
    channel_tags: BTreeSet<Vec<u8>>,
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
                max_endpoints: config.max_endpoints,
                endpoint_receive_queue: config.endpoint_receive_queue,
                next_endpoint: Some(EndpointId(0)),
                next_transmission: Some(TransmissionOrdinal(0)),
                endpoints: BTreeMap::new(),
                channel_tags: BTreeSet::new(),
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
        state.next_transmission = ordinal.0.checked_add(1).map(TransmissionOrdinal);
        let recipients: Vec<_> = state
            .endpoints
            .iter()
            .filter(|(endpoint, _)| **endpoint != from)
            .map(|(endpoint, attached)| (*endpoint, attached.inbound.clone()))
            .collect();
        state.trace.push(MediumEvent::TransmissionAccepted {
            ordinal,
            from,
            frame: frame.clone(),
        });
        if state.fault_plan.drops(ordinal) {
            for (to, _) in recipients {
                state.trace.push(MediumEvent::ReceptionDropped {
                    ordinal,
                    to,
                    reason: ReceptionDropReason::ScheduledFault,
                });
            }
            return Ok(());
        }
        for (to, inbound) in recipients {
            let event = match inbound.try_send(frame.clone()) {
                Ok(()) => MediumEvent::ReceptionQueued { ordinal, to },
                Err(mpsc::error::TrySendError::Full(_)) => MediumEvent::ReceptionDropped {
                    ordinal,
                    to,
                    reason: ReceptionDropReason::ReceiveQueueFull,
                },
                Err(mpsc::error::TrySendError::Closed(_)) => MediumEvent::ReceptionDropped {
                    ordinal,
                    to,
                    reason: ReceptionDropReason::EndpointClosed,
                },
            };
            state.trace.push(event);
        }
        Ok(())
    }

    pub(crate) fn detach(&self, endpoint: EndpointId) {
        let mut state = self.lock_state();
        let Some(detached) = state.endpoints.remove(&endpoint) else {
            return;
        };
        let _ = state.channel_tags.remove(&detached.channel_tag);
        state.trace.push(MediumEvent::EndpointDetached { endpoint });
    }

    fn lock_state(&self) -> MutexGuard<'_, MediumState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}
