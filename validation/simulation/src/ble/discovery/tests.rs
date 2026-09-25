use std::error::Error;

use super::*;
use crate::ble::{BleMediumConfig, VirtualBleMedium};
use crate::TopologyConfig;

fn peers() -> Result<[BleDiscoveredPeer; 3], Box<dyn Error>> {
    let medium = VirtualBleMedium::new(BleMediumConfig::new(
        TopologyConfig::FullyConnected,
        3,
        1,
        1,
        1,
    )?);
    let attach = |ordinal| -> Result<_, Box<dyn Error>> {
        let address = BleAddress::new([ordinal; 6]);
        Ok(BleDiscoveredPeer {
            address,
            radio: medium.attach(address, -40)?,
        })
    };
    Ok([attach(1)?, attach(2)?, attach(3)?])
}

#[test]
fn only_observations_refresh_recency_and_capacity_evicts_the_oldest() -> Result<(), Box<dyn Error>>
{
    let [first, second, third] = peers()?;
    let capacity = NonZeroUsize::new(2).unwrap_or_else(|| unreachable!());
    let mut cache = DiscoveryCache::new(capacity);
    assert_eq!(
        cache.snapshot(),
        BleDiscoverySnapshot {
            capacity,
            peers: vec![],
            evicted_peers: 0,
        }
    );
    cache.observe(first);
    cache.observe(second);
    cache.observe(first);
    assert_eq!(cache.radio_for(second.address), Some(second.radio));
    assert_eq!(cache.radio_for(third.address), None);
    assert_eq!(
        cache.snapshot(),
        BleDiscoverySnapshot {
            capacity,
            peers: vec![second, first],
            evicted_peers: 0,
        }
    );
    cache.observe(third);
    assert_eq!(cache.radio_for(second.address), None);
    assert_eq!(
        cache.snapshot(),
        BleDiscoverySnapshot {
            capacity,
            peers: vec![first, third],
            evicted_peers: 1,
        }
    );
    cache.clear();
    cache.clear();
    assert_eq!(cache.radio_for(first.address), None);
    assert_eq!(
        cache.snapshot(),
        BleDiscoverySnapshot {
            capacity,
            peers: vec![],
            evicted_peers: 1,
        }
    );
    Ok(())
}

#[test]
fn singleton_cache_replaces_incarnations_without_counting_refresh_as_eviction(
) -> Result<(), Box<dyn Error>> {
    let [first, second, _] = peers()?;
    let replacement = BleDiscoveredPeer {
        address: first.address,
        radio: second.radio,
    };
    let mut cache = DiscoveryCache::new(NonZeroUsize::MIN);
    for peer in [first, first, replacement, replacement] {
        cache.observe(peer);
        assert_eq!(
            cache.snapshot(),
            BleDiscoverySnapshot {
                capacity: NonZeroUsize::MIN,
                peers: vec![peer],
                evicted_peers: 0,
            }
        );
    }
    cache.observe(second);
    assert_eq!(
        cache.snapshot(),
        BleDiscoverySnapshot {
            capacity: NonZeroUsize::MIN,
            peers: vec![second],
            evicted_peers: 1,
        }
    );
    cache.evicted_peers = u64::MAX;
    cache.observe(first);
    assert_eq!(
        cache.snapshot(),
        BleDiscoverySnapshot {
            capacity: NonZeroUsize::MIN,
            peers: vec![first],
            evicted_peers: u64::MAX,
        }
    );
    Ok(())
}
