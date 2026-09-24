use super::*;

#[test]
fn mixed_queued_request_response_request_losses_preserve_each_deadline() {
    matrix_with_order(
        Exchange::Request,
        CutBoundary::Queued,
        Retirement::KeepAll,
        ExchangeOrder::Alternating,
    );
}

#[test]
fn mixed_consumed_request_response_request_losses_preserve_each_deadline() {
    matrix_with_order(
        Exchange::Request,
        CutBoundary::Consumed,
        Retirement::KeepAll,
        ExchangeOrder::Alternating,
    );
}

#[test]
fn mixed_queued_response_request_response_losses_preserve_each_deadline() {
    matrix_with_order(
        Exchange::Response,
        CutBoundary::Queued,
        Retirement::KeepAll,
        ExchangeOrder::Alternating,
    );
}

#[test]
fn mixed_consumed_response_request_response_losses_preserve_each_deadline() {
    matrix_with_order(
        Exchange::Response,
        CutBoundary::Consumed,
        Retirement::KeepAll,
        ExchangeOrder::Alternating,
    );
}
