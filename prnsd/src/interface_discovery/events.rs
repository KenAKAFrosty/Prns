use personal_rns::interface_discovery::{DiscoveryCatalogRefresh, DiscoveryCatalogUpdate};
use personal_rns::interfaces::InterfaceOriginKind;
use personal_rns::{DiscoveryIngressOutcome, TokioDiscoveryEvent};

pub(super) fn trace_ingress(outcome: DiscoveryIngressOutcome) {
    match outcome {
        DiscoveryIngressOutcome::Disabled
        | DiscoveryIngressOutcome::NotDiscovery
        | DiscoveryIngressOutcome::Queued => {}
        DiscoveryIngressOutcome::QueueFull => {
            tracing::warn!(event = "interface_discovery_ingress_full");
        }
        DiscoveryIngressOutcome::Closed => {
            tracing::debug!(event = "interface_discovery_ingress_closed");
        }
    }
}

pub(super) fn trace(event: &TokioDiscoveryEvent<'_>) {
    match event {
        TokioDiscoveryEvent::IntakeNotApplicable(reason) => {
            tracing::trace!(event = "interface_discovery_not_applicable", reason = ?reason);
        }
        TokioDiscoveryEvent::IntakeRejected(rejection) => {
            tracing::debug!(
                event = "interface_discovery_rejected",
                reason = ?rejection.kind(),
                detail = ?rejection,
            );
        }
        TokioDiscoveryEvent::CatalogStoreRejected(error) => {
            tracing::warn!(
                event = "interface_discovery_catalog_store_rejected",
                error = %error,
            );
        }
        TokioDiscoveryEvent::CatalogUpdated { update, record } => {
            let interface = record.interface();
            match *update {
                DiscoveryCatalogUpdate::Added { .. } => {
                    tracing::info!(
                        event = "interface_discovered",
                        interface_origin = InterfaceOriginKind::Discovered.as_str(),
                        discovery_id = ?interface.id.as_bytes(),
                        interface_name = %interface.name,
                        interface_type = interface.advertisement.interface_type.rns_name(),
                        announced_by = ?interface.provenance.announced_by.as_bytes(),
                        received_on = ?interface.provenance.received_on.as_bytes(),
                        hops = interface.provenance.hops.0,
                        stamp_value = interface.stamp_value.get(),
                    );
                }
                DiscoveryCatalogUpdate::Refreshed { refresh, .. } => {
                    let advertisement_changed = match refresh {
                        DiscoveryCatalogRefresh::AdvertisementUnchanged => false,
                        DiscoveryCatalogRefresh::AdvertisementChanged => true,
                    };
                    tracing::debug!(
                        event = "interface_discovery_refreshed",
                        interface_origin = InterfaceOriginKind::Discovered.as_str(),
                        discovery_id = ?interface.id.as_bytes(),
                        interface_name = %interface.name,
                        advertisement_changed,
                        observations = record.observation_count().get(),
                    );
                }
                DiscoveryCatalogUpdate::IgnoredOutOfOrder {
                    received_at,
                    last_heard,
                    ..
                } => {
                    tracing::debug!(
                        event = "interface_discovery_out_of_order",
                        interface_origin = InterfaceOriginKind::Discovered.as_str(),
                        discovery_id = ?interface.id.as_bytes(),
                        received_at = received_at.0,
                        last_heard = last_heard.0,
                    );
                }
            }
        }
        TokioDiscoveryEvent::CatalogExpired(record) => {
            tracing::info!(
                event = "interface_discovery_expired",
                interface_origin = InterfaceOriginKind::Discovered.as_str(),
                discovery_id = ?record.id().as_bytes(),
                interface_name = %record.interface().name,
            );
        }
        TokioDiscoveryEvent::CatalogBlackholed(record) => {
            tracing::info!(
                event = "interface_discovery_blackholed",
                interface_origin = InterfaceOriginKind::Discovered.as_str(),
                discovery_id = ?record.id().as_bytes(),
                interface_name = %record.interface().name,
            );
        }
        TokioDiscoveryEvent::ConnectionAttached { plan, interface } => {
            tracing::info!(
                event = "interface_discovery_connected",
                interface_origin = InterfaceOriginKind::Discovered.as_str(),
                interface = ?interface.as_bytes(),
                discovery_id = ?plan.discovery_id().as_bytes(),
                interface_name = %plan.name(),
                interface_type = plan.advertised_type().rns_name(),
                host = plan.endpoint().host(),
                port = plan.endpoint().port(),
                announced_by = ?plan.provenance().announced_by.as_bytes(),
            );
        }
        TokioDiscoveryEvent::ConnectionAttachFailed { plan, failure } => {
            tracing::warn!(
                event = "interface_discovery_connect_failed",
                interface_origin = InterfaceOriginKind::Discovered.as_str(),
                discovery_id = ?plan.discovery_id().as_bytes(),
                interface_name = %plan.name(),
                host = plan.endpoint().host(),
                port = plan.endpoint().port(),
                failure = ?failure,
            );
        }
        TokioDiscoveryEvent::ConnectionDisconnected {
            discovery,
            interface,
            since,
        } => {
            tracing::debug!(
                event = "interface_discovery_disconnected",
                interface_origin = InterfaceOriginKind::Discovered.as_str(),
                discovery_id = ?discovery.as_bytes(),
                interface = ?interface.as_bytes(),
                since = since.0,
            );
        }
        TokioDiscoveryEvent::ConnectionReconnected {
            discovery,
            interface,
        } => {
            tracing::info!(
                event = "interface_discovery_reconnected",
                interface_origin = InterfaceOriginKind::Discovered.as_str(),
                discovery_id = ?discovery.as_bytes(),
                interface = ?interface.as_bytes(),
            );
        }
        TokioDiscoveryEvent::ConnectionDetached {
            discovery,
            interface,
        } => {
            tracing::info!(
                event = "interface_discovery_detached",
                interface_origin = InterfaceOriginKind::Discovered.as_str(),
                discovery_id = ?discovery.as_bytes(),
                interface = ?interface.as_bytes(),
            );
        }
        TokioDiscoveryEvent::AutoConnectCapacity { online, maximum } => {
            tracing::trace!(
                event = "interface_discovery_auto_connect_capacity",
                online,
                maximum,
            );
        }
    }
}
