use std::collections::HashMap;

use tokio::sync::mpsc::{error::TrySendError, Sender, UnboundedSender};
use tokio::sync::oneshot;

use crate::engine::{
    AnnounceRateState, CommandId, Journaled, SendRequestFailure, SendRequestRejection,
    SendResourceFailure, SendResourceRejection, Settlement, WakeSchedules,
};
use crate::routing::links::channel::byte_stream::{self, StreamId, STREAM_DATA_TYPE};
use crate::routing::links::resources::ResourceHash;
use crate::routing::links::LinkId;
use crate::runtime::node_introspection::{AnnounceRateHistory, AnnounceRateSnapshot};
#[cfg(feature = "runtime-metrics")]
use crate::runtime::ReliabilityMetricsSnapshot;
use crate::units::RttMillis;

use super::host_protocol::{
    ResourceInbound, StreamInbound, StreamReaderRegistrationError, StreamReceiveFailure,
};

struct RequestPending {
    completion: oneshot::Sender<Result<(std::vec::Vec<u8>, RttMillis), SendRequestFailure>>,
    data: Option<std::vec::Vec<u8>>,
}

pub(super) struct JournalDispatch<J>
where
    J: for<'a> FnMut(Journaled<'a>),
{
    delivery: JournalDelivery,
    announce_rate_history: AnnounceRateHistory,
    #[cfg(feature = "runtime-metrics")]
    reliability: ReliabilityMetricsSnapshot,
    on_journaled: J,
}

impl<J> JournalDispatch<J>
where
    J: for<'a> FnMut(Journaled<'a>),
{
    pub(super) fn new(on_journaled: J) -> Self {
        Self {
            delivery: JournalDelivery::default(),
            announce_rate_history: AnnounceRateHistory::default(),
            #[cfg(feature = "runtime-metrics")]
            reliability: ReliabilityMetricsSnapshot::default(),
            on_journaled,
        }
    }

    pub(super) fn route(&mut self, journaled: Journaled<'_>) {
        if let Journaled::AnnounceHeard {
            observation,
            rate_accounting,
        } = &journaled
        {
            self.announce_rate_history.record(
                observation.destination,
                observation.arrived_at,
                *rate_accounting,
            );
        }
        #[cfg(feature = "runtime-metrics")]
        self.reliability.record_journaled(&journaled);
        if let Some(journaled) = self.delivery.route(journaled) {
            (self.on_journaled)(journaled);
        }
    }

    pub(super) fn register_completion(
        &mut self,
        id: CommandId,
        completion: oneshot::Sender<Settlement>,
    ) {
        self.delivery.register_completion(id, completion);
    }

    pub(super) fn register_request(
        &mut self,
        id: CommandId,
        completion: oneshot::Sender<Result<(std::vec::Vec<u8>, RttMillis), SendRequestFailure>>,
    ) {
        self.delivery.register_request(id, completion);
    }

    pub(super) fn fail_request(&mut self, id: CommandId) -> WakeSchedules {
        self.delivery.fail_request(id)
    }

    pub(super) fn register_stream_reader(
        &mut self,
        link_id: LinkId,
        stream_id: StreamId,
        sink: Sender<StreamInbound>,
        failure: oneshot::Sender<StreamReceiveFailure>,
    ) -> Result<(), StreamReaderRegistrationError> {
        self.delivery
            .register_stream_reader(link_id, stream_id, sink, failure)
    }

    pub(super) fn register_resource_sink(
        &mut self,
        link_id: LinkId,
        sink: UnboundedSender<ResourceInbound>,
    ) {
        self.delivery.register_resource_sink(link_id, sink);
    }

    pub(super) fn announce_rate_snapshot(&self, state: AnnounceRateState) -> AnnounceRateSnapshot {
        self.announce_rate_history.snapshot(state)
    }

    #[cfg(feature = "runtime-metrics")]
    pub(super) fn reliability_metrics(&self) -> ReliabilityMetricsSnapshot {
        self.reliability
    }
}

#[derive(Default)]
struct JournalDelivery {
    completions: HashMap<CommandId, oneshot::Sender<Settlement>>,
    requests: HashMap<CommandId, RequestPending>,
    stream_readers: HashMap<(LinkId, StreamId), StreamReaderDelivery>,
    resource_sinks: HashMap<LinkId, UnboundedSender<ResourceInbound>>,
}

enum StreamReaderDelivery {
    Receiving {
        sink: Sender<StreamInbound>,
        failure: oneshot::Sender<StreamReceiveFailure>,
    },
    Discarding,
}

impl StreamReaderDelivery {
    fn fail(&mut self, reason: StreamReceiveFailure) {
        if let Self::Receiving { failure, .. } = core::mem::replace(self, Self::Discarding) {
            let _ = failure.send(reason);
        }
    }
}

impl JournalDelivery {
    fn register_completion(&mut self, id: CommandId, completion: oneshot::Sender<Settlement>) {
        self.completions.insert(id, completion);
    }

    fn register_request(
        &mut self,
        id: CommandId,
        completion: oneshot::Sender<Result<(std::vec::Vec<u8>, RttMillis), SendRequestFailure>>,
    ) {
        self.requests.insert(
            id,
            RequestPending {
                completion,
                data: None,
            },
        );
    }

    fn fail_request(&mut self, id: CommandId) -> WakeSchedules {
        if let Some(entry) = self.requests.remove(&id) {
            let _ = entry.completion.send(Err(SendRequestFailure::WriteFailed));
        }
        WakeSchedules::UNCHANGED
    }

    fn register_stream_reader(
        &mut self,
        link_id: LinkId,
        stream_id: StreamId,
        sink: Sender<StreamInbound>,
        failure: oneshot::Sender<StreamReceiveFailure>,
    ) -> Result<(), StreamReaderRegistrationError> {
        match self.stream_readers.entry((link_id, stream_id)) {
            std::collections::hash_map::Entry::Vacant(entry) => {
                entry.insert(StreamReaderDelivery::Receiving { sink, failure });
                Ok(())
            }
            std::collections::hash_map::Entry::Occupied(_) => {
                let _ = failure.send(StreamReceiveFailure::AlreadyRegistered);
                Err(StreamReaderRegistrationError::AlreadyRegistered)
            }
        }
    }

    fn register_resource_sink(&mut self, link_id: LinkId, sink: UnboundedSender<ResourceInbound>) {
        self.resource_sinks.insert(link_id, sink);
    }

    fn route<'a>(&mut self, journaled: Journaled<'a>) -> Option<Journaled<'a>> {
        let journaled = self.settle_or_forward(journaled)?;
        let journaled = self.route_request_or_forward(journaled)?;
        let journaled = self.route_stream_or_forward(journaled)?;
        self.route_resource_or_forward(journaled)
    }

    fn settle_or_forward<'a>(&mut self, journaled: Journaled<'a>) -> Option<Journaled<'a>> {
        match journaled {
            Journaled::CommandSettled { id, settlement } => {
                if let Some(completion) = self.completions.remove(&id) {
                    let _ = completion.send(settlement);
                    None
                } else {
                    Some(Journaled::CommandSettled { id, settlement })
                }
            }
            journaled => Some(journaled),
        }
    }

    fn route_request_or_forward<'a>(&mut self, journaled: Journaled<'a>) -> Option<Journaled<'a>> {
        match &journaled {
            Journaled::ResponseReceived {
                command_id, data, ..
            } => {
                if let Some(entry) = self.requests.get_mut(command_id) {
                    entry.data = Some(data.to_vec());
                    return None;
                }
            }
            Journaled::ResponseSegmentReceived {
                command_id, data, ..
            } => {
                if let Some(entry) = self.requests.get_mut(command_id) {
                    entry
                        .data
                        .get_or_insert_with(std::vec::Vec::new)
                        .extend_from_slice(data);
                    return None;
                }
            }
            Journaled::CommandSettled {
                id,
                settlement: Settlement::SendRequest(result),
            } => {
                if let Some(entry) = self.requests.remove(id) {
                    let resolved = match (*result, entry.data) {
                        (Ok(delivered), Some(data)) => Ok((data, delivered.rtt)),
                        (Ok(_), None) => Err(SendRequestFailure::WriteFailed),
                        (Err(failure), _) => Err(failure),
                    };
                    let _ = entry.completion.send(resolved);
                    return None;
                }
            }
            Journaled::CommandSettled {
                id,
                settlement: Settlement::SendResource(Err(failure)),
            } => {
                // RequestAny can use a Resource before a response receipt exists.
                // A rejected or failed outbound transfer must settle that same
                // request waiter; successful transfer alone is not a response.
                if let Some(entry) = self.requests.remove(id) {
                    let failure = match failure {
                        SendResourceFailure::Rejected(SendResourceRejection::NoSuchLink) => {
                            SendRequestFailure::Rejected(SendRequestRejection::NoSuchLink)
                        }
                        SendResourceFailure::Rejected(SendResourceRejection::LinkNotActive) => {
                            SendRequestFailure::Rejected(SendRequestRejection::LinkNotActive)
                        }
                        SendResourceFailure::Timeout => SendRequestFailure::Timeout,
                        SendResourceFailure::LinkClosed => SendRequestFailure::LinkClosed,
                        SendResourceFailure::Rejected(_)
                        | SendResourceFailure::WriteFailed
                        | SendResourceFailure::RejectedByPeer
                        | SendResourceFailure::Sequencing
                        | SendResourceFailure::PredecessorFailed => SendRequestFailure::WriteFailed,
                    };
                    let _ = entry.completion.send(Err(failure));
                    return None;
                }
            }
            _ => {}
        }
        Some(journaled)
    }

    fn route_stream_or_forward<'a>(&mut self, journaled: Journaled<'a>) -> Option<Journaled<'a>> {
        if let Journaled::LinkClosed { link_id, .. } = &journaled {
            self.stream_readers.retain(|(reader_link, _), reader| {
                if reader_link != link_id {
                    return true;
                }
                reader.fail(StreamReceiveFailure::LinkClosed);
                false
            });
        }
        if let Journaled::ChannelMessageReceived {
            link_id,
            message_type,
            data,
        } = &journaled
        {
            if *message_type == STREAM_DATA_TYPE {
                if let Ok(frame) = byte_stream::parse(data) {
                    let key = (*link_id, frame.header.stream_id);
                    if let Some(reader) = self.stream_readers.get_mut(&key) {
                        if let StreamReaderDelivery::Receiving { sink, .. } = reader {
                            let inbound = StreamInbound {
                                payload: frame.payload.to_vec(),
                                eof: frame.header.eof,
                                compressed: frame.header.compressed,
                            };
                            match sink.try_send(inbound) {
                                Ok(()) => {}
                                Err(TrySendError::Full(_)) => {
                                    reader.fail(StreamReceiveFailure::Overflowed);
                                }
                                Err(TrySendError::Closed(_)) => {
                                    *reader = StreamReaderDelivery::Discarding;
                                }
                            }
                        }
                        return None;
                    }
                }
            }
        }
        Some(journaled)
    }

    fn route_resource_or_forward<'a>(&mut self, journaled: Journaled<'a>) -> Option<Journaled<'a>> {
        enum ResourceJournal<'a> {
            Received {
                link_id: LinkId,
                hash: ResourceHash,
                metadata: Option<&'a [u8]>,
                data: &'a [u8],
            },
            Segment {
                link_id: LinkId,
                metadata: Option<&'a [u8]>,
                data: &'a [u8],
            },
            Assembled {
                link_id: LinkId,
                original_hash: ResourceHash,
                total_size_bytes: u64,
            },
            Failed {
                link_id: LinkId,
            },
        }

        impl ResourceJournal<'_> {
            fn link_id(&self) -> LinkId {
                match self {
                    Self::Received { link_id, .. }
                    | Self::Segment { link_id, .. }
                    | Self::Assembled { link_id, .. }
                    | Self::Failed { link_id } => *link_id,
                }
            }
        }

        if let Journaled::LinkClosed { link_id, .. } = &journaled {
            if let Some(sink) = self.resource_sinks.remove(link_id) {
                let _ = sink.send(ResourceInbound::Failed);
            }
            return Some(journaled);
        }
        let resource = match &journaled {
            Journaled::ResourceReceived {
                link_id,
                hash,
                metadata,
                data,
            } => ResourceJournal::Received {
                link_id: *link_id,
                hash: *hash,
                metadata: *metadata,
                data,
            },
            Journaled::ResourceSegmentReceived {
                link_id,
                metadata,
                data,
                ..
            } => ResourceJournal::Segment {
                link_id: *link_id,
                metadata: *metadata,
                data,
            },
            Journaled::ResourceAssembled {
                link_id,
                original_hash,
                total_size_bytes,
                ..
            } => ResourceJournal::Assembled {
                link_id: *link_id,
                original_hash: *original_hash,
                total_size_bytes: *total_size_bytes,
            },
            Journaled::ResourceFailed { link_id, .. } => {
                ResourceJournal::Failed { link_id: *link_id }
            }
            _ => return Some(journaled),
        };
        let link = resource.link_id();
        let sink = match self.resource_sinks.get(&link) {
            Some(sink) => sink.clone(),
            None => return Some(journaled),
        };
        let retire = match resource {
            ResourceJournal::Received {
                hash,
                metadata,
                data,
                ..
            } => {
                if let Some(metadata) = metadata {
                    let _ = sink.send(ResourceInbound::Metadata(metadata.to_vec()));
                }
                let _ = sink.send(ResourceInbound::Chunk(data.to_vec()));
                let _ = sink.send(ResourceInbound::Complete {
                    original_hash: hash,
                    total_size_bytes: data.len() as u64,
                });
                true
            }
            ResourceJournal::Segment { metadata, data, .. } => {
                if let Some(metadata) = metadata {
                    let _ = sink.send(ResourceInbound::Metadata(metadata.to_vec()));
                }
                sink.send(ResourceInbound::Chunk(data.to_vec())).is_err()
            }
            ResourceJournal::Assembled {
                original_hash,
                total_size_bytes,
                ..
            } => {
                let _ = sink.send(ResourceInbound::Complete {
                    original_hash,
                    total_size_bytes,
                });
                true
            }
            ResourceJournal::Failed { .. } => {
                let _ = sink.send(ResourceInbound::Failed);
                true
            }
        };
        if retire {
            self.resource_sinks.remove(&link);
        }
        None
    }
}

#[cfg(test)]
mod stream_tests {
    use super::*;
    use crate::engine::LinkClosedReason;
    use crate::routing::links::channel::byte_stream::StreamDataHeader;

    #[test]
    fn duplicate_registration_preserves_the_existing_reader() {
        let mut delivery = JournalDelivery::default();
        let link_id = LinkId::new([7; 16]);
        let stream_id = StreamId::new(3).unwrap();
        let (sink, mut original) = tokio::sync::mpsc::channel(1);
        let (failure, _original_failure) = oneshot::channel();
        delivery
            .register_stream_reader(link_id, stream_id, sink, failure)
            .unwrap();
        let (duplicate, _inbound) = tokio::sync::mpsc::channel(1);
        let (failure, mut duplicate_failure) = oneshot::channel();
        assert_eq!(
            delivery.register_stream_reader(link_id, stream_id, duplicate, failure),
            Err(StreamReaderRegistrationError::AlreadyRegistered)
        );
        assert_eq!(
            duplicate_failure.try_recv(),
            Ok(StreamReceiveFailure::AlreadyRegistered)
        );
        let header = StreamDataHeader {
            stream_id,
            eof: false,
            compressed: false,
        }
        .to_bytes();
        let frame = [header[0], header[1], b'x'];
        delivery.route_stream_or_forward(Journaled::ChannelMessageReceived {
            link_id,
            message_type: STREAM_DATA_TYPE,
            data: &frame,
        });
        assert_eq!(original.try_recv().unwrap().payload, b"x");
    }

    #[test]
    fn full_stream_reader_is_marked_failed_without_unbounded_buffering() {
        let mut delivery = JournalDelivery::default();
        let link_id = LinkId::new([7; 16]);
        let stream_id = StreamId::new(3).unwrap();
        let (sink, mut inbound) = tokio::sync::mpsc::channel(1);
        let (failure, mut outcome) = oneshot::channel();
        delivery
            .register_stream_reader(link_id, stream_id, sink, failure)
            .unwrap();
        let header = StreamDataHeader {
            stream_id,
            eof: false,
            compressed: false,
        }
        .to_bytes();
        let frame = [header[0], header[1], b'x'];
        for _ in 0..2 {
            assert!(delivery
                .route_stream_or_forward(Journaled::ChannelMessageReceived {
                    link_id,
                    message_type: STREAM_DATA_TYPE,
                    data: &frame,
                })
                .is_none());
        }
        assert_eq!(outcome.try_recv(), Ok(StreamReceiveFailure::Overflowed));
        assert_eq!(inbound.try_recv().unwrap().payload, b"x");
        assert!(inbound.try_recv().is_err());

        delivery.route_stream_or_forward(Journaled::LinkClosed {
            link_id,
            reason: LinkClosedReason::PeerClosed,
        });
        assert!(delivery.stream_readers.is_empty());
    }

    #[test]
    fn dropped_reader_keeps_later_stream_frames_out_of_app_events() {
        let mut delivery = JournalDelivery::default();
        let link_id = LinkId::new([8; 16]);
        let stream_id = StreamId::new(4).unwrap();
        let (sink, inbound) = tokio::sync::mpsc::channel(1);
        let (failure, _outcome) = oneshot::channel();
        delivery
            .register_stream_reader(link_id, stream_id, sink, failure)
            .unwrap();
        drop(inbound);
        let header = StreamDataHeader {
            stream_id,
            eof: false,
            compressed: false,
        }
        .to_bytes();
        let frame = [header[0], header[1], b'x'];
        for _ in 0..2 {
            assert!(delivery
                .route_stream_or_forward(Journaled::ChannelMessageReceived {
                    link_id,
                    message_type: STREAM_DATA_TYPE,
                    data: &frame,
                })
                .is_none());
        }
        assert!(delivery
            .stream_readers
            .get(&(link_id, stream_id))
            .is_some_and(|reader| matches!(reader, StreamReaderDelivery::Discarding)));
    }

    #[test]
    fn closing_a_link_reports_failure_to_its_reader() {
        let mut delivery = JournalDelivery::default();
        let link_id = LinkId::new([9; 16]);
        let stream_id = StreamId::new(5).unwrap();
        let (sink, _inbound) = tokio::sync::mpsc::channel(1);
        let (failure, mut outcome) = oneshot::channel();
        delivery
            .register_stream_reader(link_id, stream_id, sink, failure)
            .unwrap();
        delivery.route_stream_or_forward(Journaled::LinkClosed {
            link_id,
            reason: LinkClosedReason::PeerClosed,
        });
        assert_eq!(outcome.try_recv(), Ok(StreamReceiveFailure::LinkClosed));
        assert!(delivery.stream_readers.is_empty());
    }
}

#[cfg(test)]
mod tests;
