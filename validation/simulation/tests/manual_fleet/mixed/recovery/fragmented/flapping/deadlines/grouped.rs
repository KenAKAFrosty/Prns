use super::*;

fn matrix(boundary: CutBoundary, deadlines: DeadlineOrder, retirement: Retirement) {
    for exchange in [Exchange::Request, Exchange::Response] {
        matrix_scenario(
            exchange,
            boundary,
            Scenario {
                retirement,
                order: ExchangeOrder::Alternating,
                deadlines,
            },
        );
    }
}

#[test]
fn early_queued_pair_leaves_middle_caller_pending() {
    matrix(
        CutBoundary::Queued,
        DeadlineOrder::OuterPairFirst,
        Retirement::KeepAll,
    );
}

#[test]
fn early_consumed_pair_leaves_middle_caller_pending() {
    matrix(
        CutBoundary::Consumed,
        DeadlineOrder::OuterPairFirst,
        Retirement::KeepAll,
    );
}

#[test]
fn late_queued_pair_survives_middle_caller_expiry() {
    matrix(
        CutBoundary::Queued,
        DeadlineOrder::OuterPairLast,
        Retirement::KeepAll,
    );
}

#[test]
fn late_consumed_pair_survives_middle_caller_expiry() {
    matrix(
        CutBoundary::Consumed,
        DeadlineOrder::OuterPairLast,
        Retirement::KeepAll,
    );
}

#[test]
fn cancelled_queued_pair_member_preserves_both_deadline_batches() {
    matrix(
        CutBoundary::Queued,
        DeadlineOrder::OuterPairFirst,
        Retirement::CancelLast,
    );
}

#[test]
fn cancelled_consumed_pair_member_preserves_both_deadline_batches() {
    matrix(
        CutBoundary::Consumed,
        DeadlineOrder::OuterPairFirst,
        Retirement::CancelLast,
    );
}
