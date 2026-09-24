use personal_rns::engine::{
    CommandId, LinkClosedReason, PacketReceiptDelivered, RespondFailure, SendRequestFailure,
    SendResourceFailure, SendToChannelFailure, SetResourceStrategyFailure, Settlement,
};
use personal_rns::identity::IdentityHash;
use personal_rns::routing::links::channel::MessageType;
use personal_rns::routing::links::resources::{ResourceFailureCause, ResourceHash};
use personal_rns::routing::links::LinkId;
use personal_rns::runtime::{Diagnostic, Message, PrnsEvent};
use std::cell::RefCell;
use std::rc::Rc;

const EVENT_CAPACITY: usize = 32768;
const PAYLOAD_CAPACITY: usize = 32768;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandOutcome {
    Channel(Result<PacketReceiptDelivered, SendToChannelFailure>),
    Request(Result<PacketReceiptDelivered, SendRequestFailure>),
    Respond(Result<(), RespondFailure>),
    Resource(Result<(), SendResourceFailure>),
    Strategy(Result<(), SetResourceStrategyFailure>),
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Announce {
        destination: personal_rns::wire::DestinationHash,
    },
    Channel {
        link: LinkId,
        kind: MessageType,
        bytes: Vec<u8>,
    },
    Resource {
        link: LinkId,
        hash: ResourceHash,
        bytes: Vec<u8>,
    },
    Segment {
        link: LinkId,
        hash: ResourceHash,
        index: u64,
        total: u64,
        bytes: Vec<u8>,
    },
    Settled {
        id: CommandId,
        settlement: CommandOutcome,
    },
    Closed {
        link: LinkId,
        reason: LinkClosedReason,
    },
    Identified {
        link: LinkId,
        identity: IdentityHash,
    },
    ResourceFailed {
        link: LinkId,
        hash: ResourceHash,
        cause: ResourceFailureCause,
    },
    ResourceAssembled {
        link: LinkId,
        hash: ResourceHash,
        bytes: u64,
    },
}

#[derive(Clone)]
pub struct EventLog(Rc<RefCell<Vec<Event>>>);
impl EventLog {
    pub fn new() -> Self {
        Self(Rc::new(RefCell::new(Vec::new())))
    }
    pub fn snapshot(&self) -> Vec<Event> {
        self.0.borrow().clone()
    }
    pub fn observe(&self, event: &PrnsEvent<'_>) {
        let event = match event {
            PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard { destination, .. }) => {
                Event::Announce {
                    destination: *destination,
                }
            }
            PrnsEvent::Message(Message::ChannelMessage {
                link_id,
                message_type,
                data,
            }) => Event::Channel {
                link: *link_id,
                kind: *message_type,
                bytes: bounded(data),
            },
            PrnsEvent::Message(Message::Resource {
                link_id,
                hash,
                data,
                ..
            }) => Event::Resource {
                link: *link_id,
                hash: *hash,
                bytes: bounded(data),
            },
            PrnsEvent::Message(Message::ResourceSegment {
                link_id,
                original_hash,
                segment_index,
                total_segments,
                data,
                ..
            }) => Event::Segment {
                link: *link_id,
                hash: *original_hash,
                index: *segment_index,
                total: *total_segments,
                bytes: bounded(data),
            },
            PrnsEvent::Diagnostic(Diagnostic::CommandSettled { id, settlement }) => {
                Event::Settled {
                    id: *id,
                    settlement: match settlement {
                        Settlement::SendToChannel(result) => CommandOutcome::Channel(*result),
                        Settlement::SendRequest(result) => CommandOutcome::Request(*result),
                        Settlement::Respond(result) => CommandOutcome::Respond(*result),
                        Settlement::SendResource(result) => CommandOutcome::Resource(*result),
                        Settlement::SetResourceStrategy(result) => {
                            CommandOutcome::Strategy(*result)
                        }
                        _ => return,
                    },
                }
            }
            PrnsEvent::Diagnostic(Diagnostic::LinkClosed { link_id, reason }) => Event::Closed {
                link: *link_id,
                reason: *reason,
            },
            PrnsEvent::Diagnostic(Diagnostic::PeerIdentified { link_id, identity }) => {
                Event::Identified {
                    link: *link_id,
                    identity: *identity,
                }
            }
            PrnsEvent::Diagnostic(Diagnostic::ResourceFailed {
                link_id,
                hash,
                cause,
            }) => Event::ResourceFailed {
                link: *link_id,
                hash: *hash,
                cause: *cause,
            },
            PrnsEvent::Diagnostic(Diagnostic::ResourceAssembled {
                link_id,
                original_hash,
                total_size_bytes,
            }) => Event::ResourceAssembled {
                link: *link_id,
                hash: *original_hash,
                bytes: *total_size_bytes,
            },
            _ => return,
        };
        let mut events = self.0.borrow_mut();
        assert!(events.len() < EVENT_CAPACITY, "bounded node event evidence");
        events.push(event);
    }
}
fn bounded(bytes: &[u8]) -> Vec<u8> {
    assert!(
        bytes.len() <= PAYLOAD_CAPACITY,
        "bounded delivered payload evidence"
    );
    bytes.to_vec()
}
