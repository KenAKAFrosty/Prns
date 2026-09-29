use super::*;

fn matrix(boundary: CutBoundary, retirement: Retirement) {
    for exchange in [Exchange::Request, Exchange::Response] {
        matrix_scenario(
            exchange,
            boundary,
            Scenario {
                retirement,
                order: ExchangeOrder::Alternating,
                deadlines: deadlines::DeadlineOrder::Reverse,
            },
        );
    }
}

#[test]
fn cancelling_earliest_queued_deadline_preserves_later_callers() {
    matrix(CutBoundary::Queued, Retirement::CancelLast);
}

#[test]
fn cancelling_earliest_consumed_deadline_preserves_later_callers() {
    matrix(CutBoundary::Consumed, Retirement::CancelLast);
}

#[test]
fn cancelling_latest_queued_deadline_preserves_earlier_callers() {
    matrix(CutBoundary::Queued, Retirement::CancelFirst);
}

#[test]
fn cancelling_latest_consumed_deadline_preserves_earlier_callers() {
    matrix(CutBoundary::Consumed, Retirement::CancelFirst);
}

#[test]
fn cancelling_all_queued_callers_leaves_silent_deadlines_and_live_traffic() {
    matrix(CutBoundary::Queued, Retirement::CancelAll);
}

#[test]
fn cancelling_all_consumed_callers_leaves_silent_deadlines_and_live_traffic() {
    matrix(CutBoundary::Consumed, Retirement::CancelAll);
}
