mod announce;
mod channel;
mod link;
mod path;
mod registered_announce_app_data;
mod request;
mod resource;
mod send_group;
mod send_plain;
mod send_single;

use crate::routing::dedup::PacketHash;

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
pub use registered_announce_app_data::{
    SetRegisteredAnnounceAppData, SetRegisteredAnnounceAppDataFailure,
    SetRegisteredAnnounceAppDataRejection,
};
pub use request::{
    AllowRequester, AllowRequesterFailure, AllowRequesterRejection, RequestResponseTimeout,
    Respond, RespondData, RespondFailure, RespondPayload, RespondRejection, SendRequest,
    SendRequestData, SendRequestFailure, SendRequestRejection, MAX_RESPOND_DATA_LEN,
    MAX_SEND_REQUEST_DATA_LEN,
};
pub use resource::{
    SendResourceFailure, SendResourceRejection, SetResourceStrategy, SetResourceStrategyFailure,
    SetResourceStrategyRejection,
};
pub use send_group::{
    SendGroup, SendGroupFailure, SendGroupPayload, SendGroupRejection, MAX_SEND_GROUP_PLAINTEXT_LEN,
};
pub use send_plain::{
    SendPlainPacket, SendPlainPacketFailure, SendPlainPacketPayload,
    MAX_SEND_PLAIN_PACKET_PAYLOAD_LEN,
};
pub use send_single::{
    SendSinglePacket, SendSinglePacketFailure, SendSinglePacketPayload, SendSinglePacketRejection,
    MAX_SEND_SINGLE_PACKET_PLAINTEXT_LEN,
};

use crate::engine::EngineState;
use crate::interfaces::AttachedInterfaces;
use crate::interfaces::InterfaceId;
use crate::storage::StorageLayout;
use crate::units::RttMillis;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CommandId(pub u64);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssuedCommand {
    pub id: CommandId,
    pub command: PrnsCommand,
}

#[derive(Debug, Clone, PartialEq, Eq)]
// repr(C) is CRITICAL here and on every enum that crosses the dual-core embassy channels (PrnsCommand, Settlement, EngineReaction, Journaled, Directive, InterfaceLifecycle): the esp Xtensa toolchain miscompiled the default repr(Rust) layout, and core 1 read Directive's fan target at the wrong offset, corrupting the supervisor's match into UB.
// Proven on hardware both broken and fixed; do not remove.
#[repr(C)]
pub enum PrnsCommand {
    AnnounceNow(AnnounceNow),
    SetRegisteredAnnounceAppData(SetRegisteredAnnounceAppData),
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
    SendPlainPacket(SendPlainPacket),
}

// The Owes* variants hand the caller its whole command payload back (SendSinglePacket rides ~400B of heapless body) beside slim rejections. Outcomes are transient by-value returns, destructured immediately, and the no-alloc core has no Box to shrink them.
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
    RegisteredAnnounceAppDataSet {
        id: CommandId,
    },
    SetRegisteredAnnounceAppDataRejected {
        id: CommandId,
        rejection: SetRegisteredAnnounceAppDataRejection,
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
        rejection: SendGroupRejection,
    },
    OwesSendPlainPacket {
        id: CommandId,
        send: SendPlainPacket,
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
    OwesResourceResponse {
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
}

/// Paired verb-for-verb with [`PrnsCommand`]: a data boundary erases type-level ties, so the tie is explicit here.
#[derive(Debug, Clone, PartialEq, Eq)]
// repr(C): crosses the dual-core channel; see the layout note on [`PrnsCommand`].
#[repr(C)]
pub enum Settlement {
    AnnounceNow(Result<(), AnnounceNowFailure>),
    SetRegisteredAnnounceAppData(Result<(), SetRegisteredAnnounceAppDataFailure>),
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
    SendPlainPacket(Result<(), SendPlainPacketFailure>),
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
    pub evidence: DeliveryEvidence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryEvidence {
    Proof(DeliveryProof),
    Response,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeliveryProof {
    Explicit(PacketHash),
    Implicit(PacketHash),
}

impl DeliveryProof {
    pub const fn packet_hash(self) -> PacketHash {
        match self {
            Self::Explicit(packet_hash) | Self::Implicit(packet_hash) => packet_hash,
        }
    }
}

/// A command's `*Rejection` enum names the reasons ingest refuses it at the door; its `*Failure` enum is everything the awaiting caller can see, wrapping those same door refusals as `Rejected(*Rejection)` beside the ways an accepted command can still fail later, where a broken lower layer surfaces as a `*Error` payload.
/// A command that cannot be refused at the door has no `*Rejection`, and one with a single refusal reason may inline it.
pub trait Settleable {
    type Success;
    type Failure;

    fn into_command(self) -> PrnsCommand;
    fn from_settlement(settlement: Settlement) -> Option<Result<Self::Success, Self::Failure>>;
}

impl<S: StorageLayout> EngineState<S> {
    #[must_use]
    pub fn ingest_command(
        &mut self,
        issued: IssuedCommand,
        interfaces: AttachedInterfaces<'_>,
    ) -> CommandOutcome {
        self.ingested_command_count = self.ingested_command_count.saturating_add(1);
        let IssuedCommand { id, command } = issued;
        match command {
            PrnsCommand::AnnounceNow(announce_now) => {
                self.ingest_announce_now(id, announce_now, interfaces)
            }
            PrnsCommand::SetRegisteredAnnounceAppData(set) => {
                self.ingest_set_registered_announce_app_data(id, set)
            }
            PrnsCommand::SendSinglePacket(send) => self.ingest_send_single_packet(id, send),
            PrnsCommand::SendGroup(send) => self.ingest_send_group(id, send),
            PrnsCommand::SendPlainPacket(send) => self.ingest_send_plain_packet(id, send),
            PrnsCommand::RequestPath(request) => CommandOutcome::OwesPathRequest { id, request },
            PrnsCommand::EstablishLink(establish) => self.ingest_establish_link(id, establish),
            PrnsCommand::SendToLink(send) => self.ingest_send_to_link(id, send),
            PrnsCommand::SendToChannel(send) => self.ingest_send_to_channel(id, send),
            PrnsCommand::Identify(identify) => self.ingest_identify(id, identify),
            PrnsCommand::SendRequest(request) => self.ingest_send_request(id, request),
            PrnsCommand::Respond(respond) => self.ingest_respond(id, respond),
            PrnsCommand::CloseLink(close) => self.ingest_close_link(id, close),
            PrnsCommand::SetResourceStrategy(set) => self.ingest_set_resource_strategy(id, set),
            PrnsCommand::AllowRequester(allow) => self.ingest_allow_requester_command(id, allow),
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
            command: PrnsCommand::AnnounceNow(AnnounceNow {
                destination,
                target: AnnounceTarget::AllInterfaces,
                app_data: AnnounceAppData::Registered,
            }),
        };

        for id in [CommandId(0), CommandId(42), CommandId(u64::MAX)] {
            assert_eq!(
                state.ingest_command(issued_as(id), AttachedInterfaces::new(&[])),
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
