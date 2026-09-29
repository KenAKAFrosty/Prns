use super::*;
use prns_simulation::SimulationSeed;

#[test]
fn every_runtime_pair_obeys_the_same_request_failure_and_recovery_contract() {
    let echo = |marker| {
        [
            Ok((vec![marker; 256], RttMillis::new(0))),
            Ok((vec![marker; 256], RttMillis::new(0))),
        ]
    };
    let expected = Report {
        initial: echo(41),
        expired: [
            TimedReply {
                reply: Err(SendError::Failed(SendRequestFailure::Timeout)),
                elapsed_ms: RESPONSE_TIMEOUT_MS,
            },
            TimedReply {
                reply: Err(SendError::Failed(SendRequestFailure::Timeout)),
                elapsed_ms: RESPONSE_TIMEOUT_MS,
            },
        ],
        timeout_elapsed_ms: RESPONSE_TIMEOUT_MS,
        after_expiry: echo(43),
        after_cancellation: echo(45),
        after_reconnect: echo(44),
    };
    for runtimes in RUNTIME_PAIRS {
        for scheduling in [
            ManualTaskScheduling::Cyclic,
            ManualTaskScheduling::Seeded {
                seed: SimulationSeed::new(7),
            },
        ] {
            let observed = run(runtimes, scheduling);
            assert!(
                observed.recovery_elapsed_ms <= DISCOVERY_BUDGET_MS,
                "{runtimes:?} {scheduling:?}: recovery took {} ms",
                observed.recovery_elapsed_ms
            );
            assert_eq!(observed.report, expected, "{runtimes:?} {scheduling:?}");
        }
    }
}
