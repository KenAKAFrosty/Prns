use heapless::Vec as HeaplessVec;

use crate::routing::delivery::send_single::WriteSendSingleError;
use crate::wire::DestinationHash;

use super::{Delivered, EngineCommand, Settleable, Settlement};

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
            | Settlement::SetResourceStrategy(_)
            | Settlement::SendChannel(_)
            | Settlement::AllowRequester(_)
            | Settlement::RpcQuery(_) => None,
        }
    }
}
