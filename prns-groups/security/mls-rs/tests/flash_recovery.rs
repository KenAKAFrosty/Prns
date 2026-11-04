#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "support/nor.rs"]
mod nor;

use nor::{Cut, Peers, TestResult, GROUP};
use prns_groups_mls::{Admission, Event};
use prns_groups_store::{OperationId, RecoveryOutcome, Revision};
use std::cell::Cell;

const CUTS: [Cut; 3] = [Cut::TornBody, Cut::BeforeMarker, Cut::AfterMarker];
const BODY: &[u8] = b"real MLS and NOR publication recovery";
fn operation(id: u8) -> OperationId {
    OperationId([id; 32])
}

#[test]
fn nor_send_recovery_reopens_exact_outbox_and_delivers_once() -> TestResult {
    for cut in CUTS {
        let mut peers = Peers::joined()?;
        let before = peers.owner.adapter().coordinator().head()?;
        peers.owner.controller.arm(cut);
        assert!(
            peers
                .owner
                .adapter()
                .send(operation(3), GROUP, BODY)
                .is_err(),
            "{cut:?}"
        );
        assert!(peers.owner.controller.fired());
        assert!(peers.owner.adapter().coordinator().head().is_err());
        let physical = peers.owner.controller.bytes();
        peers.owner.reopen()?;
        let recovered = peers.owner.adapter().coordinator().recover(operation(3))?;
        assert_eq!(
            peers.owner.controller.bytes(),
            physical,
            "recovery must not rewrite flash"
        );
        let output = match cut {
            Cut::AfterMarker => {
                assert!(matches!(recovered, RecoveryOutcome::Committed(_)));
                assert_eq!(peers.owner.last_mount.retired_slots, 0);
                peers.owner.adapter().retry(operation(3))?.unwrap()
            }
            Cut::TornBody | Cut::BeforeMarker => {
                assert_eq!(
                    recovered,
                    RecoveryOutcome::NotCommitted { revision: before }
                );
                assert_eq!(peers.owner.last_mount.retired_slots, 1);
                assert!(peers.owner.adapter().retry(operation(3))?.is_none());
                peers
                    .owner
                    .adapter()
                    .send(operation(3), GROUP, BODY)?
                    .output
            }
        };
        assert!(matches!(output.event, Event::Sent));
        assert_eq!(output.messages.len(), 1);
        assert_eq!(
            peers.owner.adapter().coordinator().head()?,
            Revision(before.0 + 1)
        );
        // A second complete reopen must return the stored ciphertext, without
        // invoking MLS encryption or incrementing its generation again.
        let physical = peers.owner.controller.bytes();
        peers.owner.reopen()?;
        let retry = peers.owner.adapter().retry(operation(3))?.unwrap();
        assert_eq!(retry.messages, output.messages);
        assert_eq!(peers.owner.controller.bytes(), physical);
        assert!(peers
            .owner
            .adapter()
            .send(operation(3), GROUP, b"reused ID")
            .is_err());
        assert_eq!(
            peers.owner.adapter().coordinator().head()?,
            Revision(before.0 + 1)
        );

        let received =
            peers
                .member
                .adapter()
                .receive(operation(3), GROUP, &retry.messages[0], |_| {
                    Admission::Accept
                })?;
        assert!(matches!(&received.output.event, Event::Application(body) if body == BODY));
        let revision = peers.member.adapter().coordinator().head()?;
        peers.member.reopen()?;
        let replay_callbacks = Cell::new(0);
        assert!(peers
            .member
            .adapter()
            .receive(operation(4), GROUP, &retry.messages[0], |_| {
                replay_callbacks.set(replay_callbacks.get() + 1);
                Admission::Accept
            })
            .is_err());
        assert_eq!(replay_callbacks.get(), 0);
        assert_eq!(peers.member.adapter().coordinator().head()?, revision);
        assert!(
            matches!(peers.member.adapter().retry(operation(3))?.unwrap().event, Event::Application(ref body) if body == BODY)
        );
    }
    Ok(())
}

#[test]
fn nor_receive_recovery_commits_inbox_and_consumed_ratchet_together() -> TestResult {
    for cut in CUTS {
        let mut peers = Peers::joined()?;
        let sent = peers.owner.adapter().send(operation(3), GROUP, BODY)?;
        let wire = &sent.output.messages[0];
        let before = peers.member.adapter().coordinator().head()?;
        let callbacks = Cell::new(0);
        peers.member.controller.arm(cut);
        assert!(peers
            .member
            .adapter()
            .receive(operation(3), GROUP, wire, |_| {
                callbacks.set(callbacks.get() + 1);
                Admission::Accept
            })
            .is_err());
        assert!(peers.member.controller.fired());
        assert_eq!(callbacks.get(), 1);
        assert!(peers.member.adapter().coordinator().head().is_err());
        let physical = peers.member.controller.bytes();
        peers.member.reopen()?;
        let recovered = peers.member.adapter().coordinator().recover(operation(3))?;
        assert_eq!(peers.member.controller.bytes(), physical);
        let inbox = match cut {
            Cut::AfterMarker => {
                assert!(matches!(recovered, RecoveryOutcome::Committed(_)));
                assert_eq!(peers.member.last_mount.retired_slots, 0);
                peers.member.adapter().retry(operation(3))?.unwrap()
            }
            Cut::TornBody | Cut::BeforeMarker => {
                assert_eq!(
                    recovered,
                    RecoveryOutcome::NotCommitted { revision: before }
                );
                assert_eq!(peers.member.last_mount.retired_slots, 1);
                assert!(peers.member.adapter().retry(operation(3))?.is_none());
                // Admission can run again after a definite non-commit. It must
                // not perform external effects requiring exactly-once delivery.
                peers
                    .member
                    .adapter()
                    .receive(operation(3), GROUP, wire, |_| {
                        callbacks.set(callbacks.get() + 1);
                        Admission::Accept
                    })?
                    .output
            }
        };
        assert!(matches!(&inbox.event, Event::Application(body) if body == BODY));
        assert_eq!(
            peers.member.adapter().coordinator().head()?,
            Revision(before.0 + 1)
        );
        let expected_callbacks = if cut == Cut::AfterMarker { 1 } else { 2 };
        assert_eq!(callbacks.get(), expected_callbacks);
        peers.member.reopen()?;
        assert!(peers
            .member
            .adapter()
            .receive(operation(4), GROUP, wire, |_| {
                callbacks.set(callbacks.get() + 1);
                Admission::Accept
            })
            .is_err());
        assert_eq!(callbacks.get(), expected_callbacks);
        assert_eq!(
            peers.member.adapter().coordinator().head()?,
            Revision(before.0 + 1)
        );
        let durable = peers.member.adapter().retry(operation(3))?.unwrap();
        assert_eq!(durable.sender, inbox.sender);
        assert_eq!(durable.context, inbox.context);
        assert!(matches!(&durable.event, Event::Application(body) if body == BODY));
        // The recovered participant can keep using the same MLS group in both
        // directions; recovery did not merely restore an isolated receipt.
        let reply = peers.member.adapter().send(operation(5), GROUP, BODY)?;
        let received = peers.owner.adapter().receive(
            operation(5),
            GROUP,
            &reply.output.messages[0],
            |_| Admission::Accept,
        )?;
        assert!(matches!(&received.output.event, Event::Application(body) if body == BODY));
    }
    Ok(())
}
