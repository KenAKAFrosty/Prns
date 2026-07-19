use tokio::sync::mpsc::{self, UnboundedReceiver};

use crate::engine::InstantMillis;
use crate::identity::Zeroizing;
use crate::reactor::driver::{HostCommand, SelfRatchetSnapshot};
use crate::routing::{BlackholeExpiry, BlackholedIdentity};
use crate::runtime::{Manual, PreConfiguredDestination, PrnsNodeRecipe};
use crate::wire::DestinationHash;

use super::super::{PrnsNode, PrnsNodeHandle};
use super::{
    try_zeroed_buffer, wall_clock_timeline_origin, BlackholeSeedReport, MAX_BOOT_RECORD_LEN,
};

fn handle() -> (PrnsNodeHandle, UnboundedReceiver<HostCommand>) {
    let (commands, command_rx) = mpsc::unbounded_channel();
    (PrnsNodeHandle::over(commands), command_rx)
}

#[test]
fn an_oversized_persisted_length_is_rejected_before_allocation() {
    assert!(try_zeroed_buffer(MAX_BOOT_RECORD_LEN + 1).is_none());
    assert!(try_zeroed_buffer(usize::MAX).is_none());
}

#[tokio::test]
async fn one_ratchet_snapshot_command_resolves_one_destination() {
    let (handle, mut command_rx) = handle();
    let destination = DestinationHash::new([0x5A; 16]);
    let snapshotting = tokio::spawn(async move { handle.snapshot_self_ratchet(destination).await });
    let HostCommand::SnapshotSelfRatchet {
        destination: requested,
        reply,
    } = command_rx.recv().await.unwrap()
    else {
        panic!("expected one ratchet snapshot command");
    };
    assert_eq!(requested, destination);
    assert!(reply
        .send(Some(SelfRatchetSnapshot {
            destination,
            sealed: Zeroizing::new(vec![0xA5; 64]),
        }))
        .is_ok());
    let snapshot = snapshotting.await.unwrap().unwrap().unwrap();
    assert_eq!(snapshot.destination, destination);
    assert_eq!(snapshot.sealed.as_slice(), &[0xA5; 64]);
}

#[test]
fn the_standard_timeline_origin_is_unix_epoch_aligned() {
    let wall_now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis();
    let origin = wall_clock_timeline_origin();

    assert!(wall_now.abs_diff(u128::from(origin.0)) < 1_000);
}

#[test]
fn boot_blackholes_seed_against_the_resumed_timeline() {
    let mut prns = PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        pre_configured_destinations: [] as [PreConfiguredDestination<'static>; 0],
        app_state: (),
        storage: crate::storage::GrowableHeap,
        routes: crate::routes![],
        interfaces: Manual,
        on_event: |_event, _state: &()| {},
    })
    .with_timeline_origin(InstantMillis(1_000));
    let identity = crate::identity::IdentityHash::new([0x31; 16]);
    let source = crate::identity::IdentityHash::new([0x41; 16]);

    let report = prns.seed_blackholed_identities([
        BlackholedIdentity {
            identity,
            source,
            expiry: BlackholeExpiry::At(InstantMillis(2_000)),
            reason: Some("active"),
        },
        BlackholedIdentity {
            identity,
            source,
            expiry: BlackholeExpiry::Indefinite,
            reason: Some("duplicate"),
        },
        BlackholedIdentity {
            identity: crate::identity::IdentityHash::new([0x32; 16]),
            source,
            expiry: BlackholeExpiry::At(InstantMillis(999)),
            reason: Some("expired"),
        },
    ]);

    assert_eq!(
        report,
        BlackholeSeedReport {
            seeded_count: 1,
            refused_count: 1,
            dropped_count: 1,
        }
    );
    assert!(prns.node.engine.is_identity_blackholed(&identity));
    assert_eq!(prns.node.engine.blackholed_identity_count(), 1);
}
