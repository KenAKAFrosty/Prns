use crate::engine::{
    EngineReaction, EngineState, InstantMillis, Journaled, RequestPathFailure, SendSingleFailure,
    Settlement, WakeSchedules,
};
use crate::routing::storage::EngineStorage;

impl<S: EngineStorage> EngineState<S> {
    /// Settle every send-single whose proof deadline has passed: each gives up and closes
    /// `SendSingle(Timeout)`. Returns the send-timeout lane's new soonest deadline.
    pub fn settle_timed_out_send_singles(
        &mut self,
        now: InstantMillis,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) -> WakeSchedules {
        while let Some(expired) = self.pop_timed_out_send_single(now) {
            sink(EngineReaction::Journaled(Journaled::CommandSettled {
                id: expired.command_id,
                settlement: Settlement::SendSingle(Err(SendSingleFailure::Timeout)),
            }));
        }
        WakeSchedules {
            send_single_timeout: self.send_timeout_lane(),
            ..WakeSchedules::UNCHANGED
        }
    }

    /// Settle every path request whose answer never arrived in time: each closes
    /// `RequestPath(Timeout)`. Returns the path-timeout lane's new soonest deadline.
    pub fn settle_timed_out_path_requests(
        &mut self,
        now: InstantMillis,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) -> WakeSchedules {
        while let Some(expired) = self.pop_timed_out_path_request(now) {
            sink(EngineReaction::Journaled(Journaled::CommandSettled {
                id: expired.command_id,
                settlement: Settlement::RequestPath(Err(RequestPathFailure::Timeout)),
            }));
        }
        WakeSchedules {
            path_request_timeout: self.path_timeout_lane(),
            ..WakeSchedules::UNCHANGED
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::test_support::Cap;
    use crate::engine::{
        CommandId, PathRequestId, PathRequestWriteOutcome, RequestPath, PATH_REQUEST_TIMEOUT_MS,
    };
    use crate::wire::{DestinationHash, MTU};

    #[test]
    fn settle_timed_out_path_requests_closes_each_expired_request_once_past_its_deadline() {
        let mut engine = EngineState::<Cap>::default();
        let issued_at = InstantMillis(1_000);
        let mut buf = [0u8; MTU];
        let outcome = engine.write_commanded_path_request(
            CommandId(9),
            &RequestPath {
                destination: DestinationHash::new([0x44; 16]),
                id: PathRequestId::new([0x55; 16]),
            },
            issued_at,
            &mut buf,
        );
        assert!(matches!(outcome, PathRequestWriteOutcome::Written { .. }));

        let mut settled: std::vec::Vec<(CommandId, Settlement)> = std::vec::Vec::new();

        engine.settle_timed_out_path_requests(issued_at, &mut |reaction| {
            if let EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) =
                reaction
            {
                settled.push((id, settlement));
            }
        });
        assert!(settled.is_empty(), "before the deadline, nothing settles");

        engine.settle_timed_out_path_requests(
            InstantMillis(issued_at.0 + PATH_REQUEST_TIMEOUT_MS + 1),
            &mut |reaction| {
                if let EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) =
                    reaction
                {
                    settled.push((id, settlement));
                }
            },
        );
        assert_eq!(
            settled,
            std::vec![(
                CommandId(9),
                Settlement::RequestPath(Err(RequestPathFailure::Timeout)),
            )],
            "past the deadline the request settles Timeout, exactly once",
        );
    }
}
