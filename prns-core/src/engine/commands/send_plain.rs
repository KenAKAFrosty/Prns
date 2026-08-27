use heapless::Vec as HeaplessVec;

use crate::routing::delivery::send_plain::SendPlainPacketWriteError;
use crate::wire::{DestinationHash, BROADCAST_MDU};

use super::{PrnsCommand, Settleable, Settlement};

pub const MAX_SEND_PLAIN_PACKET_PAYLOAD_LEN: usize = BROADCAST_MDU;

pub type SendPlainPacketPayload = HeaplessVec<u8, MAX_SEND_PLAIN_PACKET_PAYLOAD_LEN>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendPlainPacket {
    pub destination: DestinationHash,
    pub payload: SendPlainPacketPayload,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendPlainPacketFailure {
    WriteFailed(SendPlainPacketWriteError),
}

impl Settleable for SendPlainPacket {
    type Success = ();
    type Failure = SendPlainPacketFailure;

    fn into_command(self) -> PrnsCommand {
        PrnsCommand::SendPlainPacket(self)
    }

    fn from_settlement(settlement: Settlement) -> Option<Result<(), SendPlainPacketFailure>> {
        match settlement {
            Settlement::SendPlainPacket(result) => Some(result),

            Settlement::AnnounceNow(_)
            | Settlement::SendSinglePacket(_)
            | Settlement::SendGroup(_)
            | Settlement::RequestPath(_)
            | Settlement::EstablishLink(_)
            | Settlement::SendToLink(_)
            | Settlement::CloseLink(_)
            | Settlement::Identify(_)
            | Settlement::SendRequest(_)
            | Settlement::Respond(_)
            | Settlement::SendResource(_)
            | Settlement::SetResourceStrategy(_)
            | Settlement::SendToChannel(_)
            | Settlement::AllowRequester(_)
            | Settlement::SetRegisteredAnnounceAppData(_) => None,
        }
    }
}
