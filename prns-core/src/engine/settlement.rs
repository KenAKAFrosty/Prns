use crate::engine::{
    CommandId, EngineReaction, EngineState, Journaled,
    RemoteControlControllerPairingRequestFailure,
    RemoteControlControllerPairingRequestFailureCause, SendRequestFailure, SendRequestIntent,
    SendSinglePacketFailure, SendToLinkFailure, Settlement,
};
use crate::routing::delivery::receipts::{LinkOwnedReceiptKind, ReceiptKind};
use crate::routing::links::LinkId;
use crate::storage::StorageLayout;

pub(crate) fn settle(
    sink: &mut impl FnMut(EngineReaction<'_>),
    id: CommandId,
    settlement: Settlement,
) {
    sink(EngineReaction::Journaled(Journaled::CommandSettled {
        id,
        settlement,
    }));
}

impl<S: StorageLayout> EngineState<S> {
    pub(crate) fn failed_send_request_settlement(
        &mut self,
        link_id: LinkId,
        intent: SendRequestIntent,
        failure: SendRequestFailure,
    ) -> Settlement {
        match intent {
            SendRequestIntent::Application => Settlement::SendRequest(Err(failure)),
            SendRequestIntent::RemoteControlControllerPairing => self
                .failed_remote_control_controller_pairing_request_settlement(
                    link_id,
                    RemoteControlControllerPairingRequestFailureCause::Request(failure),
                ),
        }
    }

    pub(crate) fn failed_remote_control_controller_pairing_request_settlement(
        &mut self,
        link_id: LinkId,
        cause: RemoteControlControllerPairingRequestFailureCause,
    ) -> Settlement {
        let exchange = self
            .remote_control_controller_pairing
            .request_failed(link_id);
        Settlement::RemoteControlControllerPairingRequest(Err(
            RemoteControlControllerPairingRequestFailure { cause, exchange },
        ))
    }

    pub(crate) fn link_closed_settlement(
        &mut self,
        link_id: LinkId,
        kind: LinkOwnedReceiptKind,
    ) -> Settlement {
        match kind {
            LinkOwnedReceiptKind::SendToLink => {
                Settlement::SendToLink(Err(SendToLinkFailure::LinkClosed))
            }
            LinkOwnedReceiptKind::SendRequest(intent) => {
                self.failed_send_request_settlement(link_id, intent, SendRequestFailure::LinkClosed)
            }
        }
    }

    pub(super) fn culled_settlement(&mut self, kind: ReceiptKind) -> Settlement {
        match kind {
            ReceiptKind::SendSinglePacket { .. } => {
                Settlement::SendSinglePacket(Err(SendSinglePacketFailure::Culled))
            }
            ReceiptKind::SendToLink(_) => Settlement::SendToLink(Err(SendToLinkFailure::Culled)),
            ReceiptKind::SendRequest { link_id, response } => self.failed_send_request_settlement(
                link_id,
                response.intent(),
                SendRequestFailure::Culled,
            ),
        }
    }

    pub(super) fn timeout_settlement(&mut self, kind: ReceiptKind) -> Settlement {
        match kind {
            ReceiptKind::SendSinglePacket { .. } => {
                Settlement::SendSinglePacket(Err(SendSinglePacketFailure::Timeout))
            }
            ReceiptKind::SendToLink(_) => Settlement::SendToLink(Err(SendToLinkFailure::Timeout)),
            ReceiptKind::SendRequest { link_id, response } => self.failed_send_request_settlement(
                link_id,
                response.intent(),
                SendRequestFailure::Timeout,
            ),
        }
    }
}
