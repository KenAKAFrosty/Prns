//! Safe metadata only; the embedding opts into this exact target and owns its sink.
use std::fmt;
#[cfg(any(feature = "log", test))]
use std::sync::atomic::{AtomicU32, Ordering};

use super::{CloseReason, HandshakeFailure, HandshakeOutcome, HandshakeRole};
use prns_core::interfaces::bluetooth_auto::Control;

#[cfg(feature = "log")]
const TARGET: &str = "prns.bluetooth.diagnostics";
#[cfg(any(feature = "log", test))]
const LIMIT: u32 = 64;
#[cfg(any(feature = "log", test))]
const REJECTION_LIMIT: u32 = 32;
#[cfg(feature = "log")]
static EVENTS: AtomicU32 = AtomicU32::new(0);
#[cfg(feature = "log")]
static REJECTIONS: AtomicU32 = AtomicU32::new(0);

#[cfg(any(feature = "log", test))]
fn reserve(counter: &AtomicU32, limit: u32) -> Option<u32> {
    {
        let mut previous = counter.load(Ordering::Relaxed);
        loop {
            let Some(next) = (previous <= limit).then(|| previous + 1) else {
                break Err(previous);
            };
            match counter.compare_exchange_weak(
                previous,
                next,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(previous) => break Ok(previous),
                Err(current) => previous = current,
            }
        }
    }
    .ok()
}

pub(super) fn emit(message: fmt::Arguments<'_>) {
    #[cfg(feature = "log")]
    if log::log_enabled!(target: TARGET, log::Level::Info) {
        match reserve(&EVENTS, LIMIT) {
            Some(count) if count < LIMIT => log::info!(target: TARGET, "{message}"),
            Some(_) => {
                log::info!(target: TARGET, "stage=diagnostics category=family_limit family=supervisor limit={LIMIT} dropped_at_least=1")
            }
            None => {}
        }
    }
    #[cfg(not(feature = "log"))]
    let _ = message;
}

/// Only static variant labels cross this boundary; no protocol field is formatted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct RejectionRecord {
    category: &'static str,
    protocol: &'static str,
    role: &'static str,
    input: &'static str,
    reply: &'static str,
    outcome: &'static str,
}

impl fmt::Display for RejectionRecord {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            output,
            "stage=rejection category={} protocol={} role={} input={} reply={} outcome={}",
            self.category, self.protocol, self.role, self.input, self.reply, self.outcome
        )
    }
}

fn role_name(role: HandshakeRole) -> &'static str {
    match role {
        HandshakeRole::Dialer => "dialer",
        HandshakeRole::Listener => "listener",
    }
}

fn reason_name(reason: CloseReason) -> &'static str {
    match reason {
        CloseReason::Incompatible => "incompatible",
        CloseReason::SelfConnection => "self_connection",
        CloseReason::DuplicateLink => "duplicate_link",
    }
}

fn control_name(control: &Control) -> &'static str {
    match control {
        Control::Hello { .. } => "hello",
        Control::Welcome { .. } => "welcome",
        Control::Close {
            reason: CloseReason::Incompatible,
        } => "close_incompatible",
        Control::Close {
            reason: CloseReason::SelfConnection,
        } => "close_self_connection",
        Control::Close {
            reason: CloseReason::DuplicateLink,
        } => "close_duplicate_link",
        Control::Probe { .. } => "probe",
        Control::ProbeReply { .. } => "probe_reply",
    }
}

fn native_rejection_record(
    role: HandshakeRole,
    input: &Control,
    reply: Option<&Control>,
    outcome: &HandshakeOutcome,
) -> Option<RejectionRecord> {
    let HandshakeOutcome::Aborted(reason) = outcome else {
        return None;
    };
    Some(RejectionRecord {
        category: "native_reaction",
        protocol: "native",
        role: role_name(role),
        input: control_name(input),
        reply: reply.map_or("none", control_name),
        outcome: reason_name(*reason),
    })
}

pub(super) fn native_rejection(
    role: HandshakeRole,
    input: &Control,
    reply: Option<&Control>,
    outcome: &HandshakeOutcome,
) {
    emit_rejection(native_rejection_record(role, input, reply, outcome));
}

pub(super) fn columba_policy_rejection(role: HandshakeRole) {
    // Called only from the adapter's existing early rejection, not a repeated policy check.
    emit_rejection(Some(columba_rejection_record(role)));
}

fn columba_rejection_record(role: HandshakeRole) -> RejectionRecord {
    RejectionRecord {
        category: "columba_policy_reject",
        protocol: "columba",
        role: role_name(role),
        input: "none",
        reply: "none",
        outcome: "incompatible",
    }
}

#[cfg(any(feature = "log", test))]
#[derive(Debug, PartialEq, Eq)]
enum RejectionEvent {
    Record(RejectionRecord),
    Limit,
}

#[cfg(any(feature = "log", test))]
fn rejection_event(
    counter: &AtomicU32,
    enabled: bool,
    record: Option<RejectionRecord>,
) -> Option<RejectionEvent> {
    if !enabled {
        return None;
    }
    let record = record?;
    match reserve(counter, REJECTION_LIMIT) {
        Some(count) if count < REJECTION_LIMIT => Some(RejectionEvent::Record(record)),
        Some(_) => Some(RejectionEvent::Limit),
        None => None,
    }
}

fn emit_rejection(record: Option<RejectionRecord>) {
    #[cfg(feature = "log")]
    match rejection_event(
        &REJECTIONS,
        log::log_enabled!(target: TARGET, log::Level::Info),
        record,
    ) {
        Some(RejectionEvent::Record(record)) => log::info!(target: TARGET, "{record}"),
        Some(RejectionEvent::Limit) => log::info!(target: TARGET,
            "stage=diagnostics category=family_limit family=rejection limit={REJECTION_LIMIT} dropped_at_least=1"),
        None => {}
    }
    #[cfg(not(feature = "log"))]
    let _ = record;
}

pub(super) fn handshake_category<T>(outcome: &Result<T, HandshakeFailure>) -> &'static str {
    match outcome {
        Ok(_) => "validated",
        Err(HandshakeFailure::Timeout) => "timeout",
        Err(HandshakeFailure::MissingColumbaIdentity) => "missing_identity",
        Err(HandshakeFailure::ColumbaIdentitySend) => "identity_send_failed",
        Err(HandshakeFailure::InitialSend) => "initial_send_failed",
        Err(HandshakeFailure::Recv) => "receive_failed",
        Err(HandshakeFailure::ReplySend) => "reply_send_failed",
        Err(HandshakeFailure::Aborted(CloseReason::Incompatible)) => "incompatible",
        Err(HandshakeFailure::Aborted(CloseReason::SelfConnection)) => "self_connection",
        Err(HandshakeFailure::Aborted(CloseReason::DuplicateLink)) => "duplicate_link",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prns_core::interfaces::bluetooth_auto::{
        AppleHost, BleIdentity, DiscoveryGroupSet, Endpoint, Handshake, LinkCapabilities,
        LocalPeer, PeerDiscoveryGroups,
    };
    use prns_core::interfaces::DiscoveryGroupId;

    fn peer(identity: u8, group: &str) -> LocalPeer {
        LocalPeer {
            identity: BleIdentity::new([identity; 16]),
            endpoint: Endpoint::CoreBluetooth(AppleHost::Ios),
            capabilities: LinkCapabilities {
                l2cap: None,
                link_mtu: 512,
            },
            discovery_groups: DiscoveryGroupSet::singleton(DiscoveryGroupId::parse(group).unwrap())
                .unwrap()
                .hashes(),
        }
    }

    fn greeting(peer: LocalPeer, welcome: bool) -> Control {
        let discovery_groups = PeerDiscoveryGroups::Explicit(peer.discovery_groups);
        if welcome {
            Control::Welcome {
                identity: peer.identity,
                endpoint: peer.endpoint,
                capabilities: peer.capabilities,
                peer_rssi: Some(-47),
                discovery_groups,
            }
        } else {
            Control::Hello {
                identity: peer.identity,
                endpoint: peer.endpoint,
                capabilities: peer.capabilities,
                peer_rssi: Some(-47),
                discovery_groups,
            }
        }
    }

    #[test]
    fn rejection_records_describe_actual_core_reactions_without_rechecking_policy() {
        let local = peer(1, "local-private-group");
        let other = peer(2, "other-private-group");
        for (role, input, expected_input) in [
            (HandshakeRole::Listener, greeting(other, false), "hello"),
            (HandshakeRole::Dialer, greeting(other, true), "welcome"),
        ] {
            let (mut handshake, _) = Handshake::begin(role, local, None);
            let reaction = handshake.absorb(local, input);
            let record =
                native_rejection_record(role, &input, reaction.reply.as_ref(), &reaction.outcome)
                    .unwrap();
            assert_eq!(record.input, expected_input);
            assert_eq!(record.reply, "close_incompatible");
            assert_eq!(record.outcome, "incompatible");
        }
        let (mut handshake, _) = Handshake::begin(HandshakeRole::Dialer, local, None);
        let input = Control::Close {
            reason: CloseReason::Incompatible,
        };
        let reaction = handshake.absorb(local, input);
        let record = native_rejection_record(
            HandshakeRole::Dialer,
            &input,
            reaction.reply.as_ref(),
            &reaction.outcome,
        )
        .unwrap();
        assert_eq!(record.input, "close_incompatible");
        assert_eq!(record.reply, "none");

        let input = Control::Probe { nonce: u64::MAX };
        let reaction = handshake.absorb(local, input);
        let record = native_rejection_record(
            HandshakeRole::Dialer,
            &input,
            reaction.reply.as_ref(),
            &reaction.outcome,
        )
        .unwrap();
        assert_eq!(record.input, "probe");
        assert_eq!(record.reply, "close_incompatible");

        let input = greeting(peer(2, "local-private-group"), true);
        let reaction = handshake.absorb(local, input);
        assert!(matches!(reaction.outcome, HandshakeOutcome::Settled(_)));
        assert!(native_rejection_record(
            HandshakeRole::Dialer,
            &input,
            reaction.reply.as_ref(),
            &reaction.outcome
        )
        .is_none());
        assert!(native_rejection_record(
            HandshakeRole::Dialer,
            &input,
            None,
            &HandshakeOutcome::Pending
        )
        .is_none());
    }

    #[test]
    fn rejection_metadata_formats_only_the_closed_variant_whitelist() {
        let abort = HandshakeOutcome::Aborted(CloseReason::Incompatible);
        let close = Control::Close {
            reason: CloseReason::Incompatible,
        };
        for (input, expected) in [
            (greeting(peer(0x12, "secret-alpha"), false), "hello"),
            (greeting(peer(0xfe, "secret-beta"), false), "hello"),
            (greeting(peer(0x12, "secret-alpha"), true), "welcome"),
            (greeting(peer(0xfe, "secret-beta"), true), "welcome"),
            (Control::Probe { nonce: 0 }, "probe"),
            (Control::Probe { nonce: u64::MAX }, "probe"),
            (Control::ProbeReply { nonce: 0 }, "probe_reply"),
            (Control::ProbeReply { nonce: u64::MAX }, "probe_reply"),
            (close, "close_incompatible"),
            (
                Control::Close {
                    reason: CloseReason::SelfConnection,
                },
                "close_self_connection",
            ),
            (
                Control::Close {
                    reason: CloseReason::DuplicateLink,
                },
                "close_duplicate_link",
            ),
        ] {
            let record =
                native_rejection_record(HandshakeRole::Dialer, &input, Some(&close), &abort)
                    .unwrap();
            assert_eq!(record.to_string(), format!("stage=rejection category=native_reaction protocol=native role=dialer input={expected} reply=close_incompatible outcome=incompatible"));
            assert_eq!(control_name(&input), expected);
        }
        for role in [HandshakeRole::Dialer, HandshakeRole::Listener] {
            let record = columba_rejection_record(role);
            assert_eq!(record.to_string(), format!("stage=rejection category=columba_policy_reject protocol=columba role={} input=none reply=none outcome=incompatible", role_name(role)));
        }
    }

    #[test]
    fn rejection_budget_is_independent_and_success_or_disabled_events_use_none() {
        let supervisor = AtomicU32::new(0);
        while reserve(&supervisor, LIMIT).is_some() {}
        let rejections = AtomicU32::new(0);
        let record = columba_rejection_record(HandshakeRole::Dialer);
        for _ in 0..100 {
            assert_eq!(rejection_event(&rejections, false, Some(record)), None);
            assert_eq!(rejection_event(&rejections, true, None), None);
        }
        assert_eq!(rejections.load(Ordering::Relaxed), 0);
        for _ in 0..REJECTION_LIMIT {
            assert_eq!(
                rejection_event(&rejections, true, Some(record)),
                Some(RejectionEvent::Record(record))
            );
        }
        assert_eq!(
            rejection_event(&rejections, true, Some(record)),
            Some(RejectionEvent::Limit)
        );
        assert_eq!(rejection_event(&rejections, true, Some(record)), None);
        assert_eq!(supervisor.load(Ordering::Relaxed), LIMIT + 1);
        rejections.store(u32::MAX, Ordering::Relaxed);
        assert_eq!(rejection_event(&rejections, true, Some(record)), None);
    }

    #[test]
    fn diagnostic_handshake_outcomes_do_not_conflate_validation_timeout_or_coarse_rejection() {
        assert_eq!(
            handshake_category(&Ok::<(), HandshakeFailure>(())),
            "validated"
        );
        for (failure, expected) in [
            (HandshakeFailure::Timeout, "timeout"),
            (HandshakeFailure::Recv, "receive_failed"),
            (
                HandshakeFailure::Aborted(CloseReason::Incompatible),
                "incompatible",
            ),
            (
                HandshakeFailure::Aborted(CloseReason::DuplicateLink),
                "duplicate_link",
            ),
        ] {
            assert_eq!(handshake_category(&Err::<(), _>(failure)), expected);
        }
    }

    #[test]
    fn supervisor_trace_has_one_cap_slot_and_cannot_wrap() {
        let count = AtomicU32::new(0);
        for expected in 0..=LIMIT {
            assert_eq!(reserve(&count, LIMIT), Some(expected));
        }
        assert_eq!(reserve(&count, LIMIT), None);
        count.store(u32::MAX, Ordering::Relaxed);
        assert_eq!(reserve(&count, LIMIT), None);
    }
}
