use std::collections::BTreeMap;
use std::sync::Arc;

use personal_rns::config::{
    DaemonPlan, DiscoveryEncryption, InterfaceDiscoveryPlan, PlannedInterface,
};
use personal_rns::engine::RatchetPolicy;
use personal_rns::identity::in_memory::InMemoryNodeIdentity;
use personal_rns::identity::{IdentitySigner, RemoteIdentity, Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::interface_discovery::{discovery_destination_hash, APP_ASPECTS, APP_NAME};
use personal_rns::interfaces::{InterfaceId, InterfaceOriginKind};
use personal_rns::manifold::tokio::TokioHost;
use personal_rns::routing::links::resources::ResourceStrategy;
use personal_rns::routing::{LinkRequestPolicy, ProofStrategy};
use personal_rns::runtime::{PreConfiguredDestination, PrnsNodeHandle};
use personal_rns::wire::{DestinationHash, TransportId};
use personal_rns::{
    RunningTokioInterfaceDiscoveryPublisher, TokioDiscoveryPublicationEvent,
    TokioDiscoveryPublisherConstructionError, TokioInterfaceDiscoveryPublisher,
};

use crate::daemon::AttachedConfiguredInterface;

mod advertisement;

#[cfg(all(test, unix))]
use advertisement::expand_user_path;
#[cfg(test)]
use advertisement::resolve_reachable_on;
use advertisement::{security_name, DiscoveryAdvertisementResolutionError, PublicationSource};

#[derive(Clone)]
pub(crate) struct PreparedDiscoveryPublisher {
    destination: DestinationHash,
    transport_enabled: bool,
    transport_id: TransportId,
    network_identity: Option<RemoteIdentity>,
}

pub(crate) fn prepare(
    plan: &DaemonPlan,
    transport_identity: &Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>,
    network_identity: Option<&Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>>,
) -> (
    PreConfiguredDestination<'static>,
    PreparedDiscoveryPublisher,
) {
    let transport_signer = InMemoryNodeIdentity::from_secret_key_bytes(transport_identity);
    let transport_id = TransportId::new(*transport_signer.identity_hash().as_bytes());
    let destination_identity = match network_identity {
        Some(identity) => (*identity).clone(),
        None => transport_identity.clone(),
    };
    let destination_signer = InMemoryNodeIdentity::from_secret_key_bytes(&destination_identity);
    let destination = discovery_destination_hash(&destination_signer.identity_hash());
    let network_identity = network_identity.map(|identity| {
        let identity = InMemoryNodeIdentity::from_secret_key_bytes(identity);
        RemoteIdentity::from_public_keys(
            identity.encryption_public_key(),
            identity.signing_public_key(),
        )
    });

    (
        PreConfiguredDestination::Single {
            app_name: APP_NAME,
            aspects: APP_ASPECTS,
            identity: destination_identity,
            announce_app_data: &[],
            proof: ProofStrategy::ProveNone,
            link_requests: LinkRequestPolicy::AcceptNone,
            ratchet: RatchetPolicy::NoRatchets,
            resource_strategy: ResourceStrategy::AcceptNone,
            request_handlers: personal_rns::runtime::RequestHandlerRegistration::None,
        },
        PreparedDiscoveryPublisher {
            destination,
            transport_enabled: plan.transport.routing_enabled(),
            transport_id,
            network_identity,
        },
    )
}

impl PreparedDiscoveryPublisher {
    pub(crate) fn spawn(
        self,
        handle: PrnsNodeHandle,
        clock: TokioHost,
        constructed: Vec<AttachedConfiguredInterface>,
    ) -> Result<Option<RunningTokioInterfaceDiscoveryPublisher>, DiscoveryPublisherStartError> {
        let sources = self.publication_sources(constructed)?;
        if sources.is_empty() {
            return Ok(None);
        }
        let registrations = sources
            .iter()
            .map(|(interface, source)| source.registration(*interface))
            .collect::<Vec<_>>();
        let publisher = TokioInterfaceDiscoveryPublisher::new(
            self.destination,
            registrations,
            self.network_identity,
        )
        .map_err(DiscoveryPublisherStartError::Publisher)?;
        for (interface, source) in &sources {
            tracing::info!(
                event = "interface_discovery_publication_scheduled",
                interface_origin = InterfaceOriginKind::Configured.as_str(),
                interface = ?interface.as_bytes(),
                interface_name = %source.interface_name,
                interface_type = source.interface_type().rns_name(),
                interval_millis = source.announcement.interval.0,
                security = security_name(source.security()),
            );
        }
        let sources = Arc::new(sources);
        let resolver_sources = Arc::clone(&sources);
        let reporter_sources = Arc::clone(&sources);
        Ok(Some(publisher.spawn(
            handle,
            clock,
            move |interface| {
                let sources = Arc::clone(&resolver_sources);
                async move {
                    let source = sources.get(&interface).ok_or(
                        DiscoveryAdvertisementResolutionError::UnknownInterface { interface },
                    )?;
                    source.advertisement(interface).await
                }
            },
            move |event| report_publication_event(&reporter_sources, event),
        )))
    }

    fn publication_sources(
        &self,
        constructed: Vec<AttachedConfiguredInterface>,
    ) -> Result<BTreeMap<InterfaceId, PublicationSource>, DiscoveryPublisherStartError> {
        let mut sources = BTreeMap::new();
        for constructed in constructed {
            let AttachedConfiguredInterface { id, plan } = constructed;
            let PlannedInterface {
                name,
                access,
                discovery,
                ..
            } = plan;
            let announcement = match discovery {
                InterfaceDiscoveryPlan::Disabled | InterfaceDiscoveryPlan::Unpublishable(_) => {
                    continue
                }
                InterfaceDiscoveryPlan::Announce(announcement) => announcement,
            };
            if announcement.encryption == DiscoveryEncryption::NetworkIdentity
                && self.network_identity.is_none()
            {
                tracing::warn!(
                    event = "interface_discovery_publication_unavailable",
                    interface_origin = InterfaceOriginKind::Configured.as_str(),
                    interface = ?id.as_bytes(),
                    interface_name = %name,
                    reason = "network_identity_unavailable",
                );
                continue;
            }
            let source = PublicationSource::new(
                name,
                access,
                announcement,
                self.transport_enabled,
                self.transport_id,
            );
            if sources.insert(id, source).is_some() {
                return Err(DiscoveryPublisherStartError::DuplicateInterface { interface: id });
            }
        }
        Ok(sources)
    }
}

#[derive(Debug)]
pub(crate) enum DiscoveryPublisherStartError {
    DuplicateInterface { interface: InterfaceId },
    Publisher(TokioDiscoveryPublisherConstructionError),
}

impl core::fmt::Display for DiscoveryPublisherStartError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::DuplicateInterface { interface } => {
                write!(formatter, "duplicate discovery interface {interface:?}")
            }
            Self::Publisher(error) => write!(formatter, "publisher construction failed: {error:?}"),
        }
    }
}

impl std::error::Error for DiscoveryPublisherStartError {}

fn report_publication_event(
    sources: &BTreeMap<InterfaceId, PublicationSource>,
    event: TokioDiscoveryPublicationEvent<DiscoveryAdvertisementResolutionError>,
) {
    let interface = match &event {
        TokioDiscoveryPublicationEvent::AdvertisementUnavailable { interface, .. }
        | TokioDiscoveryPublicationEvent::PreparationFailed { interface, .. }
        | TokioDiscoveryPublicationEvent::Prepared { interface, .. }
        | TokioDiscoveryPublicationEvent::FramingFailed { interface, .. }
        | TokioDiscoveryPublicationEvent::AnnounceFailed { interface, .. }
        | TokioDiscoveryPublicationEvent::Announced { interface, .. } => *interface,
    };
    let Some(source) = sources.get(&interface) else {
        tracing::error!(
            event = "interface_discovery_publication_source_missing",
            interface = ?interface.as_bytes(),
        );
        return;
    };
    match event {
        TokioDiscoveryPublicationEvent::AdvertisementUnavailable { error, .. } => {
            tracing::warn!(
                event = "interface_discovery_advertisement_unavailable",
                interface_origin = InterfaceOriginKind::Configured.as_str(),
                interface = ?interface.as_bytes(),
                interface_name = %source.interface_name,
                interface_type = source.interface_type().rns_name(),
                error = %error,
            );
        }
        TokioDiscoveryPublicationEvent::PreparationFailed { failure, .. } => {
            tracing::warn!(
                event = "interface_discovery_publication_preparation_failed",
                interface_origin = InterfaceOriginKind::Configured.as_str(),
                interface = ?interface.as_bytes(),
                interface_name = %source.interface_name,
                interface_type = source.interface_type().rns_name(),
                failure = ?failure,
            );
        }
        TokioDiscoveryPublicationEvent::Prepared {
            stamp_value,
            stamp_attempts,
            cache_hit,
            ..
        } => {
            tracing::debug!(
                event = "interface_discovery_publication_prepared",
                interface_origin = InterfaceOriginKind::Configured.as_str(),
                interface = ?interface.as_bytes(),
                interface_name = %source.interface_name,
                interface_type = source.interface_type().rns_name(),
                stamp_value = stamp_value.get(),
                stamp_attempts,
                cache_hit,
            );
        }
        TokioDiscoveryPublicationEvent::FramingFailed { failure, .. } => {
            tracing::warn!(
                event = "interface_discovery_publication_framing_failed",
                interface_origin = InterfaceOriginKind::Configured.as_str(),
                interface = ?interface.as_bytes(),
                interface_name = %source.interface_name,
                interface_type = source.interface_type().rns_name(),
                failure = ?failure,
            );
        }
        TokioDiscoveryPublicationEvent::AnnounceFailed { failure, .. } => {
            tracing::warn!(
                event = "interface_discovery_announce_failed",
                interface_origin = InterfaceOriginKind::Configured.as_str(),
                interface = ?interface.as_bytes(),
                interface_name = %source.interface_name,
                interface_type = source.interface_type().rns_name(),
                failure = ?failure,
            );
        }
        TokioDiscoveryPublicationEvent::Announced { app_data_bytes, .. } => {
            tracing::info!(
                event = "interface_discovery_announced",
                interface_origin = InterfaceOriginKind::Configured.as_str(),
                interface = ?interface.as_bytes(),
                interface_name = %source.interface_name,
                interface_type = source.interface_type().rns_name(),
                app_data_bytes = app_data_bytes,
            );
        }
    }
}

#[cfg(test)]
mod tests;
