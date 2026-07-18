use core::cell::RefCell;
use std::collections::HashMap;

use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::oneshot;

use crate::engine::{CommandId, Journaled, SendRequestFailure, Settlement, WakeSchedules};
use crate::routing::links::channel::byte_stream::{self, StreamId, STREAM_DATA_TYPE};
use crate::routing::links::LinkId;
use crate::units::RttMillis;

use super::host_protocol::{ResourceInbound, StreamInbound};

struct RequestPending {
    completion: oneshot::Sender<Result<(std::vec::Vec<u8>, RttMillis), SendRequestFailure>>,
    data: Option<std::vec::Vec<u8>>,
}

#[derive(Default)]
pub(super) struct JournalDelivery {
    completions: RefCell<HashMap<CommandId, oneshot::Sender<Settlement>>>,
    requests: RefCell<HashMap<CommandId, RequestPending>>,
    stream_readers: RefCell<HashMap<(LinkId, StreamId), UnboundedSender<StreamInbound>>>,
    resource_sinks: RefCell<HashMap<LinkId, UnboundedSender<ResourceInbound>>>,
}

impl JournalDelivery {
    pub(super) fn register_completion(
        &self,
        id: CommandId,
        completion: oneshot::Sender<Settlement>,
    ) {
        self.completions.borrow_mut().insert(id, completion);
    }

    pub(super) fn register_request(
        &self,
        id: CommandId,
        completion: oneshot::Sender<Result<(std::vec::Vec<u8>, RttMillis), SendRequestFailure>>,
    ) {
        self.requests.borrow_mut().insert(
            id,
            RequestPending {
                completion,
                data: None,
            },
        );
    }

    pub(super) fn fail_request(&self, id: CommandId) -> WakeSchedules {
        if let Some(entry) = self.requests.borrow_mut().remove(&id) {
            let _ = entry.completion.send(Err(SendRequestFailure::WriteFailed));
        }
        WakeSchedules::UNCHANGED
    }

    pub(super) fn register_stream_reader(
        &self,
        link_id: LinkId,
        stream_id: StreamId,
        sink: UnboundedSender<StreamInbound>,
    ) {
        self.stream_readers
            .borrow_mut()
            .insert((link_id, stream_id), sink);
    }

    pub(super) fn register_resource_sink(
        &self,
        link_id: LinkId,
        sink: UnboundedSender<ResourceInbound>,
    ) {
        self.resource_sinks.borrow_mut().insert(link_id, sink);
    }

    pub(super) fn route<'a>(&self, journaled: Journaled<'a>) -> Option<Journaled<'a>> {
        let journaled = self.settle_or_forward(journaled)?;
        let journaled = self.route_request_or_forward(journaled)?;
        let journaled = self.route_stream_or_forward(journaled)?;
        self.route_resource_or_forward(journaled)
    }

    fn settle_or_forward<'a>(&self, journaled: Journaled<'a>) -> Option<Journaled<'a>> {
        if let Journaled::CommandSettled { id, settlement } = &journaled {
            if let Some(completion) = self.completions.borrow_mut().remove(id) {
                let _ = completion.send(settlement.clone());
                return None;
            }
        }
        Some(journaled)
    }

    fn route_request_or_forward<'a>(&self, journaled: Journaled<'a>) -> Option<Journaled<'a>> {
        match &journaled {
            Journaled::ResponseReceived {
                command_id, data, ..
            } => {
                if let Some(entry) = self.requests.borrow_mut().get_mut(command_id) {
                    entry.data = Some(data.to_vec());
                    return None;
                }
            }
            Journaled::ResponseSegmentReceived {
                command_id, data, ..
            } => {
                if let Some(entry) = self.requests.borrow_mut().get_mut(command_id) {
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
                if let Some(entry) = self.requests.borrow_mut().remove(id) {
                    let resolved = match (*result, entry.data) {
                        (Ok(delivered), Some(data)) => Ok((data, delivered.rtt)),
                        (Ok(_), None) => Err(SendRequestFailure::WriteFailed),
                        (Err(failure), _) => Err(failure),
                    };
                    let _ = entry.completion.send(resolved);
                    return None;
                }
            }
            _ => {}
        }
        Some(journaled)
    }

    fn route_stream_or_forward<'a>(&self, journaled: Journaled<'a>) -> Option<Journaled<'a>> {
        if let Journaled::ChannelMessageReceived {
            link_id,
            message_type,
            data,
        } = &journaled
        {
            if *message_type == STREAM_DATA_TYPE {
                if let Ok(frame) = byte_stream::parse(data) {
                    let key = (*link_id, frame.header.stream_id);
                    let mut readers = self.stream_readers.borrow_mut();
                    if let Some(sink) = readers.get(&key) {
                        let inbound = StreamInbound {
                            payload: frame.payload.to_vec(),
                            eof: frame.header.eof,
                            compressed: frame.header.compressed,
                        };
                        if sink.send(inbound).is_err() {
                            readers.remove(&key);
                        }
                        return None;
                    }
                }
            }
        }
        Some(journaled)
    }

    fn route_resource_or_forward<'a>(&self, journaled: Journaled<'a>) -> Option<Journaled<'a>> {
        let link = match &journaled {
            Journaled::ResourceReceived { link_id, .. }
            | Journaled::ResourceSegmentReceived { link_id, .. }
            | Journaled::ResourceAssembled { link_id, .. }
            | Journaled::ResourceFailed { link_id, .. } => *link_id,
            _ => return Some(journaled),
        };
        let sink = match self.resource_sinks.borrow().get(&link) {
            Some(sink) => sink.clone(),
            None => return Some(journaled),
        };
        let retire = match &journaled {
            Journaled::ResourceReceived {
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
                    original_hash: *hash,
                    total_size: data.len() as u64,
                });
                true
            }
            Journaled::ResourceSegmentReceived { metadata, data, .. } => {
                if let Some(metadata) = metadata {
                    let _ = sink.send(ResourceInbound::Metadata(metadata.to_vec()));
                }
                sink.send(ResourceInbound::Chunk(data.to_vec())).is_err()
            }
            Journaled::ResourceAssembled {
                original_hash,
                total_size,
                ..
            } => {
                let _ = sink.send(ResourceInbound::Complete {
                    original_hash: *original_hash,
                    total_size: *total_size,
                });
                true
            }
            Journaled::ResourceFailed { .. } => {
                let _ = sink.send(ResourceInbound::Failed);
                true
            }
            _ => unreachable!("the link only matched a resource journal above"),
        };
        if retire {
            self.resource_sinks.borrow_mut().remove(&link);
        }
        None
    }
}

#[cfg(test)]
mod tests;
