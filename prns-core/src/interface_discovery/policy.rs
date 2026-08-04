use alloc::vec::Vec;
use core::num::NonZeroUsize;

use crate::identity::IdentityHash;
use crate::interfaces::InterfaceGravity;
use crate::units::{DurationMillis, InstantMillis};

use super::StampCost;

pub const DISCOVERY_UNKNOWN_AFTER: DurationMillis = DurationMillis(24 * 60 * 60 * 1_000);
pub const DISCOVERY_STALE_AFTER: DurationMillis = DurationMillis(3 * 24 * 60 * 60 * 1_000);
pub const DISCOVERY_EXPIRES_AFTER: DurationMillis = DurationMillis(7 * 24 * 60 * 60 * 1_000);

#[derive(Debug, Clone, PartialEq)]
pub enum InterfaceDiscoveryPolicy {
    Disabled,
    Enabled(EnabledDiscoveryPolicy),
}

impl InterfaceDiscoveryPolicy {
    pub fn enabled(
        required_stamp_cost: StampCost,
        sources: DiscoverySourcePolicy,
        auto_connect: AutoConnectPolicy,
        auto_connect_routing: AutoConnectRoutingPolicy,
    ) -> Self {
        Self::Enabled(EnabledDiscoveryPolicy {
            required_stamp_cost,
            sources,
            auto_connect,
            auto_connect_routing,
        })
    }

    pub const fn enabled_policy(&self) -> Option<&EnabledDiscoveryPolicy> {
        match self {
            Self::Disabled => None,
            Self::Enabled(policy) => Some(policy),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct EnabledDiscoveryPolicy {
    required_stamp_cost: StampCost,
    sources: DiscoverySourcePolicy,
    auto_connect: AutoConnectPolicy,
    auto_connect_routing: AutoConnectRoutingPolicy,
}

impl EnabledDiscoveryPolicy {
    pub const fn required_stamp_cost(&self) -> StampCost {
        self.required_stamp_cost
    }

    pub const fn sources(&self) -> &DiscoverySourcePolicy {
        &self.sources
    }

    pub const fn auto_connect(&self) -> &AutoConnectPolicy {
        &self.auto_connect
    }

    pub const fn auto_connect_gravity(&self) -> InterfaceGravity {
        self.auto_connect_routing.gravity
    }

    pub const fn auto_connect_announces_to_internal(&self) -> bool {
        self.auto_connect_routing.announces_to_internal
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoConnectRoutingPolicy {
    pub gravity: InterfaceGravity,
    pub announces_to_internal: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DiscoverySourcePolicy {
    Open,
    Restricted(DiscoverySourceAllowList),
}

#[derive(Debug, Clone, PartialEq)]
pub struct DiscoverySourceAllowList {
    sources: Vec<IdentityHash>,
}

impl DiscoverySourcePolicy {
    pub fn from_sources(sources: Vec<IdentityHash>) -> Self {
        if sources.is_empty() {
            Self::Open
        } else {
            Self::Restricted(DiscoverySourceAllowList { sources })
        }
    }

    pub fn accepts(&self, source: &IdentityHash) -> bool {
        match self {
            Self::Open => true,
            Self::Restricted(sources) => sources.contains(source),
        }
    }

    pub fn allow_list(&self) -> Option<&[IdentityHash]> {
        match self {
            Self::Open => None,
            Self::Restricted(sources) => Some(sources.as_slice()),
        }
    }
}

impl DiscoverySourceAllowList {
    pub fn contains(&self, source: &IdentityHash) -> bool {
        self.sources.contains(source)
    }

    pub fn as_slice(&self) -> &[IdentityHash] {
        &self.sources
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum AutoConnectPolicy {
    Disabled,
    Limited(NonZeroUsize),
}

impl AutoConnectPolicy {
    pub const fn from_maximum(maximum: usize) -> Self {
        match NonZeroUsize::new(maximum) {
            Some(maximum) => Self::Limited(maximum),
            None => Self::Disabled,
        }
    }

    pub const fn maximum(&self) -> Option<usize> {
        match self {
            Self::Disabled => None,
            Self::Limited(maximum) => Some(maximum.get()),
        }
    }

    pub const fn remaining_slots(&self, connected: usize) -> usize {
        match self.maximum() {
            Some(maximum) => maximum.saturating_sub(connected),
            None => 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveredInterfaceStatus {
    Available,
    Unknown,
    Stale,
    Expired,
}

pub const fn discovered_interface_status(
    last_heard: InstantMillis,
    now: InstantMillis,
) -> DiscoveredInterfaceStatus {
    let age = now.duration_since(last_heard);
    if age.0 > DISCOVERY_EXPIRES_AFTER.0 {
        DiscoveredInterfaceStatus::Expired
    } else if age.0 > DISCOVERY_STALE_AFTER.0 {
        DiscoveredInterfaceStatus::Stale
    } else if age.0 > DISCOVERY_UNKNOWN_AFTER.0 {
        DiscoveredInterfaceStatus::Unknown
    } else {
        DiscoveredInterfaceStatus::Available
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(byte: u8) -> IdentityHash {
        IdentityHash::new([byte; 16])
    }

    #[test]
    fn an_empty_source_list_is_open_and_a_nonempty_list_is_exact() {
        let open = DiscoverySourcePolicy::from_sources(Vec::new());
        assert!(open.accepts(&identity(1)));
        assert_eq!(open.allow_list(), None);

        let restricted = DiscoverySourcePolicy::from_sources(vec![identity(2), identity(3)]);
        assert!(restricted.accepts(&identity(2)));
        assert!(!restricted.accepts(&identity(4)));
        assert_eq!(
            restricted.allow_list(),
            Some([identity(2), identity(3)].as_slice())
        );
    }

    #[test]
    fn an_autoconnect_limit_never_overbooks() {
        let disabled = AutoConnectPolicy::from_maximum(0);
        assert_eq!(disabled.maximum(), None);
        assert_eq!(disabled.remaining_slots(0), 0);

        let limited = AutoConnectPolicy::from_maximum(3);
        assert_eq!(limited.maximum(), Some(3));
        assert_eq!(limited.remaining_slots(1), 2);
        assert_eq!(limited.remaining_slots(3), 0);
        assert_eq!(limited.remaining_slots(9), 0);
    }

    #[test]
    fn record_status_uses_the_reference_threshold_boundaries() {
        let last_heard = InstantMillis(1_000);
        assert!(matches!(
            discovered_interface_status(
                last_heard,
                last_heard.saturating_add(DISCOVERY_UNKNOWN_AFTER),
            ),
            DiscoveredInterfaceStatus::Available,
        ));
        assert!(matches!(
            discovered_interface_status(
                last_heard,
                InstantMillis(last_heard.0 + DISCOVERY_UNKNOWN_AFTER.0 + 1),
            ),
            DiscoveredInterfaceStatus::Unknown,
        ));
        assert!(matches!(
            discovered_interface_status(
                last_heard,
                last_heard.saturating_add(DISCOVERY_STALE_AFTER),
            ),
            DiscoveredInterfaceStatus::Unknown,
        ));
        assert!(matches!(
            discovered_interface_status(
                last_heard,
                InstantMillis(last_heard.0 + DISCOVERY_STALE_AFTER.0 + 1),
            ),
            DiscoveredInterfaceStatus::Stale,
        ));
        assert!(matches!(
            discovered_interface_status(
                last_heard,
                last_heard.saturating_add(DISCOVERY_EXPIRES_AFTER),
            ),
            DiscoveredInterfaceStatus::Stale,
        ));
        assert!(matches!(
            discovered_interface_status(
                last_heard,
                InstantMillis(last_heard.0 + DISCOVERY_EXPIRES_AFTER.0 + 1),
            ),
            DiscoveredInterfaceStatus::Expired,
        ));
        assert!(matches!(
            discovered_interface_status(InstantMillis(5_000), InstantMillis(4_000)),
            DiscoveredInterfaceStatus::Available,
        ));
    }
}
