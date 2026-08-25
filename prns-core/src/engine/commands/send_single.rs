use heapless::Vec as HeaplessVec;

use crate::crypto::TOKEN_OVERHEAD;
use crate::identity::ENCRYPTION_EPHEMERAL_PUBLIC_KEY_LEN;
use crate::routing::delivery::send_single::SendSinglePacketWriteError;
use crate::wire::{DestinationHash, BROADCAST_MDU};

use super::{PacketReceiptDelivered, PrnsCommand, Settleable, Settlement};

/// RNS 1.4.2 `Packet.ENCRYPTED_MDU`: whole AES blocks, less one byte so PKCS7 always has room to pad.
pub const MAX_SEND_SINGLE_PACKET_PLAINTEXT_LEN: usize =
    ((BROADCAST_MDU - ENCRYPTION_EPHEMERAL_PUBLIC_KEY_LEN - TOKEN_OVERHEAD) / 16) * 16 - 1;

pub type SendSinglePacketPayload = HeaplessVec<u8, MAX_SEND_SINGLE_PACKET_PLAINTEXT_LEN>;

/// RNS 1.4.2 `Packet(destination, data).send()` with its `PacketReceipt`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendSinglePacket {
    pub destination: DestinationHash,
    pub payload: SendSinglePacketPayload,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendSinglePacketRejection {
    NoRouteToDestination,
    NotDirectlyReachable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendSinglePacketFailure {
    Rejected(SendSinglePacketRejection),
    WriteFailed(SendSinglePacketWriteError),
    Culled,
    Timeout,
}

impl Settleable for SendSinglePacket {
    type Success = PacketReceiptDelivered;
    type Failure = SendSinglePacketFailure;

    fn into_command(self) -> PrnsCommand {
        PrnsCommand::SendSinglePacket(self)
    }

    fn from_settlement(
        settlement: Settlement,
    ) -> Option<Result<PacketReceiptDelivered, SendSinglePacketFailure>> {
        match settlement {
            Settlement::SendSinglePacket(result) => Some(result),

            Settlement::AnnounceNow(_)
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
            | Settlement::SendPlainPacket(_) => None,
        }
    }
}
