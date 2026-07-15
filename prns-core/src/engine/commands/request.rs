use heapless::Vec as HeaplessVec;

use crate::identity::IdentityHash;
use crate::routing::links::data::link_mdu;
use crate::routing::links::request::{RequestId, REQUEST_WIRE_OVERHEAD, RESPONSE_WIRE_OVERHEAD};
use crate::routing::links::LinkId;
use crate::routing::request_handlers::RequestPathHash;
use crate::wire::DestinationHash;

use super::{EngineCommand, PacketReceiptDelivered, Settleable, Settlement};

pub const MAX_SEND_REQUEST_DATA_LEN: usize =
    link_mdu(crate::wire::BROADCAST_MTU) - REQUEST_WIRE_OVERHEAD;

pub type SendRequestData = HeaplessVec<u8, MAX_SEND_REQUEST_DATA_LEN>;

/// RNS 1.3.5 `Link.request(path, data)`, sub-MDU form; empty `data` = the reference's None.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendRequest {
    pub link_id: LinkId,
    pub path_hash: RequestPathHash,
    pub data: SendRequestData,
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
}

pub const MAX_RESPOND_DATA_LEN: usize =
    link_mdu(crate::wire::BROADCAST_MTU) - RESPONSE_WIRE_OVERHEAD;

pub type RespondData = HeaplessVec<u8, MAX_RESPOND_DATA_LEN>;

/// Msgpack'd `[request_id, data]`. Fire-and-forget, like the reference's response packet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Respond {
    pub link_id: LinkId,
    pub request_id: RequestId,
    pub data: RespondData,
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
}

/// RNS 1.3.5 `Destination.register_request_handler(..., allowed_list=…)`, mutated at runtime.
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

    fn into_command(self) -> EngineCommand {
        EngineCommand::SendRequest(self)
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
            | Settlement::Inspection(_) => None,
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
            | Settlement::Inspection(_) => None,
        }
    }
}

impl Settleable for AllowRequester {
    type Success = ();
    type Failure = AllowRequesterFailure;

    fn into_command(self) -> EngineCommand {
        EngineCommand::AllowRequester(self)
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
            | Settlement::Inspection(_) => None,
        }
    }
}
