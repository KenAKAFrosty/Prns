use crate::engine::{
    CommandId, EngineReaction, EngineState, RespondFailure, SendRequestFailure,
    SendResourceFailure, Settlement,
};
use crate::routing::links::request::RequestId;
use crate::routing::links::LinkId;
use crate::storage::StorageLayout;

use super::ResourceCorrelation;

pub(crate) fn resource_failure_settlement(
    correlation: ResourceCorrelation,
    failure: SendResourceFailure,
) -> Settlement {
    match correlation {
        ResourceCorrelation::Request { .. } => {
            Settlement::SendRequest(Err(SendRequestFailure::RequestTransferFailed(failure)))
        }
        ResourceCorrelation::Response(_) => {
            Settlement::Respond(Err(RespondFailure::Resource(failure)))
        }
        ResourceCorrelation::Unsolicited => Settlement::SendResource(Err(failure)),
    }
}

impl<S: StorageLayout> EngineState<S> {
    /// Match Python RequestReceipt.request_resource_concluded: the default
    /// response wait begins after a successful upload, not its advertisement.
    /// Explicit caller deadlines retain their original send-time ceiling.
    pub(crate) fn complete_request_resource_upload(
        &mut self,
        command: CommandId,
        link: &LinkId,
        correlation: ResourceCorrelation,
        now: crate::engine::InstantMillis,
    ) {
        let ResourceCorrelation::Request {
            id,
            response_timeout: crate::engine::RequestResponseTimeout::LinkDefault,
            ..
        } = correlation
        else {
            return;
        };
        let Some(crate::routing::links::table::LinkPhase::Active { rtt, .. }) =
            self.links.phase_for(link)
        else {
            return;
        };
        let deadline =
            now.0
                .saturating_add(crate::routing::links::request::request_response_timeout_ms(
                    *rtt,
                ));
        self.receipts.transition_request_deadline_for_command(
            link,
            command,
            id,
            crate::routing::delivery::receipts::ReceiptDeadline::AwaitingRequestUpload,
            crate::routing::delivery::receipts::ReceiptDeadline::Due(crate::engine::InstantMillis(
                deadline,
            )),
        );
    }

    /// An authenticated response demonstrates upload completion even if its proof
    /// was lost. The response transfer now owns the request's failure deadline.
    pub(crate) fn claim_resource_response(&mut self, link: &LinkId, request: RequestId) {
        self.receipts.claim_request_for_transfer(link, request);
        let Some(command) = self.receipts.pending_request_command(link, request) else {
            return;
        };
        for index in (0..self.outgoing_resources.len()).rev() {
            let state = self.outgoing_resources.state(index);
            if self.outgoing_resources.link_at(index) == link
                && state.command_id == command
                && matches!(state.correlation, ResourceCorrelation::Request { id, .. } if id == request)
            {
                self.outgoing_resources.remove_at(index);
            }
        }
    }

    /// Once advertised, a request's receipt owns its terminal result. Upload proof
    /// is not response success, and a late upload failure cannot settle it twice.
    pub(crate) fn settle_advertised_resource<Work>(
        &mut self,
        command: CommandId,
        link: &LinkId,
        correlation: ResourceCorrelation,
        result: Result<(), SendResourceFailure>,
        sink: &mut impl FnMut(EngineReaction<'_, Work>),
    ) {
        let settlement = match (correlation, result) {
            (ResourceCorrelation::Request { .. }, Ok(())) => return,
            (ResourceCorrelation::Request { id, .. }, Err(failure)) => {
                if self
                    .receipts
                    .take_request_for_command(link, command, id)
                    .is_none()
                {
                    return;
                }
                resource_failure_settlement(correlation, failure)
            }
            (ResourceCorrelation::Response(_), result) => {
                Settlement::Respond(result.map_err(RespondFailure::Resource))
            }
            (ResourceCorrelation::Unsolicited, result) => Settlement::SendResource(result),
        };
        crate::engine::settle(sink, command, settlement);
    }
}

#[cfg(test)]
mod tests;
