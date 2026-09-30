use super::*;
use crate::interfaces::bluetooth_auto::{
    AndroidHost, BleIdentity, BleUuid, CloseReason, ControlParseError, DiscoveryGroupSet, Endpoint,
    Handshake, HandshakeOutcome, HandshakeRole, LinkCapabilities, LocalPeer, PeerDiscoveryGroups,
    NATIVE_LIVENESS_UUID,
};

fn send(policy: &mut LivenessPolicy, now_ms: u64) -> LivenessWrite {
    let LivenessAction::Send(write) = policy.poll(now_ms) else {
        panic!("expected one reserved liveness write");
    };
    write
}

fn reply(nonce: u64) -> Control {
    Control::ProbeReply { nonce }
}

fn greeting() -> Control {
    Control::Hello {
        identity: BleIdentity::new([1; 16]),
        endpoint: Endpoint::Android(AndroidHost::Android),
        capabilities: LinkCapabilities {
            l2cap: None,
            link_mtu: 247,
        },
        peer_rssi: None,
        discovery_groups: PeerDiscoveryGroups::LegacyReticulum,
    }
}

#[test]
fn legacy_greeting_bytes_are_unchanged() {
    let expected = [
        1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 3, 0, 0, 0, 247, 128,
    ];
    let mut encoded = [0; 23];
    assert_eq!(greeting().encode(&mut encoded), Some(23));
    assert_eq!(encoded, expected);
    assert_eq!(Control::try_decode(&expected), Ok(greeting()));
}

#[test]
fn probes_cannot_replace_the_greeting_handshake() {
    let local = LocalPeer {
        identity: BleIdentity::new([1; 16]),
        endpoint: Endpoint::Android(AndroidHost::Android),
        capabilities: LinkCapabilities {
            l2cap: None,
            link_mtu: 247,
        },
        discovery_groups: DiscoveryGroupSet::reticulum().hashes(),
    };
    for role in [HandshakeRole::Dialer, HandshakeRole::Listener] {
        for control in [Control::Probe { nonce: 7 }, reply(7)] {
            let (mut handshake, _) = Handshake::begin(role, local, None);
            let result = handshake.absorb(local, control);
            assert_eq!(
                result.outcome,
                HandshakeOutcome::Aborted(CloseReason::Incompatible)
            );
        }
    }
}

#[test]
fn greeting_flood_neither_activates_listener_nor_postpones_probe() {
    let mut listener = LivenessPolicy::new(LivenessMode::Listener, 0, 7);
    let mut initiator = LivenessPolicy::new(LivenessMode::Initiator, 0, 7);
    let write = send(&mut initiator, 0);
    initiator.sent(0, write, true);
    for time in 1..30_000 {
        listener.receive(time, &greeting());
        initiator.receive(time, &greeting());
    }
    assert_eq!(listener.poll(30_000), LivenessAction::Wait);
    assert_eq!(listener.next_deadline_ms(), None);
    assert_eq!(initiator.poll(30_000), LivenessAction::Retire);
}

#[test]
fn capability_is_exact_versioned_value_on_optional_native_uuid() {
    assert_eq!(LIVENESS_CAPABILITY_BYTES, *b"PRNS\x01\x01");
    assert!(supports_liveness_capability(&LIVENESS_CAPABILITY_BYTES));
    for len in 0..LIVENESS_CAPABILITY_BYTES.len() {
        assert!(!supports_liveness_capability(
            &LIVENESS_CAPABILITY_BYTES[..len]
        ));
    }
    for index in 0..LIVENESS_CAPABILITY_BYTES.len() {
        let mut unsupported = LIVENESS_CAPABILITY_BYTES;
        unsupported[index] ^= 0xff;
        assert!(!supports_liveness_capability(&unsupported));
    }
    assert!(!supports_liveness_capability(b"PRNS\x01\x01\0"));
    assert_eq!(
        NATIVE_LIVENESS_UUID,
        BleUuid::Bit128([
            0x37, 0x14, 0x5b, 0x00, 0x44, 0x2d, 0x4a, 0x94, 0x91, 0x7f, 0x8f, 0x42, 0xc5, 0xda,
            0x28, 0xe9,
        ]),
    );
}

#[test]
fn probe_wire_tags_are_exact_nine_byte_big_endian_messages() {
    for nonce in [0, 0x0102_0304_0506_0708, u64::MAX] {
        for (tag, control) in [(4, Control::Probe { nonce }), (5, reply(nonce))] {
            let mut encoded = [0xaa; 16];
            assert_eq!(control.encode(&mut encoded), Some(9));
            assert_eq!(encoded[0], tag);
            assert_eq!(&encoded[1..9], &nonce.to_be_bytes());
            assert_eq!(Control::try_decode(&encoded[..9]), Ok(control));
            assert_eq!(&encoded[9..], &[0xaa; 7]);
            let mut short = [0xaa; 8];
            assert_eq!(control.encode(&mut short), None);
            assert_eq!(short, [0xaa; 8]);
        }
    }
}

#[test]
fn probe_decoder_rejects_missing_or_trailing_bytes() {
    for tag in [4, 5] {
        for length in 1..20 {
            let mut bytes = [0u8; 20];
            bytes[0] = tag;
            if length != 9 {
                assert_eq!(
                    Control::try_decode(&bytes[..length]),
                    Err(ControlParseError::InvalidLength),
                );
            }
        }
    }
    assert_eq!(
        Control::try_decode(&[6]),
        Err(ControlParseError::UnknownKind(6))
    );
    assert_eq!(super::super::CONTROL_MAX_LEN, 153);
}

#[test]
fn disabled_and_unactivated_listener_have_no_timeouts() {
    for mode in [LivenessMode::Disabled, LivenessMode::Listener] {
        let mut policy = LivenessPolicy::new(mode, 0, 7);
        policy.receive(1, &reply(7));
        assert_eq!(policy.poll(u64::MAX), LivenessAction::Wait);
        assert_eq!(policy.next_deadline_ms(), None);
        // There is still no negotiated timeout after a clock reset.
        assert_eq!(policy.poll(0), LivenessAction::Wait);
    }
    let mut disabled = LivenessPolicy::new(LivenessMode::Disabled, 0, 7);
    disabled.receive(0, &Control::Probe { nonce: 42 });
    assert_eq!(disabled.poll(u64::MAX), LivenessAction::Wait);
}

#[test]
fn initiator_probes_immediately_once_and_local_success_is_not_a_reply() {
    let mut policy = LivenessPolicy::new(LivenessMode::Initiator, 100, 7);
    assert_eq!(policy.next_deadline_ms(), Some(100));
    let write = send(&mut policy, 100);
    assert_eq!(write.control(), Control::Probe { nonce: 7 });
    assert_eq!(write.deadline_ms, 30_100);
    assert_eq!(policy.poll(100), LivenessAction::Wait);
    policy.sent(101, write, true);
    assert_eq!(policy.next_deadline_ms(), Some(30_100));
    assert_eq!(policy.poll(30_099), LivenessAction::Wait);
    assert_eq!(policy.poll(30_100), LivenessAction::Retire);
}

#[test]
fn listener_activates_only_from_probe_and_starts_own_cadence() {
    let mut policy = LivenessPolicy::new(LivenessMode::Listener, 0, 7);
    policy.receive(10, &reply(7));
    assert_eq!(policy.poll(10), LivenessAction::Wait);
    policy.receive(100, &Control::Probe { nonce: 42 });
    let response = send(&mut policy, 100);
    assert_eq!(response.control(), reply(42));
    policy.sent(101, response, true);
    assert_eq!(policy.next_deadline_ms(), Some(30_100));
    assert_eq!(
        send(&mut policy, 30_100).control(),
        Control::Probe { nonce: 7 }
    );
}

#[test]
fn only_matching_timely_reply_starts_next_idle_interval() {
    let mut policy = LivenessPolicy::new(LivenessMode::Initiator, 0, 7);
    let write = send(&mut policy, 0);
    policy.sent(1, write, true);
    policy.receive(10, &reply(8));
    assert_eq!(policy.next_deadline_ms(), Some(30_000));
    policy.receive(20, &reply(7));
    assert_eq!(policy.next_deadline_ms(), Some(30_020));
    policy.receive(29_999, &reply(7));
    assert_eq!(policy.next_deadline_ms(), Some(30_020));
    let next = send(&mut policy, 30_020);
    assert_eq!(next.control(), Control::Probe { nonce: 8 });
    policy.sent(30_021, next, true);
    policy.receive(30_022, &reply(7));
    assert_eq!(policy.poll(60_020), LivenessAction::Retire);
}

#[test]
fn reply_at_deadline_is_late_even_before_timeout_poll() {
    for time in [30_000, 30_001, 900_000] {
        let mut policy = LivenessPolicy::new(LivenessMode::Initiator, 0, 7);
        let write = send(&mut policy, 0);
        policy.sent(1, write, true);
        policy.receive(time, &reply(7));
        assert_eq!(policy.poll(time), LivenessAction::Retire);
    }
}

#[test]
fn timely_reply_does_not_allow_late_local_completion() {
    let mut policy = LivenessPolicy::new(LivenessMode::Initiator, 0, 7);
    let write = send(&mut policy, 0);
    policy.receive(1, &reply(7));
    policy.sent(30_000, write, true);
    assert_eq!(policy.poll(30_000), LivenessAction::Retire);
}

#[test]
fn early_reply_is_retained_but_send_must_still_complete() {
    let mut policy = LivenessPolicy::new(LivenessMode::Initiator, 0, 7);
    let write = send(&mut policy, 0);
    policy.receive(10, &reply(7));
    assert_eq!(policy.poll(11), LivenessAction::Wait);
    policy.sent(20, write, true);
    assert_eq!(policy.next_deadline_ms(), Some(30_010));
}

#[test]
fn delayed_first_poll_does_not_restart_original_budget() {
    let mut policy = LivenessPolicy::new(LivenessMode::Initiator, 100, 7);
    assert_eq!(send(&mut policy, 30_000).deadline_ms, 30_100);
    assert_eq!(policy.poll(30_100), LivenessAction::Retire);
    let mut suspended = LivenessPolicy::new(LivenessMode::Initiator, 100, 7);
    assert_eq!(suspended.poll(90_000), LivenessAction::Retire);
}

#[test]
fn suspension_after_healthy_reply_expires_from_original_due_time() {
    let mut policy = LivenessPolicy::new(LivenessMode::Initiator, 0, 7);
    let write = send(&mut policy, 0);
    policy.sent(1, write, true);
    policy.receive(10, &reply(7));
    assert_eq!(policy.poll(60_010), LivenessAction::Retire);
}

#[test]
fn stale_completion_cannot_complete_or_fail_new_write() {
    let mut policy = LivenessPolicy::new(LivenessMode::Initiator, 0, 7);
    let first = send(&mut policy, 0);
    policy.sent(1, first, true);
    policy.receive(2, &reply(7));
    let second = send(&mut policy, 30_002);
    policy.sent(30_003, first, true);
    policy.sent(30_004, first, false);
    assert_eq!(policy.poll(30_005), LivenessAction::Wait);
    policy.sent(30_006, second, true);
    policy.receive(30_007, &reply(8));
    assert_eq!(policy.next_deadline_ms(), Some(60_007));
}

#[test]
fn failed_write_is_terminal_even_with_matching_peer_reply() {
    let mut policy = LivenessPolicy::new(LivenessMode::Initiator, 0, 7);
    let write = send(&mut policy, 0);
    policy.receive(1, &reply(7));
    policy.sent(2, write, false);
    assert_eq!(policy.poll(2), LivenessAction::Retire);
    policy.sent(3, write, true);
    assert_eq!(policy.poll(3), LivenessAction::Retire);
}

#[test]
fn reply_slot_keeps_first_nonce_and_deadline_under_flood() {
    let mut policy = LivenessPolicy::new(LivenessMode::Listener, 0, 7);
    policy.receive(10, &Control::Probe { nonce: 100 });
    for time in 11..2_000 {
        policy.receive(time, &Control::Probe { nonce: time });
    }
    let write = send(&mut policy, 2_000);
    assert_eq!(write.control(), reply(100));
    assert_eq!(write.deadline_ms, 30_010);
    for time in 2_001..3_000 {
        policy.receive(time, &Control::Probe { nonce: time });
    }
    assert_eq!(policy.poll(30_010), LivenessAction::Retire);
}

#[test]
fn replies_are_rate_limited_and_do_not_postpone_local_probe() {
    let mut policy = LivenessPolicy::new(LivenessMode::Listener, 0, 7);
    policy.receive(0, &Control::Probe { nonce: 100 });
    let first = send(&mut policy, 0);
    policy.sent(1, first, true);
    for time in 2..1_000 {
        policy.receive(time, &Control::Probe { nonce: time });
        assert_eq!(policy.poll(time), LivenessAction::Wait);
    }
    for time in (1_000..30_000).step_by(1_000) {
        policy.receive(time, &Control::Probe { nonce: time });
        let response = send(&mut policy, time);
        assert_eq!(response.kind, LivenessWriteKind::Reply);
        policy.sent(time + 1, response, true);
    }
    policy.receive(30_000, &Control::Probe { nonce: 123 });
    let probe = send(&mut policy, 30_000);
    assert_eq!(probe.control(), Control::Probe { nonce: 7 });
    policy.sent(30_001, probe, true);
    assert_eq!(policy.poll(60_000), LivenessAction::Retire);
}

#[test]
fn reply_send_is_capped_by_outstanding_probe_deadline() {
    let mut policy = LivenessPolicy::new(LivenessMode::Initiator, 0, 7);
    let probe = send(&mut policy, 0);
    policy.sent(1, probe, true);
    policy.receive(20_000, &Control::Probe { nonce: 100 });
    let response = send(&mut policy, 20_000);
    assert_eq!(response.deadline_ms, 30_000);
    policy.sent(30_000, response, true);
    assert_eq!(policy.poll(30_000), LivenessAction::Retire);
}

#[test]
fn reply_before_due_probe_cannot_renew_probe_budget() {
    let mut policy = LivenessPolicy::new(LivenessMode::Listener, 0, 7);
    policy.receive(0, &Control::Probe { nonce: 100 });
    let first = send(&mut policy, 0);
    policy.sent(1, first, true);
    policy.receive(29_000, &Control::Probe { nonce: 101 });
    let response = send(&mut policy, 29_000);
    assert_eq!(response.deadline_ms, 59_000);
    policy.sent(58_000, response, true);
    let probe = send(&mut policy, 58_000);
    assert_eq!(probe.deadline_ms, 60_000);
    assert_eq!(policy.poll(60_000), LivenessAction::Retire);
}

#[test]
fn unsubmitted_probe_does_not_accept_a_guessed_reply() {
    let mut policy = LivenessPolicy::new(LivenessMode::Initiator, 0, 7);
    policy.receive(0, &reply(7));
    let write = send(&mut policy, 0);
    policy.sent(1, write, true);
    assert_eq!(policy.poll(30_000), LivenessAction::Retire);
}

#[test]
fn nonce_exhaustion_never_reuses_a_token() {
    let mut policy = LivenessPolicy::new(LivenessMode::Initiator, 0, u64::MAX);
    let write = send(&mut policy, 0);
    assert_eq!(write.control(), Control::Probe { nonce: u64::MAX });
    policy.sent(1, write, true);
    policy.receive(2, &reply(u64::MAX));
    assert_eq!(policy.poll(30_002), LivenessAction::Retire);
}

#[test]
fn active_clock_regression_and_deadline_overflow_retire() {
    let mut policy = LivenessPolicy::new(LivenessMode::Initiator, 100, 7);
    send(&mut policy, 100);
    assert_eq!(policy.poll(99), LivenessAction::Retire);
    let mut overflow = LivenessPolicy::new(LivenessMode::Initiator, u64::MAX - 10, 7);
    assert_eq!(overflow.poll(u64::MAX - 10), LivenessAction::Retire);
    let mut listener = LivenessPolicy::new(LivenessMode::Listener, u64::MAX - 10, 7);
    listener.receive(u64::MAX - 10, &Control::Probe { nonce: 42 });
    assert_eq!(listener.poll(u64::MAX - 10), LivenessAction::Retire);
}

#[test]
fn retained_policy_has_small_fixed_storage() {
    assert!(core::mem::size_of::<LivenessPolicy>() <= 96);
}
