//! App-issued commands, ingested by the engine as plain data.
//! Commands cross thread, task, and FFI boundaries as owned values,
//! so any host can queue them and the engine cycle drains them deterministically.
//!
//! RNS 1.3.1 has no scheduled announces at all: `Destination.announce()` is
//! app-called, and periodic announcing lives in app land (LXMF runs its own
//! timers). So [`AnnounceNow`] is the reference primitive, and this engine's
//! re-announce schedule is the extension built ahead of it.
//!
//! This module holds the spine — the command envelope, the per-cycle
//! [`CommandOutcome`], the terminal [`Settlement`], and the dispatcher — and
//! each verb family lives in its own submodule, re-exported flat here.

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
    AnnounceAppData, AnnounceNow, AnnounceNowError, AnnounceNowFailure, AnnounceTarget,
};
pub use channel::{
    SendChannel, SendChannelBody, SendChannelError, SendChannelFailure, MAX_SEND_CHANNEL_BODY_LEN,
};
pub use link::{
    CloseLink, CloseLinkError, CloseLinkFailure, EstablishLink, EstablishLinkError,
    EstablishLinkFailure, Identify, IdentifyError, IdentifyFailure, LinkEstablished, SendLink,
    SendLinkError, SendLinkFailure, SendLinkPayload, MAX_SEND_LINK_PLAINTEXT_LEN,
};
pub use path::{PathFound, PathRequestId, RequestPath, RequestPathFailure, PATH_REQUEST_ID_LEN};
pub use request::{
    AllowRequester, AllowRequesterError, AllowRequesterFailure, Respond, RespondData, RespondError,
    RespondFailure, SendRequest, SendRequestData, SendRequestError, SendRequestFailure,
    MAX_RESPOND_DATA_LEN, MAX_SEND_REQUEST_DATA_LEN,
};
pub use resource::{
    SendResourceError, SendResourceFailure, SetResourceStrategy, SetResourceStrategyError,
    SetResourceStrategyFailure,
};
#[cfg(feature = "alloc")]
pub use rpc::RpcPathEntry;
pub use rpc::{RpcQuery, RpcQueryResult};
pub use send_group::{SendGroup, SendGroupFailure, SendGroupPayload, MAX_SEND_GROUP_PLAINTEXT_LEN};
pub use send_single::{
    SendSingle, SendSingleError, SendSingleFailure, SendSinglePayload,
    MAX_SEND_SINGLE_PLAINTEXT_LEN,
};

use crate::engine::EngineState;
use crate::interfaces::InterfaceConfig;
#[cfg(any(feature = "tokio-host", feature = "embassy-host"))]
use crate::interfaces::InterfaceId;
use crate::storage::StorageLayout;
use crate::units::Rtt;

/// Ephemeral correlation for one issued command: minted by the caller (a
/// queued command has no return channel at submit time), echoed opaquely
/// through every outcome, never inspected by the engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CommandId(pub u64);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssuedCommand {
    pub id: CommandId,
    pub command: EngineCommand,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[repr(C)]
pub enum EngineCommand {
    AnnounceNow(AnnounceNow),
    SendSingle(SendSingle),
    SendGroup(SendGroup),
    RequestPath(RequestPath),
    EstablishLink(EstablishLink),
    SendLink(SendLink),
    SendChannel(SendChannel),
    Identify(Identify),
    SendRequest(SendRequest),
    Respond(Respond),
    CloseLink(CloseLink),
    SetResourceStrategy(SetResourceStrategy),
    AllowRequester(AllowRequester),
    RpcQuery(RpcQuery),
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandOutcome {
    OwesAnnounce {
        id: CommandId,
        announce: AnnounceNow,
    },
    AnnounceRejected {
        id: CommandId,
        error: AnnounceNowError,
    },
    OwesSendSingle {
        id: CommandId,
        send: SendSingle,
    },
    SendSingleRejected {
        id: CommandId,
        error: SendSingleError,
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
        error: EstablishLinkError,
    },
    OwesSendLink {
        id: CommandId,
        send: SendLink,
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
        error: SendRequestError,
    },
    OwesRespond {
        id: CommandId,
        respond: Respond,
    },
    RespondRejected {
        id: CommandId,
        error: RespondError,
    },
    IdentifyRejected {
        id: CommandId,
        error: IdentifyError,
    },
    SendLinkRejected {
        id: CommandId,
        error: SendLinkError,
    },
    OwesSendChannel {
        id: CommandId,
        send: SendChannel,
    },
    SendChannelRejected {
        id: CommandId,
        failure: SendChannelFailure,
    },
    ResourceStrategySet {
        id: CommandId,
    },
    SetResourceStrategyRejected {
        id: CommandId,
        error: SetResourceStrategyError,
    },
    RequesterAllowed {
        id: CommandId,
    },
    AllowRequesterRejected {
        id: CommandId,
        error: AllowRequesterError,
    },
    OwesLinkClose {
        id: CommandId,
        close: CloseLink,
    },
    CloseLinkRejected {
        id: CommandId,
        error: CloseLinkError,
    },
    RpcQueryRead {
        id: CommandId,
        result: RpcQueryResult,
    },
}

/// The terminal result of one issued command, paired verb-for-verb with
/// [`EngineCommand`] so every verb's success and failure stay typed across the
/// event lane — a data boundary erases type-level ties, so the tie is explicit
/// here.
#[derive(Debug, Clone, PartialEq, Eq)]
#[repr(C)]
pub enum Settlement {
    AnnounceNow(Result<(), AnnounceNowFailure>),
    SendSingle(Result<Delivered, SendSingleFailure>),
    SendGroup(Result<(), SendGroupFailure>),
    RequestPath(Result<PathFound, RequestPathFailure>),
    EstablishLink(Result<LinkEstablished, EstablishLinkFailure>),
    SendLink(Result<Delivered, SendLinkFailure>),
    Identify(Result<(), IdentifyFailure>),
    SendRequest(Result<Delivered, SendRequestFailure>),
    Respond(Result<(), RespondFailure>),
    CloseLink(Result<(), CloseLinkFailure>),
    SendResource(Result<(), SendResourceFailure>),
    SetResourceStrategy(Result<(), SetResourceStrategyFailure>),
    SendChannel(Result<Delivered, SendChannelFailure>),
    AllowRequester(Result<(), AllowRequesterFailure>),
    RpcQuery(RpcQueryResult),
}

/// A curated read of one interface's live engine counts — the diagnostic figures a face shows per
/// card. The reactor recomputes these for a touched interface and pushes them into the runtime's
/// interface store; a face reads the store on its change signal. Kept small and `Copy`; grow the
/// fields here as a face needs more.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct InterfaceCounts {
    /// Routing-table destinations reachable via this interface.
    pub destinations: u32,
    /// Live Reticulum links this node terminates over this interface.
    pub links: u32,
    pub transported_links: u32,
}

/// RNS 1.3.1 `PacketReceipt.DELIVERED`, with the round trip it measured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Delivered {
    pub rtt: Rtt,
}

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
        interfaces: &[InterfaceConfig],
    ) -> CommandOutcome {
        self.ingested_command_count = self.ingested_command_count.saturating_add(1);
        let IssuedCommand { id, command } = issued;
        match command {
            EngineCommand::AnnounceNow(announce_now) => {
                self.ingest_announce_now(id, announce_now, interfaces)
            }
            EngineCommand::SendSingle(send) => self.ingest_send_single(id, send),
            EngineCommand::SendGroup(send) => self.ingest_send_group(id, send),
            EngineCommand::RequestPath(request) => CommandOutcome::OwesPathRequest { id, request },
            EngineCommand::EstablishLink(establish) => self.ingest_establish_link(id, establish),
            EngineCommand::SendLink(send) => self.ingest_send_link(id, send),
            EngineCommand::SendChannel(send) => self.ingest_send_channel(id, send),
            EngineCommand::Identify(identify) => self.ingest_identify(id, identify),
            EngineCommand::SendRequest(request) => self.ingest_send_request(id, request),
            EngineCommand::Respond(respond) => self.ingest_respond(id, respond),
            EngineCommand::CloseLink(close) => self.ingest_close_link(id, close),
            EngineCommand::SetResourceStrategy(set) => self.ingest_set_resource_strategy(id, set),
            EngineCommand::AllowRequester(allow) => self.ingest_allow_requester(id, allow),
            EngineCommand::RpcQuery(query) => CommandOutcome::RpcQueryRead {
                id,
                result: self.run_rpc_query(query),
            },
        }
    }

    /// Reactor seam: the per-interface census (destinations, links, transported links) the
    /// status registries publish.
    #[cfg(any(feature = "tokio-host", feature = "embassy-host"))]
    pub fn interface_counts(&self, interface: InterfaceId) -> InterfaceCounts {
        InterfaceCounts {
            destinations: self.route_count_via(interface) as u32,
            links: self.links_via(interface) as u32,
            transported_links: self.transported_links_via(interface) as u32,
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
