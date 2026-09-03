use prns_lxmf::wire::{
    encoded_basic_lxmf_payload_len, EMPTY_LXMF_FIELDS_ENCODED_BYTES, MAX_BASIC_LXMF_WIRE_BYTES,
    WIRE_HEADER_LENGTH,
};

use crate::contract::{
    ListLxmfMessagesInput, LxmfDeliveryFailure, LxmfDeliveryState, LxmfDirection, LxmfHealth,
    LxmfHealthState, LxmfMessage, LxmfMessageListOutcome, LxmfPeerListOutcome, LxmfPeerSummary,
    LxmfText, LxmfVerification, MeasureLxmfTextInput, MeasureLxmfTextOutcome,
    SendDirectTextOutcome, U64String,
};

const MAX_MESSAGE_PAGE_SIZE: u16 = 100;

pub(crate) fn project_health(health: prns_lxmf::LxmfHealth) -> LxmfHealth {
    LxmfHealth {
        state: match health.state {
            prns_lxmf::LxmfHealthState::Ready => LxmfHealthState::Ready,
            prns_lxmf::LxmfHealthState::Degraded => LxmfHealthState::Degraded,
            prns_lxmf::LxmfHealthState::Stopped => LxmfHealthState::Stopped,
        },
        inbound_overflow_count: U64String::from(health.inbound_overflow_count),
    }
}

pub(crate) fn project_peers(
    snapshot: &prns_lxmf::direct::LxmfSnapshot,
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

pub(crate) fn project_messages(
    snapshot: &prns_lxmf::direct::LxmfSnapshot,
    input: ListLxmfMessagesInput,
) -> LxmfMessageListOutcome {
    if input.limit == 0 || input.limit > MAX_MESSAGE_PAGE_SIZE {
        return LxmfMessageListOutcome::InvalidInput {
            detail: format!("limit must be an integer from 1 through {MAX_MESSAGE_PAGE_SIZE}"),
        };
    }
    let before = match input.before {
        Some(value) => match parse_canonical_u64(&value.0) {
            Some(value) => Some(value),
            None => {
                return LxmfMessageListOutcome::InvalidInput {
                    detail: "before must be a canonical unsigned 64-bit decimal string".to_owned(),
                }
            }
        },
        None => None,
    };
    let messages = snapshot
        .messages
        .iter()
        .rev()
        .filter(|message| before.is_none_or(|before| message.local_record_id < before))
        .filter(|message| {
            input
                .peer
                .is_none_or(|peer| message.source == peer || message.destination == peer)
        })
        .take(usize::from(input.limit))
        .map(project_message)
        .collect();
    LxmfMessageListOutcome::Listed { messages }
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

pub(crate) fn project_send_outcome(
    outcome: prns_lxmf::direct::SendDirectTextOutcome,
) -> SendDirectTextOutcome {
    match outcome {
        prns_lxmf::direct::SendDirectTextOutcome::Started { local_record_id } => {
            SendDirectTextOutcome::Started {
                local_record_id: U64String::from(local_record_id),
            }
        }
        prns_lxmf::direct::SendDirectTextOutcome::NeedsResource { wire_bytes } => {
            match u32::try_from(wire_bytes) {
                Ok(wire_bytes) => SendDirectTextOutcome::NeedsResource { wire_bytes },
                Err(_) => SendDirectTextOutcome::InvalidMessage,
            }
        }
        prns_lxmf::direct::SendDirectTextOutcome::UnsupportedRemoteStampRequirement => {
            SendDirectTextOutcome::UnsupportedRemoteStampRequirement
        }
        prns_lxmf::direct::SendDirectTextOutcome::PeerIdentityUnavailable => {
            SendDirectTextOutcome::PeerIdentityUnavailable
        }
        prns_lxmf::direct::SendDirectTextOutcome::NoRoute => SendDirectTextOutcome::NoRoute,
        prns_lxmf::direct::SendDirectTextOutcome::LinkFailed => SendDirectTextOutcome::LinkFailed,
        prns_lxmf::direct::SendDirectTextOutcome::DeliveryTimedOut => {
            SendDirectTextOutcome::DeliveryTimedOut
        }
        prns_lxmf::direct::SendDirectTextOutcome::LocalNodeStopped => {
            SendDirectTextOutcome::LocalNodeStopped
        }
        prns_lxmf::direct::SendDirectTextOutcome::InvalidMessage => {
            SendDirectTextOutcome::InvalidMessage
        }
    }
}

fn project_message(message: &prns_lxmf::direct::LxmfMessage) -> LxmfMessage {
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
        delivery_state: match message.delivery_state {
            prns_lxmf::LxmfDeliveryState::Received => LxmfDeliveryState::Received,
            prns_lxmf::LxmfDeliveryState::Sending => LxmfDeliveryState::Sending,
            prns_lxmf::LxmfDeliveryState::Delivered => LxmfDeliveryState::Delivered,
            prns_lxmf::LxmfDeliveryState::Failed => LxmfDeliveryState::Failed,
        },
        failure: message.failure.map(|failure| match failure {
            prns_lxmf::direct::DirectSendFailure::NoRoute => LxmfDeliveryFailure::NoRoute,
            prns_lxmf::direct::DirectSendFailure::LinkFailed => LxmfDeliveryFailure::LinkFailed,
            prns_lxmf::direct::DirectSendFailure::DeliveryTimedOut => {
                LxmfDeliveryFailure::DeliveryTimedOut
            }
            prns_lxmf::direct::DirectSendFailure::LocalNodeStopped => {
                LxmfDeliveryFailure::LocalNodeStopped
            }
        }),
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

fn parse_canonical_u64(value: &str) -> Option<u64> {
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
