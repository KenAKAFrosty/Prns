use super::*;

mod grouped;

#[derive(Clone, Copy)]
pub(super) enum DeadlineOrder {
    Admission,
    Reverse,
    Shared,
    OuterPairFirst,
    OuterPairLast,
}

impl DeadlineOrder {
    pub(super) fn at(
        &self,
        origin: SimulationTick,
        started: SimulationTick,
        wave: usize,
    ) -> SimulationTick {
        const FIRST_EXPIRY_MS: u64 = 50;
        const EXPIRY_GAP_MS: u64 = 20;
        match self {
            Self::Admission => tick(started.get() + REQUEST_TIMEOUT),
            Self::Reverse => {
                tick(origin.get() + FIRST_EXPIRY_MS + (WAVES - 1 - wave) as u64 * EXPIRY_GAP_MS)
            }
            Self::Shared => tick(origin.get() + FIRST_EXPIRY_MS),
            Self::OuterPairFirst => {
                tick(origin.get() + FIRST_EXPIRY_MS + if wave == 1 { EXPIRY_GAP_MS } else { 0 })
            }
            Self::OuterPairLast => {
                tick(origin.get() + FIRST_EXPIRY_MS + if wave == 1 { 0 } else { EXPIRY_GAP_MS })
            }
        }
    }
}

fn matrix(exchange: Exchange, boundary: CutBoundary, deadlines: DeadlineOrder) {
    matrix_scenario(
        exchange,
        boundary,
        Scenario {
            retirement: Retirement::KeepAll,
            order: ExchangeOrder::Alternating,
            deadlines,
        },
    );
}

#[test]
fn reverse_deadlines_survive_queued_request_loss() {
    matrix(
        Exchange::Request,
        CutBoundary::Queued,
        DeadlineOrder::Reverse,
    );
}

#[test]
fn reverse_deadlines_survive_consumed_request_loss() {
    matrix(
        Exchange::Request,
        CutBoundary::Consumed,
        DeadlineOrder::Reverse,
    );
}

#[test]
fn reverse_deadlines_survive_queued_response_loss() {
    matrix(
        Exchange::Response,
        CutBoundary::Queued,
        DeadlineOrder::Reverse,
    );
}

#[test]
fn reverse_deadlines_survive_consumed_response_loss() {
    matrix(
        Exchange::Response,
        CutBoundary::Consumed,
        DeadlineOrder::Reverse,
    );
}

#[test]
fn shared_deadline_settles_exact_queued_request_batch() {
    matrix(
        Exchange::Request,
        CutBoundary::Queued,
        DeadlineOrder::Shared,
    );
}

#[test]
fn shared_deadline_settles_exact_consumed_request_batch() {
    matrix(
        Exchange::Request,
        CutBoundary::Consumed,
        DeadlineOrder::Shared,
    );
}

#[test]
fn shared_deadline_settles_exact_queued_response_batch() {
    matrix(
        Exchange::Response,
        CutBoundary::Queued,
        DeadlineOrder::Shared,
    );
}

#[test]
fn shared_deadline_settles_exact_consumed_response_batch() {
    matrix(
        Exchange::Response,
        CutBoundary::Consumed,
        DeadlineOrder::Shared,
    );
}
