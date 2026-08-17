use core::num::NonZeroU8;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use personal_rns::interfaces::wifi_auto::{
    AdvertisementInsertion, CandidateInsertion, DiscoveryEndpoint, DiscoveryEndpointError,
    DiscoveryServiceName, DiscoveryServiceNameError, DiscoverySnapshot, DiscoveryVersion,
    DiscoveryVersionError, ServiceAdvertisement,
};
use personal_rns::wifi_auto::{
    DiscoveryParticipation, ServiceDiscovery, ServiceDiscoveryPublisher, SnapshotPublication,
};

pub const DISCOVERY_CAPACITY: NonZeroU8 = NonZeroU8::MAX;

struct AndroidServiceDiscoveryShared {
    publisher: ServiceDiscoveryPublisher,
    visible_services: Mutex<DiscoverySnapshot>,
    discovery: Mutex<Option<ServiceDiscovery>>,
}

#[derive(Clone)]
pub struct AndroidServiceDiscoveryBridge {
    shared: Arc<AndroidServiceDiscoveryShared>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TakeServiceDiscoveryError {
    StateUnavailable,
    AlreadyTaken,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ServiceRecordRejection {
    ServiceName(DiscoveryServiceNameError),
    Version(DiscoveryVersionError),
    Endpoint(DiscoveryEndpointError),
    NoCandidates,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ServiceResolutionOutcome {
    SnapshotChanged,
    SnapshotUnchanged,
    RejectedParticipation(DiscoveryParticipation),
    RejectedRecord(ServiceRecordRejection),
    RejectedCandidateCapacity,
    RejectedAdvertisementCapacity,
    CapacityMismatch,
    StateUnavailable,
}

impl ServiceResolutionOutcome {
    #[must_use]
    pub const fn endpoint_is_visible(&self) -> bool {
        matches!(self, Self::SnapshotChanged | Self::SnapshotUnchanged)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum ServiceRemovalOutcome {
    SnapshotChanged,
    SnapshotUnchanged,
    RejectedServiceName(DiscoveryServiceNameError),
    RejectedParticipation(DiscoveryParticipation),
    CapacityMismatch,
    StateUnavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotClearOutcome {
    Cleared,
    StateUnavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SnapshotMutation {
    Changed,
    Unchanged,
    RejectedAdvertisementCapacity,
}

impl AndroidServiceDiscoveryBridge {
    #[must_use]
    pub fn new() -> Self {
        let (service_discovery, service_discovery_publisher) =
            ServiceDiscovery::channel(DISCOVERY_CAPACITY);
        Self {
            shared: Arc::new(AndroidServiceDiscoveryShared {
                publisher: service_discovery_publisher,
                visible_services: Mutex::new(DiscoverySnapshot::new(DISCOVERY_CAPACITY)),
                discovery: Mutex::new(Some(service_discovery)),
            }),
        }
    }

    pub fn resolved(
        &self,
        service_instance: &str,
        socket_addresses: impl IntoIterator<Item = SocketAddr>,
        version: Option<&[u8]>,
    ) -> ServiceResolutionOutcome {
        let current_participation = self.shared.publisher.participation();
        if current_participation != DiscoveryParticipation::Central {
            let _clear_outcome = self.clear_visible_services();
            return ServiceResolutionOutcome::RejectedParticipation(current_participation);
        }

        let discovery_service_name = match DiscoveryServiceName::from_instance(service_instance) {
            Ok(discovery_service_name) => discovery_service_name,
            Err(service_name_error) => {
                return ServiceResolutionOutcome::RejectedRecord(
                    ServiceRecordRejection::ServiceName(service_name_error),
                );
            }
        };
        if let Err(version_error) = DiscoveryVersion::parse(version) {
            let _removal_outcome = self.remove_visible_service(&discovery_service_name);
            return ServiceResolutionOutcome::RejectedRecord(ServiceRecordRejection::Version(
                version_error,
            ));
        }
        let mut service_advertisement = ServiceAdvertisement::new(discovery_service_name.clone());
        let mut latest_endpoint_error = None;
        for socket_address in socket_addresses {
            let discovery_endpoint = match DiscoveryEndpoint::new(socket_address) {
                Ok(discovery_endpoint) => discovery_endpoint,
                Err(endpoint_error) => {
                    latest_endpoint_error = Some(endpoint_error);
                    continue;
                }
            };
            match service_advertisement.insert(discovery_endpoint) {
                CandidateInsertion::Inserted | CandidateInsertion::AlreadyPresent => {}
                CandidateInsertion::AtCapacity => {
                    let _removal_outcome = self.remove_visible_service(&discovery_service_name);
                    return ServiceResolutionOutcome::RejectedCandidateCapacity;
                }
            }
        }
        if service_advertisement.is_empty() {
            let _removal_outcome = self.remove_visible_service(&discovery_service_name);
            return ServiceResolutionOutcome::RejectedRecord(match latest_endpoint_error {
                Some(endpoint_error) => ServiceRecordRejection::Endpoint(endpoint_error),
                None => ServiceRecordRejection::NoCandidates,
            });
        }

        let (snapshot_mutation, updated_snapshot) = {
            let Ok(mut visible_services) = self.shared.visible_services.lock() else {
                return ServiceResolutionOutcome::StateUnavailable;
            };
            let snapshot_mutation =
                apply_service_resolution(&mut visible_services, service_advertisement);
            (snapshot_mutation, visible_services.clone())
        };

        match snapshot_mutation {
            SnapshotMutation::Changed => self.publish_snapshot(updated_snapshot),
            SnapshotMutation::Unchanged => ServiceResolutionOutcome::SnapshotUnchanged,
            SnapshotMutation::RejectedAdvertisementCapacity => {
                ServiceResolutionOutcome::RejectedAdvertisementCapacity
            }
        }
    }

    pub fn lost(&self, service_instance: &str) -> ServiceRemovalOutcome {
        let discovery_service_name = match DiscoveryServiceName::from_instance(service_instance) {
            Ok(discovery_service_name) => discovery_service_name,
            Err(service_name_error) => {
                return ServiceRemovalOutcome::RejectedServiceName(service_name_error);
            }
        };
        self.remove_visible_service(&discovery_service_name)
    }

    pub fn take_service_discovery(&self) -> Result<ServiceDiscovery, TakeServiceDiscoveryError> {
        let mut discovery = self
            .shared
            .discovery
            .lock()
            .map_err(|_state_unavailable| TakeServiceDiscoveryError::StateUnavailable)?;
        discovery
            .take()
            .ok_or(TakeServiceDiscoveryError::AlreadyTaken)
    }

    #[must_use]
    pub fn synchronize_participation(&self) -> DiscoveryParticipation {
        let current_participation = self.shared.publisher.participation();
        if current_participation != DiscoveryParticipation::Central {
            let _clear_outcome = self.clear_visible_services();
        }
        current_participation
    }

    #[must_use]
    pub fn work_generation(&self) -> u64 {
        self.shared.publisher.work_generation()
    }

    #[must_use]
    pub fn wait_for_work(&self, observed_generation: u64, timeout_millis: u64) -> u64 {
        self.shared
            .publisher
            .wait_for_work(observed_generation, timeout_millis)
    }

    pub fn wake_waiters(&self) {
        self.shared.publisher.wake_waiters();
    }

    fn publish_snapshot(&self, discovery_snapshot: DiscoverySnapshot) -> ServiceResolutionOutcome {
        match self.shared.publisher.replace_snapshot(discovery_snapshot) {
            SnapshotPublication::Published => ServiceResolutionOutcome::SnapshotChanged,
            SnapshotPublication::NotCentral(current_participation) => {
                let _clear_outcome = self.clear_visible_services();
                ServiceResolutionOutcome::RejectedParticipation(current_participation)
            }
            SnapshotPublication::CapacityMismatch { .. } => {
                ServiceResolutionOutcome::CapacityMismatch
            }
        }
    }

    fn remove_visible_service(
        &self,
        discovery_service_name: &DiscoveryServiceName,
    ) -> ServiceRemovalOutcome {
        let updated_snapshot = {
            let Ok(mut visible_services) = self.shared.visible_services.lock() else {
                return ServiceRemovalOutcome::StateUnavailable;
            };
            if !visible_services.remove(discovery_service_name) {
                return ServiceRemovalOutcome::SnapshotUnchanged;
            }
            visible_services.clone()
        };
        match self.shared.publisher.replace_snapshot(updated_snapshot) {
            SnapshotPublication::Published => ServiceRemovalOutcome::SnapshotChanged,
            SnapshotPublication::NotCentral(current_participation) => {
                let _clear_outcome = self.clear_visible_services();
                ServiceRemovalOutcome::RejectedParticipation(current_participation)
            }
            SnapshotPublication::CapacityMismatch { .. } => ServiceRemovalOutcome::CapacityMismatch,
        }
    }

    fn clear_visible_services(&self) -> SnapshotClearOutcome {
        let Ok(mut visible_services) = self.shared.visible_services.lock() else {
            self.shared.publisher.clear_snapshot();
            return SnapshotClearOutcome::StateUnavailable;
        };
        *visible_services = DiscoverySnapshot::new(DISCOVERY_CAPACITY);
        self.shared.publisher.clear_snapshot();
        SnapshotClearOutcome::Cleared
    }
}

fn apply_service_resolution(
    discovery_snapshot: &mut DiscoverySnapshot,
    service_advertisement: ServiceAdvertisement,
) -> SnapshotMutation {
    if discovery_snapshot.get(service_advertisement.service()) == Some(&service_advertisement) {
        return SnapshotMutation::Unchanged;
    }
    match discovery_snapshot.insert(service_advertisement) {
        AdvertisementInsertion::Inserted | AdvertisementInsertion::Replaced => {
            SnapshotMutation::Changed
        }
        AdvertisementInsertion::AtCapacity => SnapshotMutation::RejectedAdvertisementCapacity,
    }
}

impl Default for AndroidServiceDiscoveryBridge {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn service_resolution(
        service_instance: &str,
        socket_address: &str,
    ) -> Result<(DiscoveryServiceName, DiscoveryEndpoint), Box<dyn std::error::Error>> {
        Ok((
            DiscoveryServiceName::from_instance(service_instance)?,
            DiscoveryEndpoint::new(socket_address.parse()?)?,
        ))
    }

    #[test]
    fn android_service_updates_replace_candidates_under_one_bounded_identity(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut discovery_snapshot = DiscoverySnapshot::new(NonZeroU8::MIN);
        let (service_name, first_endpoint) = service_resolution("peer", "192.168.1.2:42699")?;
        let (_, second_endpoint) = service_resolution("peer", "192.168.1.3:42699")?;
        let mut initial_advertisement = ServiceAdvertisement::new(service_name.clone());
        assert_eq!(
            initial_advertisement.insert(first_endpoint),
            CandidateInsertion::Inserted
        );
        assert_eq!(
            initial_advertisement.insert(second_endpoint),
            CandidateInsertion::Inserted
        );
        assert_eq!(
            apply_service_resolution(&mut discovery_snapshot, initial_advertisement),
            SnapshotMutation::Changed
        );

        let mut replacement_advertisement = ServiceAdvertisement::new(service_name.clone());
        assert_eq!(
            replacement_advertisement.insert(second_endpoint),
            CandidateInsertion::Inserted
        );
        assert_eq!(
            apply_service_resolution(&mut discovery_snapshot, replacement_advertisement),
            SnapshotMutation::Changed
        );
        let service_advertisement = discovery_snapshot.get(&service_name);
        assert_eq!(
            service_advertisement.map(|advertisement| advertisement.endpoints().len()),
            Some(1)
        );

        let (overflow_name, overflow_endpoint) =
            service_resolution("overflow", "192.168.1.4:42699")?;
        let mut overflow_advertisement = ServiceAdvertisement::new(overflow_name);
        assert_eq!(
            overflow_advertisement.insert(overflow_endpoint),
            CandidateInsertion::Inserted
        );
        assert_eq!(
            apply_service_resolution(&mut discovery_snapshot, overflow_advertisement),
            SnapshotMutation::RejectedAdvertisementCapacity
        );
        Ok(())
    }

    #[test]
    fn android_records_share_endpoint_and_version_validation(
    ) -> Result<(), Box<dyn std::error::Error>> {
        assert_eq!(
            DiscoveryVersion::parse(None),
            Ok(DiscoveryVersion::ImplicitV1)
        );
        assert_eq!(
            DiscoveryVersion::parse(Some(b"1")),
            Ok(DiscoveryVersion::ExplicitV1)
        );
        assert!(DiscoveryVersion::parse(Some(b"2")).is_err());
        assert!(DiscoveryEndpoint::new("192.168.1.2:42699".parse()?).is_ok());
        assert!(DiscoveryEndpoint::new("8.8.8.8:42699".parse()?).is_err());
        Ok(())
    }

    #[test]
    fn inactive_bridge_rejects_late_callbacks() -> Result<(), Box<dyn std::error::Error>> {
        let service_discovery_bridge = AndroidServiceDiscoveryBridge::new();
        assert_eq!(
            service_discovery_bridge.resolved("peer", ["192.168.1.2:42699".parse()?], Some(b"1")),
            ServiceResolutionOutcome::RejectedParticipation(DiscoveryParticipation::Inactive)
        );
        assert!(matches!(
            service_discovery_bridge
                .shared
                .visible_services
                .lock()
                .map(|visible_services| visible_services.len()),
            Ok(0)
        ));
        Ok(())
    }
}
