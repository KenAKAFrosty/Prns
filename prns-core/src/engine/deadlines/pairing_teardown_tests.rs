use crate::crypto::{Ed25519PublicKey, X25519PublicKey};
use crate::engine::test_support::routable_descriptor;
use crate::engine::{
    Directive, EngineReaction, InstantMillis, Journaled, LinkClosedReason, WakeSchedule,
};
use crate::identity::{
    IdentityEncryptionPublicKey, IdentityHash, IdentityPublicKeys, IdentitySigningPublicKey,
};
use crate::interfaces::AttachedInterfaces;
use crate::remote_control::{
    BeginRemoteControlControllerPairingOutcome, RemoteControlControllerIdentity,
    RemoteControlControllerPairingAborted, RemoteControlControllerPairingView,
    RemoteControlPairingBegin, RemoteControlPairingContext, RemoteControlPairingIdentity,
    RemoteControlPairingInvitationCode,
};
use crate::routing::links::maintenance::{stale_ms_from, timeout_grace_ms_from, write_link_close};
use crate::routing::links::resources::receive::tests_support::{
    engine_with_active_link, lane, link_id, link_key,
};
use crate::routing::links::table::{LinkPhase, RespondingLink};
use crate::routing::links::LinkId;
use crate::routing::upstream_app_destinations::ProofStrategy;
use crate::units::RttMillis;
use crate::wire::BROADCAST_MTU;

#[test]
fn stale_link_teardown_refreshes_only_its_controller_pairing_attempt() {
    let stale_link = link_id();
    let fresh_link = LinkId::new([0xF1; 16]);
    for pairing_link in [stale_link, fresh_link] {
        let mut engine = engine_with_active_link();
        let Some(LinkPhase::Active {
            last_inbound,
            keepalive_ms,
            rtt,
            ..
        }) = engine.links.phase_for(&stale_link)
        else {
            panic!("fixture must have an active link");
        };
        let teardown = InstantMillis(
            last_inbound.0 + stale_ms_from(*keepalive_ms) + timeout_grace_ms_from(*rtt),
        );
        let fresh_at = InstantMillis(teardown.0 - 1_000);
        let expires_at = InstantMillis(teardown.0 + 10_000);
        let endpoint = RemoteControlPairingIdentity::new(IdentityHash::new([0xA1; 16])).endpoint();
        engine
            .links
            .track_responding(RespondingLink {
                link_id: fresh_link,
                key: link_key(),
                requested_at: fresh_at,
                timeout_at: expires_at,
                mtu: BROADCAST_MTU,
                initiator_signing: Ed25519PublicKey([0x99; 32]),
                destination: endpoint.destination_hash(),
                identity: IdentityHash::new([0xA1; 16]),
                proof_strategy: ProofStrategy::ProveNone,
            })
            .unwrap();
        engine
            .links
            .activate_responding(&fresh_link, RttMillis::new(250), lane(), fresh_at)
            .unwrap();
        let controller = RemoteControlControllerIdentity::new(IdentityPublicKeys {
            encryption: IdentityEncryptionPublicKey::new(X25519PublicKey([0xD1; 32])),
            signing: IdentitySigningPublicKey::new(Ed25519PublicKey([0xD2; 32])),
        });
        let context = RemoteControlPairingContext::new(endpoint, pairing_link);
        let invitation = RemoteControlPairingInvitationCode::from_value(0xC3C3_C3C3);
        assert_eq!(
            engine.remote_control_controller_pairing.begin(
                controller,
                context,
                invitation.clone(),
                fresh_at,
                expires_at,
            ),
            BeginRemoteControlControllerPairingOutcome::BeginOwed {
                begin: RemoteControlPairingBegin::new(controller, endpoint, invitation),
                context,
                expires_at,
            },
        );
        let descriptors = [routable_descriptor(lane())];
        let interfaces = AttachedInterfaces::new(&descriptors);
        let mut cached = engine.wake_schedules(interfaces);
        assert_eq!(cached.remote_control_pairing, WakeSchedule::At(expires_at));
        let mut aborted = std::vec::Vec::new();
        let mut closed = std::vec::Vec::new();
        let mut frames = std::vec::Vec::new();
        cached.merge(engine.fire_due_link_deadlines(
            teardown,
            interfaces,
            &mut |bytes| bytes.fill(0x66),
            &mut |reaction| match reaction {
                EngineReaction::Journaled(
                    Journaled::RemoteControlControllerPairingLinkClosed { aborted: attempt },
                ) => aborted.push(attempt),
                EngineReaction::Journaled(Journaled::LinkClosed { link_id, reason }) => {
                    closed.push((link_id, reason))
                }
                EngineReaction::Directive(Directive::Send { target, bytes }) => {
                    frames.push((target, bytes.to_vec()))
                }
                _ => panic!("unexpected pairing teardown reaction"),
            },
        ));
        assert_eq!(cached, engine.wake_schedules(interfaces));
        let view = engine.remote_control_controller_pairing_view();
        if pairing_link == stale_link {
            assert_eq!(
                aborted,
                [RemoteControlControllerPairingAborted::AwaitingOffer { context }]
            );
            assert_eq!(view, RemoteControlControllerPairingView::Idle);
            assert_eq!(cached.remote_control_pairing, WakeSchedule::Idle);
        } else {
            assert!(aborted.is_empty());
            let RemoteControlControllerPairingView::AwaitingOffer(begin) = view else {
                panic!("unrelated attempt must remain intact");
            };
            assert_eq!(
                (
                    begin.context(),
                    *begin.controller(),
                    begin.window().started_at(),
                    begin.window().expires_at()
                ),
                (context, controller, fresh_at, expires_at),
            );
            assert_eq!(cached.remote_control_pairing, WakeSchedule::At(expires_at));
        }
        assert_eq!(closed, [(stale_link, LinkClosedReason::Timeout)]);
        assert!(engine.links.phase_for(&stale_link).is_none());
        assert!(matches!(
            engine.links.phase_for(&fresh_link),
            Some(LinkPhase::Active { .. })
        ));
        let mut expected_close = [0; BROADCAST_MTU];
        let length =
            write_link_close(&stale_link, &link_key(), &[0x66; 16], &mut expected_close).unwrap();
        assert_eq!(frames, [(lane(), expected_close[..length].to_vec())]);

        let mut no_reaction =
            |_: EngineReaction<'_>| panic!("no duplicate closure or premature expiry");
        cached.merge(engine.fire_due_link_deadlines(
            teardown,
            interfaces,
            &mut |_| panic!("no duplicate close entropy"),
            &mut no_reaction,
        ));
        cached.merge(engine.fire_due_remote_control_pairing(
            teardown,
            interfaces,
            &mut |_| panic!("no premature pairing expiry entropy"),
            &mut no_reaction,
        ));
        assert_eq!(cached, engine.wake_schedules(interfaces));
    }
}
