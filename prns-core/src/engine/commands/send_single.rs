use heapless::Vec as HeaplessVec;

use crate::routing::delivery::send_single::WriteSendSingleError;
use crate::wire::DestinationHash;

use super::{EngineCommand, PacketReceiptDelivered, Settleable, Settlement};

/// RNS 1.3.5 `Packet.ENCRYPTED_MDU` (383): the most plaintext one encrypted
/// Single data packet can carry — MDU minus the token overhead (32B ephemeral
/// key, 16B IV, 32B MAC), floored to a whole AES block, minus one pad byte.
pub const MAX_SEND_SINGLE_PLAINTEXT_LEN: usize = 383;

pub type SendSinglePayload = HeaplessVec<u8, MAX_SEND_SINGLE_PLAINTEXT_LEN>;

/// RNS 1.3.5 `Packet(destination, data).send()` with its `PacketReceipt`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendSingle {
    pub destination: DestinationHash,
    pub payload: SendSinglePayload,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendSingleError {
    NoRouteToDestination,
    NotDirectlyReachable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendSingleFailure {
    Rejected(SendSingleError),
    WriteFailed(WriteSendSingleError),
    Culled,
    Timeout,
}

impl Settleable for SendSingle {
    type Success = PacketReceiptDelivered;
    type Failure = SendSingleFailure;

    fn into_command(self) -> EngineCommand {
        EngineCommand::SendSingle(self)
    }

    fn from_settlement(
        settlement: Settlement,
    ) -> Option<Result<PacketReceiptDelivered, SendSingleFailure>> {
        match settlement {
            Settlement::SendSingle(result) => Some(result),

            //We do this explicitly so that future new members must be re-considered, even if the common case is for them to end up here
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
            | Settlement::SetResourceStrategy(_)
            | Settlement::SendToChannel(_)
            | Settlement::AllowRequester(_)
            | Settlement::RpcQuery(_) => None,
        }
    }
}
