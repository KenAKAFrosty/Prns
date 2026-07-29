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

pub(crate) use blackhole_exchange::{
    spawn_updater as spawn_blackhole_updater, BlackholeUpdateTask, ListRoute,
};
pub(crate) use management_announcements::{AnnouncedDestination, ManagementAnnounceTask};
pub(crate) use remote_management::{PathRoute, StatusRoute};
pub(crate) use request_routes::DaemonRequestRoutes;
pub(crate) use request_state::{DaemonRequestState, TransportStatusIdentity};

pub(crate) struct ManagementDestinations(Vec<AnnouncedDestination>);

impl ManagementDestinations {
    pub(crate) const fn none() -> Self {
        Self(Vec::new())
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
    match node_page::activate(node, identity.clone(), node_pages) {
        Ok(Some(destination)) => {
            tracing::info!(
                event = "node_pages_enabled",
                destination = ?destination.hash.as_bytes(),
                index = %destination.index_path.display(),
            );
            destinations.push(AnnouncedDestination {
                hash: destination.hash,
                available_when: Some(destination.index_path),
            });
        }
        Ok(None) => {}
        Err(error) => {
            tracing::error!(
                event = "node_pages_start_failed",
                error = ?error,
            );
            return Err(HostedServiceActivationFailed);
        }
    }
    Ok(ManagementDestinations(destinations))
}

pub(crate) fn spawn_management_announcements(
    handle: PrnsNodeHandle,
    destinations: ManagementDestinations,
) -> Option<ManagementAnnounceTask> {
    management_announcements::spawn(handle, destinations.0)
}
