use personal_rns::config::{DaemonPlan, PlannedMedium};
use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::interface_discovery::{DiscoveryCatalogRefresh, DiscoveryCatalogUpdate};
use personal_rns::reactor::impls::tokio_reactor::TokioHost;
use personal_rns::routing::announce::AnnounceObservation;
use personal_rns::runtime::TokioPrnsHandle;
use personal_rns::{
    DiscoveryIngressOutcome, TokioDiscoveryEvent, TokioDiscoveryIngress, TokioInterfaceDiscovery,
};

pub struct PreparedDiscovery {
    service: TokioInterfaceDiscovery,
    ingress: TokioDiscoveryIngress,
}

impl PreparedDiscovery {
    pub fn from_plan(
        plan: &DaemonPlan,
        network_identity: Option<Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>>,
    ) -> Option<Self> {
        plan.discovery.enabled_policy()?;
        let (mut service, ingress) =
            TokioInterfaceDiscovery::new(plan.discovery.clone(), network_identity);
        for interface in &plan.interfaces {
            match &interface.medium {
                PlannedMedium::TcpClient { host, port }
                | PlannedMedium::BackboneClient { host, port } => {
                    service.reserve_endpoint(host, *port);
                }
                PlannedMedium::AutoWifi { .. }
                | PlannedMedium::TcpServer { .. }
                | PlannedMedium::Udp { .. }
                | PlannedMedium::Serial { .. }
                | PlannedMedium::Kiss { .. }
                | PlannedMedium::Ax25Kiss { .. }
                | PlannedMedium::Rnode { .. }
                | PlannedMedium::Backbone { .. }
                | PlannedMedium::Pipe { .. } => {}
            }
        }
        Some(Self { service, ingress })
    }

    pub fn observer(&self) -> DiscoveryObserver {
        DiscoveryObserver {
            ingress: self.ingress.clone(),
        }
    }

    pub async fn run(self, handle: TokioPrnsHandle, clock: TokioHost) {
        self.service.run(handle, clock, render).await;
    }
}

pub struct DiscoveryObserver {
    ingress: TokioDiscoveryIngress,
}

impl DiscoveryObserver {
    pub fn observe(&self, observation: AnnounceObservation<'_>) {
        match self.ingress.observe(observation) {
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
}

fn trace_discovery_event(event: TokioDiscoveryEvent<'_>) {
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
        TokioDiscoveryEvent::CatalogUpdated { update, record } => {
            let interface = record.interface();
            match update {
                DiscoveryCatalogUpdate::Added { .. } => {
                    tracing::info!(
                        event = "interface_discovered",
                        origin = "discovered",
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
                        origin = "discovered",
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
                        origin = "discovered",
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
                origin = "discovered",
                discovery_id = ?record.id().as_bytes(),
                interface_name = %record.interface().name,
            );
        }
        TokioDiscoveryEvent::ConnectionAttached { plan, interface } => {
            tracing::info!(
                event = "interface_discovery_connected",
                origin = "discovered",
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
                origin = "discovered",
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
                origin = "discovered",
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
                origin = "discovered",
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
                origin = "discovered",
                discovery_id = ?discovery.as_bytes(),
                interface = ?interface.as_bytes(),
            );
        }
    }
}
