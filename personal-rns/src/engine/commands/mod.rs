//! App-issued commands, ingested by the engine as plain data.
//! Commands cross thread, task, and FFI boundaries as owned values,
//! so any host can queue them and the engine cycle drains them deterministically.
//!
//! RNS 1.3.1 has no scheduled announces at all: `Destination.announce()` is
//! app-called, and periodic announcing lives in app land (LXMF runs its own
//! timers). So [`AnnounceNow`] is the reference primitive, and this engine's
//! re-announce schedule is the extension built ahead of it.

use crate::engine::egress::EgressSerializeError;
use crate::engine::WriteAnnounceError;
use crate::identity::IdentityHash;
use crate::interfaces::InterfaceId;
use crate::routing::announce::emit::AnnounceAppDataBytes;
use crate::routing::delivery::send_single::WriteSendSingleError;
use crate::routing::links::data::LinkDataError;
use crate::routing::links::establish::WriteEstablishLinkError;
use crate::routing::links::resources::build_outgoing::BuildOutgoingResourceError;
use crate::routing::links::resources::ResourceStrategy;
use crate::routing::links::LinkId;
use crate::routing::request_handlers::RequestPathHash;
use crate::wire::{DestinationHash, TRUNCATED_HASH_BYTE_LEN};
use heapless::Vec as HeaplessVec;

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
pub enum EngineCommand {
    AnnounceNow(AnnounceNow),
    SendSingle(SendSingle),
    SendGroup(SendGroup),
    RequestPath(RequestPath),
    EstablishLink(EstablishLink),
    SendLink(SendLink),
    Identify(Identify),
    SendRequest(SendRequest),
    Respond(Respond),
    CloseLink(CloseLink),
    SetResourceStrategy(SetResourceStrategy),
}

/// `Destination.announce(app_data=…, attached_interface=…)` as data
/// (RNS 1.3.1 Destination.py).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnnounceNow {
    pub destination: DestinationHash,
    pub target: AnnounceTarget,
    pub app_data: AnnounceAppData,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnounceTarget {
    AllInterfaces,
    Interface(InterfaceId),
}

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnnounceAppData {
    Registered,
    Data(AnnounceAppDataBytes),
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
    ResourceStrategySet {
        id: CommandId,
    },
    SetResourceStrategyRejected {
        id: CommandId,
        error: SetResourceStrategyError,
    },
    OwesLinkClose {
        id: CommandId,
        close: CloseLink,
    },
    CloseLinkRejected {
        id: CommandId,
        error: CloseLinkError,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendSingleError {
    NoRouteToDestination,
    NotDirectlyReachable,
}

/// RNS 1.3.1 `Link(destination)`: bring a session up with a peer whose
/// announce we hold. Settles established when the LRPROOF validates, or fails
/// on rejection, a write error, or the establishment timeout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EstablishLink {
    pub destination: DestinationHash,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EstablishLinkError {
    NoRouteToDestination,
    NotDirectlyReachable,
}

pub const MAX_SEND_LINK_PLAINTEXT_LEN: usize = 431;

pub type SendLinkPayload = HeaplessVec<u8, MAX_SEND_LINK_PLAINTEXT_LEN>;

/// RNS 1.3.1 `Link.identify`: reveal a held identity to the responder over the
/// encrypted link — initiator-only, shown to the peer and no one else, and
/// fire-and-forget (the reference neither proves nor acknowledges one).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Identify {
    pub link_id: LinkId,
    pub identity: IdentityHash,
}

/// The most raw msgpack data bytes one sub-MDU request can carry at the
/// broadcast MTU: the link MDU less the request pack's own overhead.
pub const MAX_SEND_REQUEST_DATA_LEN: usize = 403;

pub type SendRequestData = HeaplessVec<u8, MAX_SEND_REQUEST_DATA_LEN>;

/// RNS 1.3.1 `Link.request(path, data)`, sub-MDU form: ask the peer's
/// registered handler at `truncated_hash(path)`. `data` crosses as raw
/// msgpack value bytes (empty = the reference's None); the engine never
/// interprets it. Settles Delivered when the response names this request's
/// id back, or Timeout at `rtt × 6` plus the response grace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendRequest {
    pub link_id: LinkId,
    pub path_hash: RequestPathHash,
    pub data: SendRequestData,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendRequestError {
    NoSuchLink,
    LinkNotActive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendRequestFailure {
    Rejected(SendRequestError),
    WriteFailed,
    Culled,
    Timeout,
}

/// The most raw msgpack data bytes one sub-MDU response can carry at the
/// broadcast MTU: the link MDU less the response pack's own overhead.
pub const MAX_RESPOND_DATA_LEN: usize = 412;

pub type RespondData = HeaplessVec<u8, MAX_RESPOND_DATA_LEN>;

/// The app's answer to a journaled `RequestReceived`: msgpack
/// `[request_id, data]` sealed back over the link — fire-and-forget, like the
/// reference's response packet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Respond {
    pub link_id: LinkId,
    pub request_id: crate::routing::links::request::RequestId,
    pub data: RespondData,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RespondError {
    NoSuchLink,
    LinkNotActive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RespondFailure {
    Rejected(RespondError),
    WriteFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentifyError {
    NoSuchLink,
    LinkNotActive,
    NotInitiator,
    IdentityNotHeld,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentifyFailure {
    Rejected(IdentifyError),
    WriteFailed,
}

/// One data packet sealed under an ACTIVE link's session key, fired on the
/// interface the link rides — RNS 1.3.1 `Packet(link, data).send()` with its
/// `PacketReceipt`. Settles Delivered when the responder's proof validates,
/// or Timeout at the link's traffic deadline — never at emission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendLink {
    pub link_id: LinkId,
    pub payload: SendLinkPayload,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendLinkError {
    NoSuchLink,
    LinkNotActive,
}

/// RNS 1.3.1 `Link.teardown`: close an ACTIVE link deliberately, telling the
/// peer with the sealed LINKCLOSE and purging the session key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CloseLink {
    pub link_id: LinkId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseLinkError {
    NoSuchLink,
    LinkNotActive,
}

/// RNS 1.3.1 `Packet.ENCRYPTED_MDU` (383): the most plaintext one encrypted
/// Single data packet can carry — MDU minus the token overhead (32B ephemeral
/// key, 16B IV, 32B MAC), floored to a whole AES block, minus one pad byte.
pub const MAX_SEND_SINGLE_PLAINTEXT_LEN: usize = 383;

pub type SendSinglePayload = HeaplessVec<u8, MAX_SEND_SINGLE_PLAINTEXT_LEN>;

/// One Single data packet to a peer whose announce we hold, proof expected
/// back — RNS 1.3.1 `Packet(destination, data).send()` with its
/// `PacketReceipt`. Settles when the proof arrives, the timeout passes, or
/// the receipt is culled — never in its own cycle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendSingle {
    pub destination: DestinationHash,
    pub payload: SendSinglePayload,
}

/// A GROUP destination shares the encrypted MDU; it carries no ephemeral key,
/// so this is conservative — the wire affords more, but RNS chunks every
/// encrypted destination at one size.
pub const MAX_SEND_GROUP_PLAINTEXT_LEN: usize = MAX_SEND_SINGLE_PLAINTEXT_LEN;

pub type SendGroupPayload = HeaplessVec<u8, MAX_SEND_GROUP_PLAINTEXT_LEN>;

/// One GROUP data packet, sealed with the destination's shared symmetric key and
/// broadcast to direct neighbors — RNS 1.3.1 `Packet(group_destination, data)`.
/// A GROUP cannot prove, so the send is fire-and-forget: it settles the moment it
/// is sealed and emitted, never on a delivery confirmation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendGroup {
    pub destination: DestinationHash,
    pub payload: SendGroupPayload,
}

pub const PATH_REQUEST_ID_LEN: usize = TRUNCATED_HASH_BYTE_LEN;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PathRequestId([u8; PATH_REQUEST_ID_LEN]);

impl PathRequestId {
    pub const fn new(bytes: [u8; PATH_REQUEST_ID_LEN]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; PATH_REQUEST_ID_LEN] {
        &self.0
    }
}

/// RNS 1.3.1 `Transport.request_path`: ask the network for a path to
/// `destination`. A broadcast plain packet, answered by any reachable peer that
/// holds the path (re-)announcing it. Settles found when a route arrives, or
/// times out — the structured form of the reference's `await_path` poll.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestPath {
    pub destination: DestinationHash,
    pub id: PathRequestId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnounceNowError {
    UnknownDestination,
    NotASingleDestination,
    AppDataTooLong,
    UnknownInterface,
}

/// The terminal result of one issued command, paired verb-for-verb with
/// [`EngineCommand`] so every verb's success and failure stay typed across the
/// event lane — a data boundary erases type-level ties, so the tie is explicit
/// here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PathFound {
    pub hops: u8,
}

/// RNS 1.3.1 `PacketReceipt.DELIVERED`, with the round trip it measured.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Delivered {
    pub rtt_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendSingleFailure {
    Rejected(SendSingleError),
    WriteFailed(WriteSendSingleError),
    Culled,
    Timeout,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnnounceNowFailure {
    Rejected(AnnounceNowError),
    WriteFailed(WriteAnnounceError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendGroupFailure {
    NoGroupKey,
    WriteFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestPathFailure {
    WriteFailed(EgressSerializeError),
    Timeout,
    Culled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkEstablished {
    pub link_id: LinkId,
    pub rtt_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EstablishLinkFailure {
    Rejected(EstablishLinkError),
    WriteFailed(WriteEstablishLinkError),
    Timeout,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendLinkFailure {
    Rejected(SendLinkError),
    WriteFailed(LinkDataError),
    Culled,
    Timeout,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseLinkFailure {
    Rejected(CloseLinkError),
    WriteFailed,
}

/// RNS 1.3.1 `Link.set_resource_strategy` as a command: how an active link
/// answers inbound resource advertisements from now on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SetResourceStrategy {
    pub link_id: LinkId,
    pub strategy: ResourceStrategy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetResourceStrategyError {
    NoSuchLink,
    LinkNotActive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetResourceStrategyFailure {
    Rejected(SetResourceStrategyError),
}

/// Why a `send_resource` never started: the link, the register, or the build
/// itself refused. Unlike the queueable commands this settles straight from
/// the borrow-taking entry point — the payload never rides a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendResourceError {
    NoSuchLink,
    LinkNotActive,
    LinkBusy,
    TableFull,
    Build(BuildOutgoingResourceError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendResourceFailure {
    Rejected(SendResourceError),
    WriteFailed,
    /// The receiver's hashmap-exhausted request named a position that closes
    /// no segment (the reference's "sequencing error"), which cancels the transfer.
    Sequencing,
}

pub trait Settleable {
    type Success;
    type Failure;

    fn into_command(self) -> EngineCommand;
    fn from_settlement(settlement: Settlement) -> Option<Result<Self::Success, Self::Failure>>;
}

impl Settleable for AnnounceNow {
    type Success = ();
    type Failure = AnnounceNowFailure;

    fn into_command(self) -> EngineCommand {
        EngineCommand::AnnounceNow(self)
    }

    fn from_settlement(settlement: Settlement) -> Option<Result<(), AnnounceNowFailure>> {
        match settlement {
            Settlement::AnnounceNow(result) => Some(result),
            Settlement::SendSingle(_)
            | Settlement::SendGroup(_)
            | Settlement::RequestPath(_)
            | Settlement::EstablishLink(_)
            | Settlement::SendLink(_)
            | Settlement::CloseLink(_)
            | Settlement::Identify(_)
            | Settlement::SendRequest(_)
            | Settlement::Respond(_)
            | Settlement::SendResource(_)
            | Settlement::SetResourceStrategy(_) => None,
        }
    }
}

impl Settleable for SendGroup {
    type Success = ();
    type Failure = SendGroupFailure;

    fn into_command(self) -> EngineCommand {
        EngineCommand::SendGroup(self)
    }

    fn from_settlement(settlement: Settlement) -> Option<Result<(), SendGroupFailure>> {
        match settlement {
            Settlement::SendGroup(result) => Some(result),
            Settlement::AnnounceNow(_)
            | Settlement::SendSingle(_)
            | Settlement::RequestPath(_)
            | Settlement::EstablishLink(_)
            | Settlement::SendLink(_)
            | Settlement::CloseLink(_)
            | Settlement::Identify(_)
            | Settlement::SendRequest(_)
            | Settlement::Respond(_)
            | Settlement::SendResource(_)
            | Settlement::SetResourceStrategy(_) => None,
        }
    }
}

impl Settleable for SendSingle {
    type Success = Delivered;
    type Failure = SendSingleFailure;

    fn into_command(self) -> EngineCommand {
        EngineCommand::SendSingle(self)
    }

    fn from_settlement(settlement: Settlement) -> Option<Result<Delivered, SendSingleFailure>> {
        match settlement {
            Settlement::SendSingle(result) => Some(result),
            Settlement::AnnounceNow(_)
            | Settlement::SendGroup(_)
            | Settlement::RequestPath(_)
            | Settlement::EstablishLink(_)
            | Settlement::SendLink(_)
            | Settlement::CloseLink(_)
            | Settlement::Identify(_)
            | Settlement::SendRequest(_)
            | Settlement::Respond(_)
            | Settlement::SendResource(_)
            | Settlement::SetResourceStrategy(_) => None,
        }
    }
}

impl Settleable for RequestPath {
    type Success = PathFound;
    type Failure = RequestPathFailure;

    fn into_command(self) -> EngineCommand {
        EngineCommand::RequestPath(self)
    }

    fn from_settlement(settlement: Settlement) -> Option<Result<PathFound, RequestPathFailure>> {
        match settlement {
            Settlement::RequestPath(result) => Some(result),
            Settlement::AnnounceNow(_)
            | Settlement::SendSingle(_)
            | Settlement::SendGroup(_)
            | Settlement::EstablishLink(_)
            | Settlement::SendLink(_)
            | Settlement::CloseLink(_)
            | Settlement::Identify(_)
            | Settlement::SendRequest(_)
            | Settlement::Respond(_)
            | Settlement::SendResource(_)
            | Settlement::SetResourceStrategy(_) => None,
        }
    }
}

impl Settleable for EstablishLink {
    type Success = LinkEstablished;
    type Failure = EstablishLinkFailure;

    fn into_command(self) -> EngineCommand {
        EngineCommand::EstablishLink(self)
    }

    fn from_settlement(
        settlement: Settlement,
    ) -> Option<Result<LinkEstablished, EstablishLinkFailure>> {
        match settlement {
            Settlement::EstablishLink(result) => Some(result),
            Settlement::AnnounceNow(_)
            | Settlement::SendSingle(_)
            | Settlement::SendGroup(_)
            | Settlement::RequestPath(_)
            | Settlement::SendLink(_)
            | Settlement::CloseLink(_)
            | Settlement::Identify(_)
            | Settlement::SendRequest(_)
            | Settlement::Respond(_)
            | Settlement::SendResource(_)
            | Settlement::SetResourceStrategy(_) => None,
        }
    }
}

impl Settleable for SendLink {
    type Success = Delivered;
    type Failure = SendLinkFailure;

    fn into_command(self) -> EngineCommand {
        EngineCommand::SendLink(self)
    }

    fn from_settlement(settlement: Settlement) -> Option<Result<Delivered, SendLinkFailure>> {
        match settlement {
            Settlement::SendLink(result) => Some(result),
            Settlement::AnnounceNow(_)
            | Settlement::SendSingle(_)
            | Settlement::SendGroup(_)
            | Settlement::RequestPath(_)
            | Settlement::EstablishLink(_)
            | Settlement::CloseLink(_)
            | Settlement::Identify(_)
            | Settlement::SendRequest(_)
            | Settlement::Respond(_)
            | Settlement::SendResource(_)
            | Settlement::SetResourceStrategy(_) => None,
        }
    }
}

impl Settleable for Identify {
    type Success = ();
    type Failure = IdentifyFailure;

    fn into_command(self) -> EngineCommand {
        EngineCommand::Identify(self)
    }

    fn from_settlement(settlement: Settlement) -> Option<Result<(), IdentifyFailure>> {
        match settlement {
            Settlement::Identify(result) => Some(result),
            Settlement::SendRequest(_)
            | Settlement::Respond(_)
            | Settlement::AnnounceNow(_)
            | Settlement::SendSingle(_)
            | Settlement::SendGroup(_)
            | Settlement::RequestPath(_)
            | Settlement::EstablishLink(_)
            | Settlement::SendLink(_)
            | Settlement::CloseLink(_)
            | Settlement::SendResource(_)
            | Settlement::SetResourceStrategy(_) => None,
        }
    }
}

impl Settleable for SendRequest {
    type Success = Delivered;
    type Failure = SendRequestFailure;

    fn into_command(self) -> EngineCommand {
        EngineCommand::SendRequest(self)
    }

    fn from_settlement(settlement: Settlement) -> Option<Result<Delivered, SendRequestFailure>> {
        match settlement {
            Settlement::SendRequest(result) => Some(result),
            _ => None,
        }
    }
}

impl Settleable for Respond {
    type Success = ();
    type Failure = RespondFailure;

    fn into_command(self) -> EngineCommand {
        EngineCommand::Respond(self)
    }

    fn from_settlement(settlement: Settlement) -> Option<Result<(), RespondFailure>> {
        match settlement {
            Settlement::Respond(result) => Some(result),
            _ => None,
        }
    }
}

impl Settleable for CloseLink {
    type Success = ();
    type Failure = CloseLinkFailure;

    fn into_command(self) -> EngineCommand {
        EngineCommand::CloseLink(self)
    }

    fn from_settlement(settlement: Settlement) -> Option<Result<(), CloseLinkFailure>> {
        match settlement {
            Settlement::CloseLink(result) => Some(result),
            Settlement::AnnounceNow(_)
            | Settlement::SendSingle(_)
            | Settlement::SendGroup(_)
            | Settlement::RequestPath(_)
            | Settlement::EstablishLink(_)
            | Settlement::SendLink(_)
            | Settlement::Identify(_)
            | Settlement::SendRequest(_)
            | Settlement::Respond(_)
            | Settlement::SendResource(_)
            | Settlement::SetResourceStrategy(_) => None,
        }
    }
}

impl Settleable for SetResourceStrategy {
    type Success = ();
    type Failure = SetResourceStrategyFailure;

    fn into_command(self) -> EngineCommand {
        EngineCommand::SetResourceStrategy(self)
    }

    fn from_settlement(
        settlement: Settlement,
    ) -> Option<Result<(), SetResourceStrategyFailure>> {
        match settlement {
            Settlement::SetResourceStrategy(result) => Some(result),
            Settlement::AnnounceNow(_)
            | Settlement::SendSingle(_)
            | Settlement::SendGroup(_)
            | Settlement::RequestPath(_)
            | Settlement::EstablishLink(_)
            | Settlement::SendLink(_)
            | Settlement::Identify(_)
            | Settlement::SendRequest(_)
            | Settlement::Respond(_)
            | Settlement::CloseLink(_)
            | Settlement::SendResource(_) => None,
        }
    }
}

use crate::engine::EngineState;
use crate::interfaces::InterfaceConfig;
use crate::routing::announce::emit::MAX_RATCHETED_ANNOUNCE_APP_DATA_LEN;
use crate::routing::storage::EngineStorage;
use crate::wire::DestinationType;

impl<S: EngineStorage> EngineState<S> {
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
            EngineCommand::Identify(identify) => self.ingest_identify(id, identify),
            EngineCommand::SendRequest(request) => self.ingest_send_request(id, request),
            EngineCommand::Respond(respond) => self.ingest_respond(id, respond),
            EngineCommand::CloseLink(close) => self.ingest_close_link(id, close),
            EngineCommand::SetResourceStrategy(set) => {
                self.ingest_set_resource_strategy(id, set)
            }
        }
    }

    fn ingest_announce_now(
        &self,
        id: CommandId,
        announce_now: AnnounceNow,
        interfaces: &[InterfaceConfig],
    ) -> CommandOutcome {
        if self
            .upstream_app_destinations
            .lookup(&announce_now.destination, DestinationType::Single)
            .is_none()
        {
            return CommandOutcome::AnnounceRejected {
                id,
                error: if self
                    .upstream_app_destinations
                    .lookup(&announce_now.destination, DestinationType::Plain)
                    .is_some()
                {
                    AnnounceNowError::NotASingleDestination
                } else {
                    AnnounceNowError::UnknownDestination
                },
            };
        }
        if let AnnounceTarget::Interface(interface) = announce_now.target {
            if !interfaces
                .iter()
                .any(|descriptor| descriptor.id == interface)
            {
                return CommandOutcome::AnnounceRejected {
                    id,
                    error: AnnounceNowError::UnknownInterface,
                };
            }
        }
        if let AnnounceAppData::Data(data) = &announce_now.app_data {
            if self.self_ratchets.is_tracked(&announce_now.destination)
                && data.len() > MAX_RATCHETED_ANNOUNCE_APP_DATA_LEN
            {
                return CommandOutcome::AnnounceRejected {
                    id,
                    error: AnnounceNowError::AppDataTooLong,
                };
            }
        }
        CommandOutcome::OwesAnnounce {
            id,
            announce: announce_now,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::*;
    use crate::engine::RatchetPolicy;
    use crate::interfaces::InterfaceId;
    use crate::wire::DestinationHash;

    const TEST_COMMAND_ID: CommandId = CommandId(7);

    fn announce_now(destination: DestinationHash) -> IssuedCommand {
        IssuedCommand {
            id: TEST_COMMAND_ID,
            command: EngineCommand::AnnounceNow(AnnounceNow {
                destination,
                target: AnnounceTarget::AllInterfaces,
                app_data: AnnounceAppData::Registered,
            }),
        }
    }

    #[test]
    fn an_announce_now_for_a_registered_single_owes_the_announce() {
        let mut state = personal_node_announcer();
        let destination = personal_node_destination();

        assert_eq!(
            state.ingest_command(announce_now(destination), &[]),
            CommandOutcome::OwesAnnounce {
                id: TEST_COMMAND_ID,
                announce: AnnounceNow {
                    destination,
                    target: AnnounceTarget::AllInterfaces,
                    app_data: AnnounceAppData::Registered,
                },
            },
        );
        assert_eq!(state.ingested_command_count(), 1);
    }

    #[test]
    fn an_announce_now_for_an_unknown_destination_is_rejected() {
        let mut state = personal_node_announcer();

        assert_eq!(
            state.ingest_command(announce_now(DestinationHash::new([0x77; 16])), &[]),
            CommandOutcome::AnnounceRejected {
                id: TEST_COMMAND_ID,
                error: AnnounceNowError::UnknownDestination,
            },
        );
        assert_eq!(state.ingested_command_count(), 1);
    }

    #[test]
    fn an_announce_now_for_a_plain_destination_is_rejected() {
        let mut state = personal_node_announcer();
        let plain = state
            .register_plain_destination("personal", &["plain"])
            .unwrap();

        assert_eq!(
            state.ingest_command(announce_now(plain), &[]),
            CommandOutcome::AnnounceRejected {
                id: TEST_COMMAND_ID,
                error: AnnounceNowError::NotASingleDestination,
            },
        );
    }

    #[test]
    fn an_announce_now_targets_only_interfaces_the_view_offers() {
        let mut state = personal_node_announcer();
        let destination = personal_node_destination();
        let view = [routable_descriptor(InterfaceId::new([0xAA; 16]))];
        let on = |interface| IssuedCommand {
            id: TEST_COMMAND_ID,
            command: EngineCommand::AnnounceNow(AnnounceNow {
                destination,
                target: AnnounceTarget::Interface(interface),
                app_data: AnnounceAppData::Registered,
            }),
        };

        assert_eq!(
            state.ingest_command(on(InterfaceId::new([0xAA; 16])), &view),
            CommandOutcome::OwesAnnounce {
                id: TEST_COMMAND_ID,
                announce: AnnounceNow {
                    destination,
                    target: AnnounceTarget::Interface(InterfaceId::new([0xAA; 16])),
                    app_data: AnnounceAppData::Registered,
                },
            },
        );
        assert_eq!(
            state.ingest_command(on(InterfaceId::new([0xBB; 16])), &view),
            CommandOutcome::AnnounceRejected {
                id: TEST_COMMAND_ID,
                error: AnnounceNowError::UnknownInterface,
            },
        );
    }

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

    #[test]
    fn announce_now_recovers_its_typed_settlement() {
        let verb = AnnounceNow {
            destination: DestinationHash::new([0x11; 16]),
            target: AnnounceTarget::AllInterfaces,
            app_data: AnnounceAppData::Registered,
        };

        assert_eq!(
            verb.clone().into_command(),
            EngineCommand::AnnounceNow(verb),
        );
        assert_eq!(
            AnnounceNow::from_settlement(Settlement::AnnounceNow(Ok(()))),
            Some(Ok(())),
        );
        assert_eq!(
            AnnounceNow::from_settlement(Settlement::AnnounceNow(Err(
                AnnounceNowFailure::Rejected(AnnounceNowError::UnknownDestination)
            ))),
            Some(Err(AnnounceNowFailure::Rejected(
                AnnounceNowError::UnknownDestination
            ))),
        );
    }

    #[test]
    fn a_request_path_owes_its_emission_for_any_destination() {
        // No registration, no route — a path request to a wholly unknown
        // destination still owes its emission. That is the point of asking.
        let mut state = personal_node_announcer();
        let request = RequestPath {
            destination: DestinationHash::new([0x44; 16]),
            id: PathRequestId::new([0x55; 16]),
        };

        assert_eq!(
            state.ingest_command(
                IssuedCommand {
                    id: TEST_COMMAND_ID,
                    command: EngineCommand::RequestPath(request),
                },
                &[],
            ),
            CommandOutcome::OwesPathRequest {
                id: TEST_COMMAND_ID,
                request,
            },
        );
        assert_eq!(state.ingested_command_count(), 1);
    }

    #[test]
    fn request_path_recovers_its_typed_settlement() {
        let verb = RequestPath {
            destination: DestinationHash::new([0x11; 16]),
            id: PathRequestId::new([0x22; 16]),
        };

        assert_eq!(verb.into_command(), EngineCommand::RequestPath(verb));
        assert_eq!(
            RequestPath::from_settlement(Settlement::RequestPath(Ok(PathFound { hops: 2 }))),
            Some(Ok(PathFound { hops: 2 })),
        );
        assert_eq!(
            RequestPath::from_settlement(Settlement::AnnounceNow(Ok(()))),
            None,
            "a path request never reads another verb's settlement",
        );
    }

    #[test]
    fn establish_link_recovers_its_typed_settlement() {
        let verb = EstablishLink {
            destination: DestinationHash::new([0x11; 16]),
        };

        assert_eq!(verb.into_command(), EngineCommand::EstablishLink(verb));
        assert_eq!(
            EstablishLink::from_settlement(Settlement::EstablishLink(Ok(LinkEstablished {
                link_id: LinkId::new([0x22; 16]),
                rtt_ms: 250,
            }))),
            Some(Ok(LinkEstablished {
                link_id: LinkId::new([0x22; 16]),
                rtt_ms: 250,
            })),
        );
        assert_eq!(
            EstablishLink::from_settlement(Settlement::SendGroup(Ok(()))),
            None,
            "an establishment never reads another verb's settlement",
        );
    }

    #[test]
    fn commanded_app_data_reserves_announce_room_for_the_ratchet() {
        let oversized =
            AnnounceAppDataBytes::from_slice(&[0u8; MAX_RATCHETED_ANNOUNCE_APP_DATA_LEN + 1])
                .unwrap();
        let with_data = |destination| IssuedCommand {
            id: TEST_COMMAND_ID,
            command: EngineCommand::AnnounceNow(AnnounceNow {
                destination,
                target: AnnounceTarget::AllInterfaces,
                app_data: AnnounceAppData::Data(oversized.clone()),
            }),
        };

        let mut ratcheted = personal_node_announcer_with(RatchetPolicy::Ratcheted);
        let destination = personal_node_destination();
        assert_eq!(
            ratcheted.ingest_command(with_data(destination), &[]),
            CommandOutcome::AnnounceRejected {
                id: TEST_COMMAND_ID,
                error: AnnounceNowError::AppDataTooLong,
            },
        );

        let mut unratcheted = personal_node_announcer();
        let destination = personal_node_destination();
        assert_eq!(
            unratcheted.ingest_command(with_data(destination), &[]),
            CommandOutcome::OwesAnnounce {
                id: TEST_COMMAND_ID,
                announce: AnnounceNow {
                    destination,
                    target: AnnounceTarget::AllInterfaces,
                    app_data: AnnounceAppData::Data(oversized),
                },
            },
        );
    }
}
