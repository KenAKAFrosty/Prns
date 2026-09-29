mod cases;
mod tests;

use super::pair::Pair;
use super::*;
use cases::{Action, Case};
use prns_simulation::SimulationSeed;

#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    Expired {
        replies: [TimedReply; 2],
        elapsed_ms: u64,
    },
    Canceled {
        elapsed_ms: u64,
    },
    Reconnected,
}

#[derive(Debug, PartialEq, Eq)]
struct Phase {
    outcome: Outcome,
    echo: [Reply; 2],
}

fn execute(pair: &mut Pair<'_, '_>, case: &Case) {
    let initial = case.profile.payload(0);
    assert_eq!(
        pair.exchange(initial.clone()),
        [
            Ok((initial.clone(), RttMillis::new(0))),
            Ok((initial, RttMillis::new(0)))
        ]
    );
    for (index, action) in case.actions.iter().enumerate() {
        eprintln!("phase {index}: {action:?}");
        let before = pair.now();
        let timeout_ms = case.profile.timeout_ms();
        let (outcome, expected) = match action {
            Action::Expire => (
                Outcome::Expired {
                    replies: pair.expire(timeout_ms),
                    elapsed_ms: pair.now() - before,
                },
                Outcome::Expired {
                    replies: std::array::from_fn(|_| TimedReply {
                        reply: Err(SendError::Failed(SendRequestFailure::Timeout)),
                        elapsed_ms: timeout_ms,
                    }),
                    elapsed_ms: timeout_ms,
                },
            ),
            Action::Cancel => {
                pair.cancel();
                (
                    Outcome::Canceled {
                        elapsed_ms: pair.now() - before,
                    },
                    Outcome::Canceled { elapsed_ms: 0 },
                )
            }
            Action::Reconnect => {
                let elapsed_ms = pair.reconnect();
                assert!(elapsed_ms <= DISCOVERY_BUDGET_MS);
                (Outcome::Reconnected, Outcome::Reconnected)
            }
        };
        let payload = case.profile.payload(index + 1);
        let observed = Phase {
            outcome,
            echo: pair.exchange(payload.clone()),
        };
        let expected = Phase {
            outcome: expected,
            echo: [
                Ok((payload.clone(), RttMillis::new(0))),
                Ok((payload, RttMillis::new(0))),
            ],
        };
        assert_eq!(observed, expected, "phase {index}: {case:?}");
    }
}

#[test]
fn generated_failure_orders_obey_shared_runtime_contracts() {
    for case in cases::cases() {
        for runtimes in RUNTIME_PAIRS {
            for scheduling in [
                ManualTaskScheduling::Cyclic,
                ManualTaskScheduling::Seeded {
                    seed: SimulationSeed::new(7),
                },
            ] {
                eprintln!("case: {case:?}; runtimes: {runtimes:?}; schedule: {scheduling:?}");
                with_pair(runtimes, scheduling, |pair| execute(pair, &case));
            }
        }
    }
}
