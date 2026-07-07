mod announce;
mod channel;
mod link;
mod path;
mod request;
mod resource;
mod rpc;
mod send_group;
mod send_single;

pub use announce::{
    AnnounceAppData, AnnounceNow, AnnounceNowFailure, AnnounceNowRejection, AnnounceTarget,
};
pub use channel::{
    SendToChannel, SendToChannelBody, SendToChannelFailure, SendToChannelRejection,
    MAX_SEND_TO_CHANNEL_BODY_LEN,
};
pub use link::{
    CloseLink, CloseLinkFailure, CloseLinkRejection, EstablishLink, EstablishLinkFailure,
    EstablishLinkRejection, Identify, IdentifyFailure, IdentifyRejection, LinkEstablished,
    SendToLink, SendToLinkFailure, SendToLinkPayload, SendToLinkRejection,
    MAX_SEND_TO_LINK_PLAINTEXT_LEN,
};
pub use path::{PathFound, PathRequestId, RequestPath, RequestPathFailure, PATH_REQUEST_ID_LEN};
pub use request::{
    AllowRequester, AllowRequesterFailure, AllowRequesterRejection, Respond, RespondData,
    RespondFailure, RespondRejection, SendRequest, SendRequestData, SendRequestFailure,
    SendRequestRejection, MAX_RESPOND_DATA_LEN, MAX_SEND_REQUEST_DATA_LEN,
};
pub use resource::{
    SendResourceFailure, SendResourceRejection, SetResourceStrategy, SetResourceStrategyFailure,
    SetResourceStrategyRejection,
};
pub use send_group::{SendGroup, SendGroupFailure, SendGroupPayload, MAX_SEND_GROUP_PLAINTEXT_LEN};
pub use send_single::{
    SendSinglePacket, SendSinglePacketFailure, SendSinglePacketPayload, SendSinglePacketRejection,
    MAX_SEND_SINGLE_PACKET_PLAINTEXT_LEN,
};

#[cfg(feature = "alloc")]
pub use rpc::RpcPathEntry;
pub use rpc::{RpcQuery, RpcQueryResult};

use crate::engine::EngineState;
use crate::interfaces::{InterfaceDescriptor, InterfaceId};
use crate::storage::StorageLayout;
use crate::units::RttMillis;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CommandId(pub u64);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssuedCommand {
    pub id: CommandId,
    pub command: EngineCommand,
}

#[derive(Debug, Clone, PartialEq, Eq)]
// repr(C) is CRITICAL here and on every enum that crosses the dual-core embassy channels
// (EngineCommand, Settlement, EngineReaction, Journaled, Directive, InterfaceLifecycle):
// the esp Xtensa toolchain miscompiled the default repr(Rust) layout, and core 1 read
// Directive's fan target at the wrong offset, corrupting the supervisor's match into UB.
// Proven on hardware both broken and fixed; do not remove.
#[repr(C)]
pub enum EngineCommand {
    AnnounceNow(AnnounceNow),
    SendSinglePacket(SendSinglePacket),
    SendGroup(SendGroup),
    RequestPath(RequestPath),
    EstablishLink(EstablishLink),
    SendToLink(SendToLink),
    SendToChannel(SendToChannel),
    Identify(Identify),
    SendRequest(SendRequest),
    Respond(Respond),
    CloseLink(CloseLink),
    SetResourceStrategy(SetResourceStrategy),
    AllowRequester(AllowRequester),
    RpcQuery(RpcQuery),
}

// The Owes* variants hand the caller its whole command payload back (SendSinglePacket rides
// ~400B of heapless body) beside slim rejections. Outcomes are transient by-value
// returns, destructured immediately, and the no-alloc core has no Box to shrink them.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandOutcome {
    OwesAnnounce {
        id: CommandId,
        announce: AnnounceNow,
    },
    AnnounceRejected {
        id: CommandId,
        rejection: AnnounceNowRejection,
    },
    OwesSendSinglePacket {
        id: CommandId,
        send: SendSinglePacket,
    },
    SendSinglePacketRejected {
        id: CommandId,
        rejection: SendSinglePacketRejection,
    },
    OwesSendGroup {
        id: CommandId,
        send: SendGroup,
    },
    SendGroupRejected {
        id: CommandId,
    },
    OwesPathRequest {
        id: CommandId,
        request: RequestPath,
    },
    OwesLinkRequest {
        id: CommandId,
        establish: EstablishLink,
    },
    EstablishLinkRejected {
        id: CommandId,
        rejection: EstablishLinkRejection,
    },
    OwesSendToLink {
        id: CommandId,
        send: SendToLink,
    },
    OwesIdentify {
        id: CommandId,
        identify: Identify,
    },
    OwesSendRequest {
        id: CommandId,
        request: SendRequest,
    },
    SendRequestRejected {
        id: CommandId,
        rejection: SendRequestRejection,
    },
    OwesRespond {
        id: CommandId,
        respond: Respond,
    },
    RespondRejected {
        id: CommandId,
        rejection: RespondRejection,
    },
    IdentifyRejected {
        id: CommandId,
        rejection: IdentifyRejection,
    },
    SendToLinkRejected {
        id: CommandId,
        rejection: SendToLinkRejection,
    },
    OwesSendToChannel {
        id: CommandId,
        send: SendToChannel,
    },
    SendToChannelRejected {
        id: CommandId,
        failure: SendToChannelFailure,
    },
    ResourceStrategySet {
        id: CommandId,
    },
    SetResourceStrategyRejected {
        id: CommandId,
        rejection: SetResourceStrategyRejection,
    },
    RequesterAllowed {
        id: CommandId,
    },
    AllowRequesterRejected {
        id: CommandId,
        rejection: AllowRequesterRejection,
    },
    OwesLinkClose {
        id: CommandId,
        close: CloseLink,
    },
    CloseLinkRejected {
        id: CommandId,
        rejection: CloseLinkRejection,
    },
    RpcQueryRead {
        id: CommandId,
        result: RpcQueryResult,
    },
}

/// Paired verb-for-verb with [`EngineCommand`]: a data boundary erases type-level ties, so the tie is explicit here.
#[derive(Debug, Clone, PartialEq, Eq)]
// repr(C): crosses the dual-core channel; see the layout note on [`EngineCommand`].
#[repr(C)]
pub enum Settlement {
    AnnounceNow(Result<(), AnnounceNowFailure>),
    SendSinglePacket(Result<PacketReceiptDelivered, SendSinglePacketFailure>),
    SendGroup(Result<(), SendGroupFailure>),
    RequestPath(Result<PathFound, RequestPathFailure>),
    EstablishLink(Result<LinkEstablished, EstablishLinkFailure>),
    SendToLink(Result<PacketReceiptDelivered, SendToLinkFailure>),
    Identify(Result<(), IdentifyFailure>),
    SendRequest(Result<PacketReceiptDelivered, SendRequestFailure>),
    Respond(Result<(), RespondFailure>),
    CloseLink(Result<(), CloseLinkFailure>),
    SendResource(Result<(), SendResourceFailure>),
    SetResourceStrategy(Result<(), SetResourceStrategyFailure>),
    SendToChannel(Result<PacketReceiptDelivered, SendToChannelFailure>),
    AllowRequester(Result<(), AllowRequesterFailure>),
    RpcQuery(RpcQueryResult),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct InterfaceCounts {
    pub destinations: u32,
    pub links: u32,
    pub transported_links: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PacketReceiptDelivered {
    pub rtt: RttMillis,
}

/// A command's `*Rejection` enum names the reasons ingest refuses it at the door; its `*Failure` enum is everything the awaiting caller can see, wrapping those same door refusals as `Rejected(*Rejection)` beside the ways an accepted command can still fail later, where a broken lower layer surfaces as a `*Error` payload.
/// A command that cannot be refused at the door has no `*Rejection`, and one with a single refusal reason may inline it.
pub trait Settleable {
    type Success;
    type Failure;

    fn into_command(self) -> EngineCommand;
    fn from_settlement(settlement: Settlement) -> Option<Result<Self::Success, Self::Failure>>;
}

impl<S: StorageLayout> EngineState<S> {
    #[must_use]
    pub fn ingest_command(
        &mut self,
        issued: IssuedCommand,
        interfaces: &[InterfaceDescriptor],
    ) -> CommandOutcome {
        self.ingested_command_count = self.ingested_command_count.saturating_add(1);
        let IssuedCommand { id, command } = issued;
        match command {
            EngineCommand::AnnounceNow(announce_now) => {
                self.ingest_announce_now(id, announce_now, interfaces)
            }
            EngineCommand::SendSinglePacket(send) => self.ingest_send_single_packet(id, send),
            EngineCommand::SendGroup(send) => self.ingest_send_group(id, send),
            EngineCommand::RequestPath(request) => CommandOutcome::OwesPathRequest { id, request },
            EngineCommand::EstablishLink(establish) => self.ingest_establish_link(id, establish),
            EngineCommand::SendToLink(send) => self.ingest_send_to_link(id, send),
            EngineCommand::SendToChannel(send) => self.ingest_send_to_channel(id, send),
            EngineCommand::Identify(identify) => self.ingest_identify(id, identify),
            EngineCommand::SendRequest(request) => self.ingest_send_request(id, request),
            EngineCommand::Respond(respond) => self.ingest_respond(id, respond),
            EngineCommand::CloseLink(close) => self.ingest_close_link(id, close),
            EngineCommand::SetResourceStrategy(set) => self.ingest_set_resource_strategy(id, set),
            EngineCommand::AllowRequester(allow) => self.ingest_allow_requester_command(id, allow),
            EngineCommand::RpcQuery(query) => CommandOutcome::RpcQueryRead {
                id,
                result: self.run_rpc_query(query),
            },
        }
    }

    pub fn interface_counts(&self, interface: InterfaceId) -> InterfaceCounts {
        InterfaceCounts {
            destinations: self.route_count_via(interface) as u32,
            links: self.link_count_via(interface) as u32,
            transported_links: self.transported_link_count_via(interface) as u32,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::*;

    #[test]
    fn each_outcome_echoes_its_own_command_id() {
        let mut state = personal_node_announcer();
        let destination = personal_node_destination();
        let issued_as = |id| IssuedCommand {
            id,
            command: EngineCommand::AnnounceNow(AnnounceNow {
                destination,
                target: AnnounceTarget::AllInterfaces,
                app_data: AnnounceAppData::Registered,
            }),
        };

        for id in [CommandId(0), CommandId(42), CommandId(u64::MAX)] {
            assert_eq!(
                state.ingest_command(issued_as(id), &[]),
                CommandOutcome::OwesAnnounce {
                    id,
                    announce: AnnounceNow {
                        destination,
                        target: AnnounceTarget::AllInterfaces,
                        app_data: AnnounceAppData::Registered,
                    },
                },
            );
        }
    }
}
