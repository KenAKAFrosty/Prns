mod adapter;
mod campaign;
mod pair;
mod tests;

use adapter::Runtime;
use pair::with_pair;
use personal_rns::engine::SendRequestFailure;
use personal_rns::runtime::SendError;
use personal_rns::units::RttMillis;
use prns_simulation::ManualTaskScheduling;

const RESPONSE_TIMEOUT_MS: u64 = 50;
const DISCOVERY_BUDGET_MS: u64 = crate::fixture::ADVERTISING_INTERVAL_MS * 3;
const RUNTIME_PAIRS: [[Runtime; 2]; 4] = [
    [Runtime::Tokio, Runtime::Tokio],
    [Runtime::Embassy, Runtime::Embassy],
    [Runtime::Embassy, Runtime::Tokio],
    [Runtime::Tokio, Runtime::Embassy],
];
type Reply = Result<(Vec<u8>, RttMillis), SendError<SendRequestFailure>>;

#[derive(Debug, PartialEq, Eq)]
struct Report {
    initial: [Reply; 2],
    expired: [TimedReply; 2],
    timeout_elapsed_ms: u64,
    after_expiry: [Reply; 2],
    after_cancellation: [Reply; 2],
    after_reconnect: [Reply; 2],
}

struct Observation {
    report: Report,
    recovery_elapsed_ms: u64,
}

#[derive(Debug, PartialEq, Eq)]
struct TimedReply {
    reply: Reply,
    elapsed_ms: u64,
}

fn run(runtimes: [Runtime; 2], scheduling: ManualTaskScheduling) -> Observation {
    with_pair(runtimes, scheduling, |pair| {
        let initial = pair.exchange(vec![41; 256]);
        let before = pair.now();
        let expired = pair.expire(RESPONSE_TIMEOUT_MS);
        let timeout_elapsed_ms = pair.now() - before;
        let after_expiry = pair.exchange(vec![43; 256]);
        pair.cancel();
        let after_cancellation = pair.exchange(vec![45; 256]);
        let recovery_elapsed_ms = pair.reconnect();
        let after_reconnect = pair.exchange(vec![44; 256]);
        Observation {
            report: Report {
                initial,
                expired,
                timeout_elapsed_ms,
                after_expiry,
                after_cancellation,
                after_reconnect,
            },
            recovery_elapsed_ms,
        }
    })
}
