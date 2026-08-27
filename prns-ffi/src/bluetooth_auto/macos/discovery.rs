use std::collections::HashMap;
use std::time::{Duration, Instant};

use objc2::runtime::AnyObject;
use objc2_core_bluetooth::{
    CBAdvertisementDataLocalNameKey, CBAdvertisementDataManufacturerDataKey, CBPeripheralState,
};
use objc2_foundation::{NSData, NSDictionary, NSString};

use prns_core::interfaces::bluetooth_auto::{
    columba_role_capabilities_from_manufacturer, manufacturer_discovery_groups_match,
    GROUP_TAG_LEN,
};

use super::CoreBluetoothPeerId;

const WEAK_CANDIDATE_SUPPRESSION_TTL: Duration = Duration::from_secs(5 * 60);
// CoreBluetooth can spend several seconds reporting `Connected` after accepting a cancellation.
// Treat this as an in-flight marker until didDisconnect clears it; the deadline is only a bounded
// fallback for a missing callback.
const STALE_CANCELLATION_FALLBACK_TTL: Duration = Duration::from_secs(30);
const DISCOVERY_GUARD_CAPACITY: usize = 256;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum CandidateStrength {
    Strong,
    Weak,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum PeripheralLinkState {
    Disconnected,
    Connecting,
    Connected,
    Disconnecting,
    Unknown,
}

impl From<CBPeripheralState> for PeripheralLinkState {
    fn from(state: CBPeripheralState) -> Self {
        if state == CBPeripheralState::Disconnected {
            Self::Disconnected
        } else if state == CBPeripheralState::Connecting {
            Self::Connecting
        } else if state == CBPeripheralState::Connected {
            Self::Connected
        } else if state == CBPeripheralState::Disconnecting {
            Self::Disconnecting
        } else {
            Self::Unknown
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum DiscoverDisposition {
    Adopt,
    IgnoreOwned,
    WaitForDisconnect,
    CancelStale,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SessionPresence {
    Absent,
    Present,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum StaleCancellation {
    Idle,
    InFlight,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum StaleLinkRecovery {
    Disabled,
    Enabled,
}

pub(super) const fn discover_disposition(
    state: PeripheralLinkState,
    session: SessionPresence,
    cancellation: StaleCancellation,
    recovery: StaleLinkRecovery,
) -> DiscoverDisposition {
    match (state, session, cancellation, recovery) {
        (PeripheralLinkState::Disconnected, _, _, _) => DiscoverDisposition::Adopt,
        (
            PeripheralLinkState::Connecting | PeripheralLinkState::Connected,
            SessionPresence::Present,
            _,
            _,
        ) => DiscoverDisposition::IgnoreOwned,
        (
            PeripheralLinkState::Connecting | PeripheralLinkState::Connected,
            SessionPresence::Absent,
            StaleCancellation::Idle,
            StaleLinkRecovery::Enabled,
        ) => DiscoverDisposition::CancelStale,
        (
            PeripheralLinkState::Connecting
            | PeripheralLinkState::Connected
            | PeripheralLinkState::Disconnecting
            | PeripheralLinkState::Unknown,
            _,
            _,
            _,
        ) => DiscoverDisposition::WaitForDisconnect,
    }
}

pub(super) fn candidate_strength(
    local_name_is_prns: bool,
    manufacturer_data: Option<&[u8]>,
    local_group_tag: [u8; GROUP_TAG_LEN],
) -> CandidateStrength {
    if let Some(data) = manufacturer_data {
        let Some(company_id) = data.get(..2).and_then(|bytes| bytes.try_into().ok()) else {
            return CandidateStrength::Weak;
        };
        let company_id = u16::from_le_bytes(company_id);
        let body = &data[2..];
        if !manufacturer_discovery_groups_match(local_group_tag, company_id, body) {
            return CandidateStrength::Weak;
        }
        if columba_role_capabilities_from_manufacturer(company_id, body).is_some() {
            return CandidateStrength::Strong;
        }
    }
    if local_name_is_prns {
        CandidateStrength::Strong
    } else {
        CandidateStrength::Weak
    }
}

pub(super) fn advertisement_candidate_strength(
    advertisement_data: &NSDictionary<NSString, AnyObject>,
    local_group_tag: [u8; GROUP_TAG_LEN],
) -> CandidateStrength {
    // SAFETY: CoreBluetooth exports this dictionary key with process lifetime.
    let local_name_key = unsafe { CBAdvertisementDataLocalNameKey };
    let local_name_is_prns = advertisement_data
        .objectForKey(local_name_key)
        .is_some_and(|name| {
            name.downcast_ref::<NSString>()
                .is_some_and(|name| name.isEqualToString(&NSString::from_str("Prns")))
        });
    // SAFETY: CoreBluetooth exports this dictionary key with process lifetime.
    let manufacturer_data_key = unsafe { CBAdvertisementDataManufacturerDataKey };
    let manufacturer_data = advertisement_data
        .objectForKey(manufacturer_data_key)
        .and_then(|data| data.downcast_ref::<NSData>().map(NSData::to_vec));
    candidate_strength(local_name_is_prns, manufacturer_data.as_deref(), local_group_tag)
}

#[derive(Default)]
pub(super) struct DiscoveryGuard {
    weak_candidate_misses: HashMap<CoreBluetoothPeerId, Instant>,
    stale_cancellations: HashMap<CoreBluetoothPeerId, Instant>,
}

impl DiscoveryGuard {
    pub(super) fn admit_candidate(
        &mut self,
        peer_id: CoreBluetoothPeerId,
        strength: CandidateStrength,
        now: Instant,
    ) -> bool {
        prune_expired(&mut self.weak_candidate_misses, now);
        if strength == CandidateStrength::Strong {
            self.weak_candidate_misses.remove(&peer_id);
            true
        } else {
            !self.weak_candidate_misses.contains_key(&peer_id)
        }
    }

    pub(super) fn record_service_miss(&mut self, peer_id: CoreBluetoothPeerId, now: Instant) {
        insert_bounded_deadline(
            &mut self.weak_candidate_misses,
            peer_id,
            now + WEAK_CANDIDATE_SUPPRESSION_TTL,
        );
    }

    pub(super) fn cancellation_recent(
        &mut self,
        peer_id: CoreBluetoothPeerId,
        now: Instant,
    ) -> bool {
        prune_expired(&mut self.stale_cancellations, now);
        self.stale_cancellations.contains_key(&peer_id)
    }

    pub(super) fn record_stale_cancellation(&mut self, peer_id: CoreBluetoothPeerId, now: Instant) {
        insert_bounded_deadline(
            &mut self.stale_cancellations,
            peer_id,
            now + STALE_CANCELLATION_FALLBACK_TTL,
        );
    }

    pub(super) fn clear_stale_cancellation(&mut self, peer_id: CoreBluetoothPeerId) {
        self.stale_cancellations.remove(&peer_id);
    }

    #[cfg(test)]
    pub(super) fn suppressed_len(&self) -> usize {
        self.weak_candidate_misses.len()
    }
}

fn prune_expired(entries: &mut HashMap<CoreBluetoothPeerId, Instant>, now: Instant) {
    entries.retain(|_, deadline| *deadline > now);
}

fn insert_bounded_deadline(
    entries: &mut HashMap<CoreBluetoothPeerId, Instant>,
    peer_id: CoreBluetoothPeerId,
    deadline: Instant,
) {
    if !entries.contains_key(&peer_id) && entries.len() >= DISCOVERY_GUARD_CAPACITY {
        if let Some(oldest) = entries
            .iter()
            .min_by_key(|(_, deadline)| **deadline)
            .map(|(peer_id, _)| *peer_id)
        {
            entries.remove(&oldest);
        }
    }
    entries.insert(peer_id, deadline);
}
