use crate::engine::{
    CommandId, EngineReaction, Journaled, SendRequestFailure, SendSinglePacketFailure,
    SendToLinkFailure, Settlement,
};
use crate::routing::delivery::receipts::ReceiptKind;

pub(super) fn settle(
    sink: &mut impl FnMut(EngineReaction<'_>),
    id: CommandId,
    settlement: Settlement,
) {
    sink(EngineReaction::Journaled(Journaled::CommandSettled {
        id,
        settlement,
    }));
}

pub(super) fn culled_settlement(kind: ReceiptKind) -> Settlement {
    match kind {
        ReceiptKind::SendSinglePacket => {
            Settlement::SendSinglePacket(Err(SendSinglePacketFailure::Culled))
        }
        ReceiptKind::SendToLink(_) => Settlement::SendToLink(Err(SendToLinkFailure::Culled)),
        ReceiptKind::SendRequest => Settlement::SendRequest(Err(SendRequestFailure::Culled)),
    }
}

pub(super) fn timeout_settlement(kind: ReceiptKind) -> Settlement {
    match kind {
        ReceiptKind::SendSinglePacket => {
            Settlement::SendSinglePacket(Err(SendSinglePacketFailure::Timeout))
        }
        ReceiptKind::SendToLink(_) => Settlement::SendToLink(Err(SendToLinkFailure::Timeout)),
        ReceiptKind::SendRequest => Settlement::SendRequest(Err(SendRequestFailure::Timeout)),
    }
}
