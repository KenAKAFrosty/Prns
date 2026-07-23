use tokio::sync::mpsc;

use prns_core::interfaces::i2p;
use prns_core::interfaces::{BitrateBps, ConfiguredInterfacePolicy, InterfaceMode, MtuPolicy};
use prns_runtime::manifold::interface_seam::Interface;

use super::super::super::{I2pPeerAddress, I2pRetryPolicy};
use super::super::member::{I2pAcceptedPeer, I2pConfiguredPeer};
use crate::i2p::test_support::{public_destination, FakeSamBridge};

#[test]
fn configured_and_accepted_members_inherit_the_complete_effective_policy() {
    let policy = i2p::configured_policy(ConfiguredInterfacePolicy {
        bitrate: Some(BitrateBps::guess(432_100)),
        mode: Some(InterfaceMode::Gateway),
        mtu: Some(MtuPolicy::fixed(987)),
        ..ConfiguredInterfacePolicy::default()
    });
    let bridge = FakeSamBridge::new();
    let peer = I2pPeerAddress::new(public_destination(0x61).as_str())
        .expect("the configured destination is valid");
    let (events, _events_rx) = mpsc::unbounded_channel();
    let configured =
        I2pConfiguredPeer::new(bridge, peer, policy, I2pRetryPolicy::STOCK, events.clone());
    let (stream, _remote) = tokio::io::duplex(1024);
    let accepted = I2pAcceptedPeer::new(
        public_destination(0x62),
        1,
        tokio::io::BufReader::new(stream),
        policy,
        events,
    );

    assert_eq!(configured.descriptor(), policy.descriptor(configured.id()));
    assert_eq!(accepted.descriptor(), policy.descriptor(accepted.id()));
}

#[test]
fn accepted_connections_have_distinct_member_identities() {
    let policy = i2p::configured_policy(ConfiguredInterfacePolicy::default());
    let peer = public_destination(0x68);
    let (events, _events_rx) = mpsc::unbounded_channel();
    let (first_stream, _first_remote) = tokio::io::duplex(1024);
    let (second_stream, _second_remote) = tokio::io::duplex(1024);
    let first = I2pAcceptedPeer::new(
        peer.clone(),
        1,
        tokio::io::BufReader::new(first_stream),
        policy,
        events.clone(),
    );
    let second = I2pAcceptedPeer::new(
        peer,
        2,
        tokio::io::BufReader::new(second_stream),
        policy,
        events,
    );

    assert_ne!(first.id(), second.id());
}
