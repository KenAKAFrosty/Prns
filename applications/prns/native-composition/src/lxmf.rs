use prns_lxmf::mailbox::{
    CancelLxmfMessageOutcome as ServiceCancelOutcome, DurableLxmfDeliveryState, DurableLxmfMessage,
    DurableLxmfSnapshot, DurableSendDirectTextOutcome, MailboxDirectionFilter, MailboxFailure,
    MailboxListRequest, MailboxProjectionHealth, RetryLxmfMessageOutcome as ServiceRetryOutcome,
};
use prns_lxmf::wire::{
    encoded_basic_lxmf_payload_len, EMPTY_LXMF_FIELDS_ENCODED_BYTES, MAX_BASIC_LXMF_WIRE_BYTES,
    WIRE_HEADER_LENGTH,
};

use crate::contract::{
    CancelLxmfMessageOutcome, ListLxmfMessagesInput, LxmfDeliveryFailure, LxmfDeliveryState,
    LxmfDirection, LxmfHealth, LxmfHealthState, LxmfMessage, LxmfMessageListOutcome,
    LxmfPeerListOutcome, LxmfPeerSummary, LxmfText, LxmfVerification, MeasureLxmfTextInput,
    MeasureLxmfTextOutcome, RetryLxmfMessageOutcome, SendDirectTextOutcome, U64String,
};

const MAX_MESSAGE_PAGE_SIZE: u16 = 100;

pub(crate) fn project_health(snapshot: &DurableLxmfSnapshot) -> Result<LxmfHealth, MailboxFailure> {
    let mailbox_degraded = match &snapshot.mailbox_health {
        MailboxProjectionHealth::Ready => false,
        MailboxProjectionHealth::Busy | MailboxProjectionHealth::Unavailable(_) => true,
        MailboxProjectionHealth::ResetRequired(reason) => {
            return Err(MailboxFailure::ResetRequired(reason.clone()));
        }
    };
    Ok(LxmfHealth {
        state: match (snapshot.health.state, mailbox_degraded) {
            (prns_lxmf::LxmfHealthState::Stopped, _) => LxmfHealthState::Stopped,
            (prns_lxmf::LxmfHealthState::Degraded, _)
            | (prns_lxmf::LxmfHealthState::Ready, true) => LxmfHealthState::Degraded,
            (prns_lxmf::LxmfHealthState::Ready, false) => LxmfHealthState::Ready,
        },
        inbound_overflow_count: U64String::from(snapshot.health.inbound_overflow_count),
    })
}

pub(crate) fn project_peers(
    snapshot: &DurableLxmfSnapshot,
    now_millis: u64,
) -> LxmfPeerListOutcome {
    let peers = snapshot
        .peers
        .iter()
        .map(|peer| LxmfPeerSummary {
            destination: peer.destination,
            display_name: peer.display_name.clone(),
            required_stamp_cost: peer.required_stamp_cost.map(U64String::from),
            last_observed_age_millis: U64String::from(
                now_millis.saturating_sub(peer.observed_at_millis),
            ),
        })
        .collect();
    LxmfPeerListOutcome::Listed { peers }
}

pub(crate) fn mailbox_list_request(
    input: ListLxmfMessagesInput,
) -> Result<MailboxListRequest, LxmfMessageListOutcome> {
    if input.limit == 0 || input.limit > MAX_MESSAGE_PAGE_SIZE {
        return Err(LxmfMessageListOutcome::InvalidInput {
            detail: format!("limit must be an integer from 1 through {MAX_MESSAGE_PAGE_SIZE}"),
        });
    }
    let before = input.before.map(|value| value.0);
    Ok(MailboxListRequest {
        peer: input.peer,
        direction: None::<MailboxDirectionFilter>,
        before,
        limit: usize::from(input.limit),
    })
}

pub(crate) fn project_messages(messages: &[DurableLxmfMessage]) -> LxmfMessageListOutcome {
    LxmfMessageListOutcome::Listed {
        messages: messages.iter().map(project_message).collect(),
    }
}

pub(crate) fn measure_text(input: &MeasureLxmfTextInput) -> MeasureLxmfTextOutcome {
    let Some(payload_bytes) = encoded_basic_lxmf_payload_len(
        input.title.len(),
        input.content.len(),
        EMPTY_LXMF_FIELDS_ENCODED_BYTES,
    ) else {
        return MeasureLxmfTextOutcome::InvalidMessage;
    };
    let Some(wire_bytes) = WIRE_HEADER_LENGTH.checked_add(payload_bytes) else {
        return MeasureLxmfTextOutcome::InvalidMessage;
    };
    let Ok(wire_bytes) = u32::try_from(wire_bytes) else {
        return MeasureLxmfTextOutcome::InvalidMessage;
    };
    if usize::try_from(wire_bytes).unwrap_or(usize::MAX) > MAX_BASIC_LXMF_WIRE_BYTES {
        MeasureLxmfTextOutcome::NeedsResource { wire_bytes }
    } else {
        MeasureLxmfTextOutcome::Measured {
            wire_bytes,
            remaining_bytes: u32::try_from(MAX_BASIC_LXMF_WIRE_BYTES)
                .unwrap_or_default()
                .saturating_sub(wire_bytes),
        }
    }
}

pub(crate) fn project_send_outcome(outcome: DurableSendDirectTextOutcome) -> SendDirectTextOutcome {
    match outcome {
        DurableSendDirectTextOutcome::Accepted { local_record_id } => {
            SendDirectTextOutcome::Accepted {
                local_record_id: U64String::from(local_record_id),
            }
        }
        DurableSendDirectTextOutcome::NeedsResource { wire_bytes } => {
            match u32::try_from(wire_bytes) {
                Ok(wire_bytes) => SendDirectTextOutcome::NeedsResource { wire_bytes },
                Err(_) => SendDirectTextOutcome::DevelopmentUnavailable {
                    detail: "the composed LXMF wire length exceeds the native contract".to_owned(),
                },
            }
        }
        DurableSendDirectTextOutcome::UnsupportedRemoteStampRequirement {
            required_stamp_cost,
        } => SendDirectTextOutcome::UnsupportedRemoteStampRequirement {
            required_stamp_cost: U64String::from(required_stamp_cost),
        },
        DurableSendDirectTextOutcome::PeerIdentityUnavailable => {
            SendDirectTextOutcome::PeerIdentityUnavailable
        }
        DurableSendDirectTextOutcome::DevelopmentUnavailable { detail } => {
            SendDirectTextOutcome::DevelopmentUnavailable { detail }
        }
        DurableSendDirectTextOutcome::DevelopmentResetRequired { reason } => {
            SendDirectTextOutcome::DevelopmentResetRequired { reason }
        }
        DurableSendDirectTextOutcome::LocalNodeStopped => {
            SendDirectTextOutcome::DevelopmentUnavailable {
                detail: "the local node generation stopped before the message was queued"
                    .to_owned(),
            }
        }
        DurableSendDirectTextOutcome::Busy => SendDirectTextOutcome::DevelopmentUnavailable {
            detail: "the bounded development database lane is full".to_owned(),
        },
        DurableSendDirectTextOutcome::InvalidMessage => {
            SendDirectTextOutcome::DevelopmentUnavailable {
                detail: "the native LXMF composer rejected the message".to_owned(),
            }
        }
    }
}

pub(crate) fn project_retry_outcome(outcome: ServiceRetryOutcome) -> RetryLxmfMessageOutcome {
    match outcome {
        ServiceRetryOutcome::Accepted { local_record_id } => RetryLxmfMessageOutcome::Accepted {
            local_record_id: U64String::from(local_record_id),
        },
        ServiceRetryOutcome::NotFound => RetryLxmfMessageOutcome::NotFound,
        ServiceRetryOutcome::NotFailed { current } => RetryLxmfMessageOutcome::NotFailed {
            current: project_delivery_state(current),
        },
        ServiceRetryOutcome::DevelopmentUnavailable { detail } => {
            RetryLxmfMessageOutcome::DevelopmentUnavailable { detail }
        }
        ServiceRetryOutcome::DevelopmentResetRequired { reason } => {
            RetryLxmfMessageOutcome::DevelopmentResetRequired { reason }
        }
        ServiceRetryOutcome::LocalNodeStopped => RetryLxmfMessageOutcome::DevelopmentUnavailable {
            detail: "the local node generation stopped during retry".to_owned(),
        },
        ServiceRetryOutcome::Busy => RetryLxmfMessageOutcome::DevelopmentUnavailable {
            detail: "the bounded development database lane is full".to_owned(),
        },
    }
}

pub(crate) fn project_cancel_outcome(outcome: ServiceCancelOutcome) -> CancelLxmfMessageOutcome {
    match outcome {
        ServiceCancelOutcome::Cancelled { local_record_id } => {
            CancelLxmfMessageOutcome::Cancelled {
                local_record_id: U64String::from(local_record_id),
            }
        }
        ServiceCancelOutcome::NotFound => CancelLxmfMessageOutcome::NotFound,
        ServiceCancelOutcome::AlreadyDelivered => CancelLxmfMessageOutcome::AlreadyDelivered,
        ServiceCancelOutcome::AlreadyCancelled => CancelLxmfMessageOutcome::AlreadyCancelled,
        ServiceCancelOutcome::NotCancellable { current } => {
            CancelLxmfMessageOutcome::NotCancellable {
                current: project_delivery_state(current),
            }
        }
        ServiceCancelOutcome::DevelopmentUnavailable { detail } => {
            CancelLxmfMessageOutcome::DevelopmentUnavailable { detail }
        }
        ServiceCancelOutcome::DevelopmentResetRequired { reason } => {
            CancelLxmfMessageOutcome::DevelopmentResetRequired { reason }
        }
        ServiceCancelOutcome::LocalNodeStopped => {
            CancelLxmfMessageOutcome::DevelopmentUnavailable {
                detail: "the local node generation stopped during cancellation".to_owned(),
            }
        }
        ServiceCancelOutcome::Busy => CancelLxmfMessageOutcome::DevelopmentUnavailable {
            detail: "the bounded development database lane is full".to_owned(),
        },
    }
}

pub(crate) fn message_list_failure(failure: MailboxFailure) -> LxmfMessageListOutcome {
    match failure {
        MailboxFailure::Busy => LxmfMessageListOutcome::DevelopmentUnavailable {
            detail: "the bounded development database lane is full".to_owned(),
        },
        MailboxFailure::Unavailable(detail) => {
            LxmfMessageListOutcome::DevelopmentUnavailable { detail }
        }
        MailboxFailure::ResetRequired(reason) => {
            LxmfMessageListOutcome::DevelopmentResetRequired { reason }
        }
    }
}

pub(crate) fn retry_failure(failure: MailboxFailure) -> RetryLxmfMessageOutcome {
    match failure {
        MailboxFailure::Busy => RetryLxmfMessageOutcome::DevelopmentUnavailable {
            detail: "the bounded development database lane is full".to_owned(),
        },
        MailboxFailure::Unavailable(detail) => {
            RetryLxmfMessageOutcome::DevelopmentUnavailable { detail }
        }
        MailboxFailure::ResetRequired(reason) => {
            RetryLxmfMessageOutcome::DevelopmentResetRequired { reason }
        }
    }
}

pub(crate) fn cancel_failure(failure: MailboxFailure) -> CancelLxmfMessageOutcome {
    match failure {
        MailboxFailure::Busy => CancelLxmfMessageOutcome::DevelopmentUnavailable {
            detail: "the bounded development database lane is full".to_owned(),
        },
        MailboxFailure::Unavailable(detail) => {
            CancelLxmfMessageOutcome::DevelopmentUnavailable { detail }
        }
        MailboxFailure::ResetRequired(reason) => {
            CancelLxmfMessageOutcome::DevelopmentResetRequired { reason }
        }
    }
}

fn project_message(message: &DurableLxmfMessage) -> LxmfMessage {
    LxmfMessage {
        local_record_id: U64String::from(message.local_record_id),
        message_id: message.message_id,
        source: message.source,
        destination: message.destination,
        timestamp: U64String::from(message.timestamp_unix_ms),
        title: project_text(&message.title),
        content: project_text(&message.content),
        direction: match message.direction {
            prns_lxmf::LxmfDirection::Inbound => LxmfDirection::Inbound,
            prns_lxmf::LxmfDirection::Outbound => LxmfDirection::Outbound,
        },
        verification: match message.verification {
            prns_lxmf::LxmfVerification::Verified => LxmfVerification::Verified,
            prns_lxmf::LxmfVerification::SourceUnknown => LxmfVerification::SourceUnknown,
            prns_lxmf::LxmfVerification::InvalidSignature => LxmfVerification::InvalidSignature,
        },
        delivery_state: project_delivery_state(message.delivery_state),
    }
}

fn project_delivery_state(state: DurableLxmfDeliveryState) -> LxmfDeliveryState {
    match state {
        DurableLxmfDeliveryState::Received => LxmfDeliveryState::Received,
        DurableLxmfDeliveryState::Queued { failed_attempts } => LxmfDeliveryState::Queued {
            failed_attempts: U64String::from(failed_attempts),
        },
        DurableLxmfDeliveryState::Sending { failed_attempts } => LxmfDeliveryState::Sending {
            failed_attempts: U64String::from(failed_attempts),
        },
        DurableLxmfDeliveryState::Delivered {
            delivered_at_millis,
            rtt_millis,
        } => LxmfDeliveryState::Delivered {
            delivered_at: U64String::from(delivered_at_millis),
            rtt: rtt_millis.map(U64String::from),
        },
        DurableLxmfDeliveryState::Failed {
            failed_attempts,
            last_failure,
        } => LxmfDeliveryState::Failed {
            failed_attempts: U64String::from(failed_attempts),
            last_failure: project_failure(last_failure),
        },
        DurableLxmfDeliveryState::Cancelled {
            cancelled_at_millis,
        } => LxmfDeliveryState::Cancelled {
            cancelled_at: U64String::from(cancelled_at_millis),
        },
    }
}

const fn project_failure(failure: prns_lxmf::direct::DirectSendFailure) -> LxmfDeliveryFailure {
    match failure {
        prns_lxmf::direct::DirectSendFailure::NoRoute => LxmfDeliveryFailure::NoRoute,
        prns_lxmf::direct::DirectSendFailure::LinkFailed => LxmfDeliveryFailure::LinkFailed,
        prns_lxmf::direct::DirectSendFailure::DeliveryTimedOut => {
            LxmfDeliveryFailure::DeliveryTimedOut
        }
        prns_lxmf::direct::DirectSendFailure::LocalNodeStopped => {
            LxmfDeliveryFailure::LocalNodeStopped
        }
    }
}

fn project_text(bytes: &[u8]) -> LxmfText {
    match std::str::from_utf8(bytes) {
        Ok(value) => LxmfText::Utf8 {
            value: value.to_owned(),
        },
        Err(_) => LxmfText::InvalidUtf8 {
            bytes: bytes.to_vec(),
        },
    }
}

pub(crate) fn parse_canonical_u64(value: &str) -> Option<u64> {
    if value.is_empty()
        || (value.len() > 1 && value.starts_with('0'))
        || !value.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    value.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn health_snapshot(
        state: prns_lxmf::LxmfHealthState,
        mailbox_health: MailboxProjectionHealth,
    ) -> DurableLxmfSnapshot {
        DurableLxmfSnapshot {
            peers: vec![],
            messages: vec![],
            health: prns_lxmf::LxmfHealth {
                state,
                inbound_overflow_count: 3,
            },
            mailbox_health,
            durable_revision: 0,
            projection_revision: 1,
        }
    }

    #[test]
    fn durable_health_combines_callback_and_mailbox_failures() {
        for snapshot in [
            health_snapshot(
                prns_lxmf::LxmfHealthState::Degraded,
                MailboxProjectionHealth::Ready,
            ),
            health_snapshot(
                prns_lxmf::LxmfHealthState::Ready,
                MailboxProjectionHealth::Busy,
            ),
            health_snapshot(
                prns_lxmf::LxmfHealthState::Ready,
                MailboxProjectionHealth::Unavailable("disk full".to_owned()),
            ),
        ] {
            assert_eq!(
                project_health(&snapshot),
                Ok(LxmfHealth {
                    state: LxmfHealthState::Degraded,
                    inbound_overflow_count: U64String::from(3),
                })
            );
        }
        assert_eq!(
            project_health(&health_snapshot(
                prns_lxmf::LxmfHealthState::Ready,
                MailboxProjectionHealth::Ready,
            )),
            Ok(LxmfHealth {
                state: LxmfHealthState::Ready,
                inbound_overflow_count: U64String::from(3),
            })
        );
        assert_eq!(
            project_health(&health_snapshot(
                prns_lxmf::LxmfHealthState::Stopped,
                MailboxProjectionHealth::Unavailable("disk full".to_owned()),
            )),
            Ok(LxmfHealth {
                state: LxmfHealthState::Stopped,
                inbound_overflow_count: U64String::from(3),
            })
        );
        assert_eq!(
            project_health(&health_snapshot(
                prns_lxmf::LxmfHealthState::Ready,
                MailboxProjectionHealth::ResetRequired("corrupt".to_owned()),
            )),
            Err(MailboxFailure::ResetRequired("corrupt".to_owned()))
        );
    }

    #[test]
    fn measurement_matches_the_direct_link_packet_boundary() {
        assert_eq!(
            measure_text(&MeasureLxmfTextInput {
                title: String::new(),
                content: "x".repeat(319),
            }),
            MeasureLxmfTextOutcome::Measured {
                wire_bytes: 431,
                remaining_bytes: 0,
            }
        );
        assert_eq!(
            measure_text(&MeasureLxmfTextInput {
                title: String::new(),
                content: "x".repeat(320),
            }),
            MeasureLxmfTextOutcome::NeedsResource { wire_bytes: 432 }
        );
    }

    #[test]
    fn text_projection_never_claims_invalid_bytes_are_utf8() {
        assert_eq!(
            project_text(b"hello"),
            LxmfText::Utf8 {
                value: "hello".to_owned(),
            }
        );
        assert_eq!(
            project_text(&[0xff, 0xfe]),
            LxmfText::InvalidUtf8 {
                bytes: vec![0xff, 0xfe],
            }
        );
    }

    #[test]
    fn pagination_cursor_is_canonical_u64() {
        assert_eq!(parse_canonical_u64("0"), Some(0));
        assert_eq!(parse_canonical_u64("18446744073709551615"), Some(u64::MAX));
        for invalid in ["", "00", "+1", "-1", "1.0", "18446744073709551616"] {
            assert_eq!(parse_canonical_u64(invalid), None);
        }
    }
}
