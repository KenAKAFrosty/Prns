use heapless::Vec as HeaplessVec;

use crate::identity::IdentityHash;
use crate::routing::links::data::link_mdu;
use crate::routing::links::request::{RequestId, REQUEST_WIRE_OVERHEAD, RESPONSE_WIRE_OVERHEAD};
use crate::routing::links::LinkId;
use crate::routing::request_handlers::RequestPathHash;
use crate::units::{ByteLimit, DurationMillis};
use crate::wire::DestinationHash;

use super::{PacketReceiptDelivered, PrnsCommand, SendResourceFailure, Settleable, Settlement};

pub const MAX_SEND_REQUEST_DATA_LEN: usize =
    link_mdu(crate::wire::BROADCAST_MTU) - REQUEST_WIRE_OVERHEAD;

pub type SendRequestData = HeaplessVec<u8, MAX_SEND_REQUEST_DATA_LEN>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RequestResponseTimeout {
    #[default]
    LinkDefault,
    Exact(DurationMillis),
}

/// RNS 1.4.2 `Link.request(path, data)`, sub-MDU form; empty `data` = the reference's None.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendRequest {
    pub link_id: LinkId,
    pub path_hash: RequestPathHash,
    pub data: SendRequestData,
    pub response_timeout: RequestResponseTimeout,
    pub maximum_response_bytes: ByteLimit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendRequestRejection {
    NoSuchLink,
    LinkNotActive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendRequestFailure {
    Rejected(SendRequestRejection),
    WriteFailed,
    Culled,
    Timeout,
    ResponseTooLarge,
    /// A valid Resource response could not be admitted within the receiver's
    /// bounded memory and pending-offer limits.
    ResourceCapacity,
}

pub const MAX_RESPOND_DATA_LEN: usize =
    link_mdu(crate::wire::BROADCAST_MTU) - RESPONSE_WIRE_OVERHEAD;

pub type RespondData = HeaplessVec<u8, MAX_RESPOND_DATA_LEN>;

#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(clippy::large_enum_variant)]
pub enum RespondPayload {
    Packed(RespondData),
    StaticBytes(&'static [u8]),
    #[cfg(any(feature = "large-static-responses", test))]
    StaticFile {
        name: &'static str,
        bytes: &'static [u8],
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Respond {
    pub link_id: LinkId,
    pub request_id: RequestId,
    pub payload: RespondPayload,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RespondRejection {
    NoSuchLink,
    LinkNotActive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RespondFailure {
    Rejected(RespondRejection),
    WriteFailed,
    Resource(SendResourceFailure),
}

/// RNS 1.4.2 `Destination.register_request_handler(..., allowed_list=…)`, mutated at runtime.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AllowRequester {
    pub destination: DestinationHash,
    pub path_hash: RequestPathHash,
    pub identity: IdentityHash,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AllowRequesterRejection {
    NoSuchHandler,
    NoAllowList,
    AllowListFull,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AllowRequesterFailure {
    Rejected(AllowRequesterRejection),
}

impl Settleable for SendRequest {
    type Success = PacketReceiptDelivered;
    type Failure = SendRequestFailure;

    fn into_command(self) -> PrnsCommand {
        PrnsCommand::SendRequest(self)
    }

    fn from_settlement(
        settlement: Settlement,
    ) -> Option<Result<PacketReceiptDelivered, SendRequestFailure>> {
        match settlement {
            Settlement::SendRequest(result) => Some(result),

            Settlement::AnnounceNow(_)
            | Settlement::SendSinglePacket(_)
            | Settlement::SendGroup(_)
            | Settlement::RequestPath(_)
            | Settlement::EstablishLink(_)
            | Settlement::SendToLink(_)
            | Settlement::CloseLink(_)
            | Settlement::Identify(_)
            | Settlement::Respond(_)
            | Settlement::SendResource(_)
            | Settlement::SetResourceStrategy(_)
            | Settlement::SendToChannel(_)
            | Settlement::AllowRequester(_)
            | Settlement::SendPlainPacket(_) => None,
        }
    }
}

impl Settleable for Respond {
    type Success = ();
    type Failure = RespondFailure;

    fn into_command(self) -> PrnsCommand {
        PrnsCommand::Respond(self)
    }

    fn from_settlement(settlement: Settlement) -> Option<Result<(), RespondFailure>> {
        match settlement {
            Settlement::Respond(result) => Some(result),

            Settlement::AnnounceNow(_)
            | Settlement::SendSinglePacket(_)
            | Settlement::SendGroup(_)
            | Settlement::RequestPath(_)
            | Settlement::EstablishLink(_)
            | Settlement::SendToLink(_)
            | Settlement::CloseLink(_)
            | Settlement::Identify(_)
            | Settlement::SendRequest(_)
            | Settlement::SendResource(_)
            | Settlement::SetResourceStrategy(_)
            | Settlement::SendToChannel(_)
            | Settlement::AllowRequester(_)
            | Settlement::SendPlainPacket(_) => None,
        }
    }
}

impl Settleable for AllowRequester {
    type Success = ();
    type Failure = AllowRequesterFailure;

    fn into_command(self) -> PrnsCommand {
        PrnsCommand::AllowRequester(self)
    }

    fn from_settlement(settlement: Settlement) -> Option<Result<(), AllowRequesterFailure>> {
        match settlement {
            Settlement::AllowRequester(result) => Some(result),

            Settlement::AnnounceNow(_)
            | Settlement::SendSinglePacket(_)
            | Settlement::SendGroup(_)
            | Settlement::RequestPath(_)
            | Settlement::EstablishLink(_)
            | Settlement::SendToLink(_)
            | Settlement::Identify(_)
            | Settlement::SendRequest(_)
            | Settlement::Respond(_)
            | Settlement::CloseLink(_)
            | Settlement::SendResource(_)
            | Settlement::SetResourceStrategy(_)
            | Settlement::SendToChannel(_)
            | Settlement::SendPlainPacket(_) => None,
        }
    }
}
