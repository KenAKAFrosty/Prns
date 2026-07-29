use std::time::Duration;

use personal_rns::config::DaemonPlan;
use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::runtime::request_endpoints::RequestEndpointSet;
use personal_rns::runtime::{PrnsEvent, PrnsNode, PrnsNodeHandle};
use personal_rns::storage::StorageLayout;

mod blackhole_exchange;
mod management_announcements;
mod node_page;
mod probe_responder;
mod remote_management;
mod request_routes;
mod request_state;

const MANAGEMENT_ANNOUNCE_INTERVAL: Duration = Duration::from_secs(2 * 60 * 60);

pub(crate) use blackhole_exchange::{
    spawn_updater as spawn_blackhole_updater, BlackholeUpdateTask, ListRoute,
};
pub(crate) use management_announcements::{AnnouncedDestination, ManagementAnnounceTask};
pub(crate) use remote_management::{PathRoute, StatusRoute};
pub(crate) use request_routes::DaemonRequestRoutes;
pub(crate) use request_state::{DaemonRequestState, TransportStatusIdentity};

pub(crate) struct ManagementDestinations {
    announced: Vec<AnnouncedDestination>,
    node_pages: Option<node_page::NodePageDestination>,
}

impl ManagementDestinations {
    pub(crate) const fn none() -> Self {
        Self {
            announced: Vec::new(),
            node_pages: None,
        }
    }

    pub(crate) fn node_page_destination(&self) -> Option<personal_rns::wire::DestinationHash> {
        self.node_pages.as_ref().map(|destination| destination.hash)
    }
}

pub(crate) struct HostedServiceActivationFailed;

pub(crate) fn activate<R, F, S>(
    node: &mut PrnsNode<DaemonRequestState, R, F, S>,
    plan: &DaemonPlan,
    identity: &Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>,
    node_pages: &crate::node_pages::NodePageCatalog,
) -> Result<ManagementDestinations, HostedServiceActivationFailed>
where
    R: RequestEndpointSet<DaemonRequestState>,
    F: FnMut(PrnsEvent<'_>, &DaemonRequestState),
    S: StorageLayout,
{
    let mut destinations = Vec::new();
    if let Some(allowed) = plan.remote_management.allowed() {
        match remote_management::activate(node, identity.clone(), allowed) {
            Ok(destination) => {
                destinations.push(AnnouncedDestination {
                    hash: destination,
                    available_when: None,
                    interval: MANAGEMENT_ANNOUNCE_INTERVAL,
                });
                tracing::info!(
                    event = "remote_management_enabled",
                    destination = ?destination.as_bytes(),
                    allowed_identities = allowed.len(),
                );
            }
            Err(error) => {
                tracing::error!(
                    event = "remote_management_start_failed",
                    error = ?error,
                );
                return Err(HostedServiceActivationFailed);
            }
        }
    }
    if plan.probe_responder.is_enabled() {
        match probe_responder::activate(node, identity.clone()) {
            Ok(destination) => {
                destinations.push(AnnouncedDestination {
                    hash: destination,
                    available_when: None,
                    interval: MANAGEMENT_ANNOUNCE_INTERVAL,
                });
                tracing::info!(
                    event = "probe_responder_enabled",
                    destination = ?destination.as_bytes(),
                );
            }
            Err(error) => {
                tracing::error!(
                    event = "probe_responder_start_failed",
                    error = ?error,
                );
                return Err(HostedServiceActivationFailed);
            }
        }
    }
    if plan.blackhole_exchange.publication().is_enabled() {
        match blackhole_exchange::activate(node, identity.clone()) {
            Ok(destination) => {
                destinations.push(AnnouncedDestination {
                    hash: destination,
                    available_when: None,
                    interval: MANAGEMENT_ANNOUNCE_INTERVAL,
                });
                tracing::info!(
                    event = "blackhole_publisher_enabled",
                    destination = ?destination.as_bytes(),
                );
            }
            Err(error) => {
                tracing::error!(
                    event = "blackhole_publisher_start_failed",
                    error = ?error,
                );
                return Err(HostedServiceActivationFailed);
            }
        }
    }
    let node_page_destination = match node_page::activate(node, identity.clone(), node_pages) {
        Ok(destination) => {
            tracing::info!(
                event = "node_pages_enabled",
                destination = ?destination.hash.as_bytes(),
                index = %destination.index_path.display(),
            );
            if plan.node_page_announcements.is_enabled() {
                destinations.push(AnnouncedDestination {
                    hash: destination.hash,
                    available_when: Some(destination.index_path.clone()),
                    interval: plan.node_page_announcements.interval().duration(),
                });
            } else {
                tracing::info!(
                    event = "node_page_announcements_disabled",
                    destination = ?destination.hash.as_bytes(),
                );
            }
            Some(destination)
        }
        Err(error) => {
            tracing::error!(
                event = "node_pages_start_failed",
                error = ?error,
            );
            return Err(HostedServiceActivationFailed);
        }
    };
    Ok(ManagementDestinations {
        announced: destinations,
        node_pages: node_page_destination,
    })
}

pub(crate) fn spawn_management_announcements(
    handle: PrnsNodeHandle,
    destinations: ManagementDestinations,
) -> Option<ManagementAnnounceTask> {
    management_announcements::spawn(handle, destinations.announced)
}
