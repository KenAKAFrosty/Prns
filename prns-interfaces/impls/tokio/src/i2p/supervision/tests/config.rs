use std::time::Duration;

use super::super::super::{
    I2pBase32Address, I2pInterfaceName, I2pPeerAddress, I2pPeers, I2pRetryPolicy,
    I2pRetryPolicyError,
};
use crate::i2p::test_support::public_destination;

#[test]
fn names_and_destinations_remain_distinct_peer_types() {
    let named = I2pPeerAddress::new("example.i2p").expect("the I2P name is valid");
    let destination = I2pPeerAddress::new(public_destination(0x11).as_str())
        .expect("the I2P destination is valid");

    assert!(matches!(named, I2pPeerAddress::Named(_)));
    assert!(matches!(destination, I2pPeerAddress::Destination(_)));
    assert!(I2pPeerAddress::new("EXAMPLE.I2P").is_err());
}

#[test]
fn duplicate_peers_are_rejected_before_runtime() {
    let peer = I2pPeerAddress::new("example.i2p").expect("the I2P name is valid");
    let error = I2pPeers::new([peer.clone(), peer]).expect_err("duplicates are rejected");

    assert_eq!(error.peer.as_str(), "example.i2p");
}

#[test]
fn retry_intervals_are_non_zero_by_construction() {
    let one = Duration::from_secs(1);

    assert_eq!(
        I2pRetryPolicy::new(Duration::ZERO, one, one),
        Err(I2pRetryPolicyError::TunnelSetupZero)
    );
    assert_eq!(
        I2pRetryPolicy::new(one, Duration::ZERO, one),
        Err(I2pRetryPolicyError::PeerReconnectZero)
    );
    assert_eq!(
        I2pRetryPolicy::new(one, one, Duration::ZERO),
        Err(I2pRetryPolicyError::EndpointRetryZero)
    );
}

#[test]
fn stock_retry_intervals_match_rns_1_4_0() {
    assert_eq!(
        I2pRetryPolicy::STOCK.tunnel_setup_interval(),
        Duration::from_secs(8)
    );
    assert_eq!(
        I2pRetryPolicy::STOCK.peer_reconnect_interval(),
        Duration::from_secs(15)
    );
    assert_eq!(
        I2pRetryPolicy::STOCK.endpoint_retry_interval(),
        Duration::from_secs(15)
    );
}

#[test]
fn interface_names_cannot_be_empty() {
    assert!(I2pInterfaceName::new("").is_err());
    assert_eq!(
        I2pInterfaceName::new("Private I2P").map(|name| name.as_str().to_owned()),
        Ok(String::from("Private I2P"))
    );
}

#[test]
fn published_base32_addresses_are_canonical_by_construction() {
    let canonical = format!("{}.b32.i2p", "a".repeat(52));

    assert_eq!(
        I2pBase32Address::new(canonical.clone()).map(|address| address.as_str().to_owned()),
        Ok(canonical)
    );
    assert!(I2pBase32Address::new(format!("{}.b32.i2p", "A".repeat(52))).is_err());
    assert!(I2pBase32Address::new(format!("{}.b32.i2p", "1".repeat(52))).is_err());
    assert!(I2pBase32Address::new(format!("{}.i2p", "a".repeat(52))).is_err());
}
