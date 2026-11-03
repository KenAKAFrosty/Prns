//! LXMF-free first contact over the ordinary production owner, group API and
//! real authenticated Reticulum Links. Simultaneous initiation deliberately
//! creates two ordinary groups: this fixture does not invent a pair protocol.
#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[allow(dead_code)]
#[path = "../transport/reticulum/tests/support/wire.rs"]
mod wire;

use prns_groups::{
    announce_control,
    enrollment::InvitationState,
    owned::{LocalOptions, LocalOwner},
    run_multi_scoped_with_announcements, Clock, GroupLifecycle, IdentityHash, Intent,
    PublicationIntent, SyncPolicy, SystemClock,
};
use prns_groups_core::{Endpoint, EndpointId, EndpointIncarnation};
use std::{
    num::NonZeroUsize, os::unix::fs::PermissionsExt, sync::atomic::Ordering, time::Duration,
};
use tokio::sync::watch;

fn endpoint(peer: usize) -> Endpoint {
    Endpoint {
        id: EndpointId([(peer + 1) as u8; 32]),
        incarnation: EndpointIncarnation::new(1).unwrap(),
    }
}
fn options() -> LocalOptions {
    LocalOptions::new(NonZeroUsize::new(32).unwrap(), 1024 * 1024).with_archive(
        prns_groups::archive::Limits {
            max_records: 256,
            max_bytes: 8 * 1024 * 1024,
        },
    )
}
#[path = "support/journey.rs"]
mod journey;
use journey::*;

fn recipient_from_card(peer: u8, name: &str) -> IdentityHash {
    let identity = personal_rns::identity::PrivateIdentityMaterial::from(
        personal_rns::identity::vault::IdentitySecretKey::new([peer + 0x60; 64]),
    );
    let mut bytes = [0; prns_contact_card::MAX_CARD_BYTES];
    let length = prns_contact_card::encode_identity(
        &identity,
        &prns_groups_reticulum::contact_card::enrollment_service_name_hash(),
        name,
        &mut bytes,
    )
    .unwrap();
    let mut uri = [0; prns_contact_card::MAX_URI_BYTES];
    let uri_len = prns_contact_card::encode_uri(&bytes[..length], &mut uri).unwrap();
    let mut decoded = [0; prns_contact_card::MAX_CARD_BYTES];
    let decoded_len =
        prns_contact_card::decode_uri(std::str::from_utf8(&uri[..uri_len]).unwrap(), &mut decoded)
            .unwrap();
    let verified = prns_contact_card::verify(&decoded[..decoded_len]).unwrap();
    let recipient = prns_groups_reticulum::contact_card::enrollment_recipient(&verified).unwrap();
    assert_eq!(recipient.as_bytes(), identity.identity_hash().as_bytes());
    assert_eq!(
        verified.destination_hash(),
        prns_groups_reticulum::enrollment_destination_for(recipient)
            .unwrap()
            .as_bytes()
    );
    bytes[84] ^= 1;
    assert!(prns_contact_card::verify(&bytes[..length]).is_err());
    recipient
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn signed_cards_consent_simultaneous_groups_offline_records_and_local_deletion() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut nodes = wire::Nodes::open_scoped().await;
    let identities = [
        recipient_from_card(0, "Alice"),
        recipient_from_card(1, "Bob"),
    ];
    let paths = [
        directory.path().join("alice.redb"),
        directory.path().join("bob.redb"),
    ];
    let owners: Vec<_> = paths
        .iter()
        .zip(identities)
        .map(|(path, id)| LocalOwner::create_new(path, id, options()).unwrap())
        .collect();
    let clients: Vec<_> = owners.iter().map(LocalOwner::client).collect();
    let (alice, bob) = (&clients[0], &clients[1]);
    for (i, client) in clients.iter().enumerate() {
        assert_eq!(identities[i], nodes.scopes[i].identity_hash());
        let page = client.conversations(None, QUERY_PAGE).await.unwrap();
        assert!(page.items.is_empty() && page.next.is_none());
        let page = client.invitations(now(), None, QUERY_PAGE).await.unwrap();
        assert!(page.items.is_empty() && page.next.is_none());
    }
    // Cards supply signed service claims, not membership or preinstalled paths.
    let frames = || {
        nodes
            .state
            .frames
            .iter()
            .map(|n| n.load(Ordering::SeqCst))
            .sum::<usize>()
    };
    let initial_frames = frames();
    let mut stops = Vec::new();
    let mut tasks = Vec::new();
    for (i, client) in clients.iter().enumerate() {
        let (stop, receiver) = watch::channel(false);
        stops.push(stop);
        let (_, announcements) = announce_control(None).unwrap();
        tasks.push(tokio::spawn(run_multi_scoped_with_announcements(
            client.clone(),
            nodes.scopes[i].clone(),
            prns_groups_reticulum::destination_for(identities[i]).unwrap(),
            prns_groups_reticulum::enrollment_destination_for(identities[i]).unwrap(),
            nodes.jobs[i].take().unwrap(),
            nodes.enrollment_jobs[i].take().unwrap(),
            wire::limits(),
            wire::enrollment_limits(),
            SyncPolicy::default(),
            announcements,
            receiver,
        )));
    }
    // Product policy for simultaneous initiation: preserve two explicit groups.
    let (group_a, group_b) = tokio::join!(
        create(alice, "Alice initiated"),
        create(bob, "Bob initiated")
    );
    assert_ne!(group_a, group_b);
    let (offer_a, offer_b) = tokio::join!(
        invite(alice, group_a, identities[1]),
        invite(bob, group_b, identities[0])
    );
    for (recipient, offer) in [(bob, offer_a), (alice, offer_b)] {
        eventually(
            "signed invitation reaches its intended recipient",
            || async {
                let page = recipient
                    .invitations(now(), None, QUERY_PAGE)
                    .await
                    .unwrap();
                assert!(
                    page.next.is_none(),
                    "the fixture invitation catalog fits this page"
                );
                page.items
                    .iter()
                    .any(|item| item.id == offer && item.state == InvitationState::Pending)
            },
        )
        .await;
        let page = recipient.conversations(None, QUERY_PAGE).await.unwrap();
        assert!(page.next.is_none());
        assert_eq!(page.items.len(), 1, "invitation alone grants no membership");
    }
    tokio::join!(
        execute(
            bob,
            Intent::Respond {
                invitation: offer_a,
                accept: true
            }
        ),
        execute(
            alice,
            Intent::Respond {
                invitation: offer_b,
                accept: true
            }
        ),
    );
    tokio::join!(
        members(alice, group_a),
        members(bob, group_a),
        members(alice, group_b),
        members(bob, group_b)
    );
    let bootstrap_frames = frames() - initial_frames;
    for (client, identity) in [(alice, identities[0]), (bob, identities[1])] {
        let page = client.conversations(None, QUERY_PAGE).await.unwrap();
        assert!(page.next.is_none());
        let groups = page.items;
        assert_eq!(groups.len(), 2);
        for group in groups {
            assert_eq!(
                group.capabilities.can_close,
                group.owner_identity == Some(identity)
            );
            assert_eq!(
                group.capabilities.can_leave,
                group.owner_identity != Some(identity)
            );
        }
    }
    let payload = binary(group_a, 2048);
    let expected = payload.body.clone();
    let generic = publish(alice, payload).await;
    let text = publish(
        bob,
        PublicationIntent::text(group_b, "consented text").unwrap(),
    )
    .await;
    let received = record(bob, group_a, generic, false).await;
    assert_eq!(received.record.body.as_slice(), expected.as_slice());
    assert_eq!(received.author_identity, identities[0]);
    assert!(alice
        .messages(group_a, None, 100, SystemClock.now())
        .await
        .unwrap()
        .messages
        .is_empty());
    record(alice, group_b, text, false).await;
    tokio::join!(
        record(alice, group_a, generic, true),
        record(bob, group_b, text, true)
    );
    assert_eq!(
        alice
            .messages(group_b, None, 100, SystemClock.now())
            .await
            .unwrap()
            .messages[0]
            .text,
        "consented text"
    );
    let exchange_frames = frames() - initial_frames - bootstrap_frames;

    nodes.state.set_online(1, false);
    let offline = publish(alice, binary(group_a, 1024)).await;
    let pending = record(alice, group_a, offline, false).await;
    assert_eq!(pending.progress.summary.pending, 1);
    nodes.state.set_online(1, true);
    tokio::join!(
        record(bob, group_a, offline, false),
        record(alice, group_a, offline, true)
    );

    let roster = alice.members(group_a, None, QUERY_PAGE).await.unwrap();
    assert!(roster.next.is_none());
    let member = roster
        .members
        .into_iter()
        .find(|m| m.identity == identities[1])
        .unwrap()
        .endpoint;
    execute(
        alice,
        Intent::RemoveMember {
            group: group_a,
            member,
        },
    )
    .await;
    lifecycle(bob, group_a, GroupLifecycle::Removed).await;
    assert_eq!(
        bob.records(group_a, None, 100, SystemClock.now())
            .await
            .unwrap()
            .records
            .len(),
        2
    );
    execute(bob, Intent::DeleteLocal { group: group_a }).await;
    assert_eq!(
        alice
            .records(group_a, None, 100, SystemClock.now())
            .await
            .unwrap()
            .records
            .len(),
        2
    );
    execute(alice, Intent::Close { group: group_a }).await;
    execute(alice, Intent::DeleteLocal { group: group_a }).await;
    execute(bob, Intent::Close { group: group_b }).await;
    lifecycle(alice, group_b, GroupLifecycle::Closed).await;

    for stop in stops {
        stop.send(true).unwrap();
    }
    for task in tasks {
        tokio::time::timeout(Duration::from_secs(15), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
    for owner in &owners {
        owner.close().await.unwrap();
    }
    drop(owners);
    // Retained history survives close/reopen until each local user deletes it.
    for (path, identity) in paths.iter().zip(identities) {
        let owner = LocalOwner::open_existing(path, identity, options()).unwrap();
        let client = owner.client();
        let page = client.conversations(None, QUERY_PAGE).await.unwrap();
        assert!(page.next.is_none());
        let groups = page.items;
        assert_eq!(
            groups.len(),
            1,
            "deleted group is not resurrected by late notices"
        );
        assert_eq!(groups[0].group, group_b);
        assert_eq!(groups[0].lifecycle, GroupLifecycle::Closed);
        assert_eq!(
            client
                .messages(group_b, None, 100, SystemClock.now())
                .await
                .unwrap()
                .messages[0]
                .text,
            "consented text"
        );
        execute(&client, Intent::DeleteLocal { group: group_b }).await;
        let page = client.conversations(None, QUERY_PAGE).await.unwrap();
        assert!(page.items.is_empty() && page.next.is_none());
        owner.close().await.unwrap();
    }
    let resources = nodes.state.resources.load(Ordering::SeqCst);
    let proofs = nodes.state.resource_proofs.load(Ordering::SeqCst);
    assert!(
        resources > 0 && proofs > 0,
        "real Resource transfer and proofs were exercised"
    );
    eprintln!("group-only fixture: bootstrap_frames={bootstrap_frames}, exchange_frames={exchange_frames}, total_frames={}, resources={resources}, resource_proofs={proofs}, database_bytes={:?}", frames(), paths.iter().map(|p| std::fs::metadata(p).unwrap().len()).collect::<Vec<_>>());
    nodes.stop().await;
}

#[path = "support/member_scale.rs"]
mod member_scale;
