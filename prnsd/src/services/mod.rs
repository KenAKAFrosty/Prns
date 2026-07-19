use personal_rns::config::DaemonPlan;
use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::runtime::request_router::RouteSet;
use personal_rns::runtime::{PrnsEvent, PrnsNode, PrnsNodeHandle};
use personal_rns::storage::StorageLayout;
use personal_rns::wire::DestinationHash;

mod blackhole_exchange;
mod management_announcements;
mod probe_responder;
mod remote_management;
mod request_state;

pub(crate) use blackhole_exchange::{spawn_updater as spawn_blackhole_updater, ListRoute};
pub(crate) use management_announcements::ManagementAnnounceTask;
pub(crate) use remote_management::{PathRoute, StatusRoute};
pub(crate) use request_state::{DaemonRequestState, TransportStatusIdentity};

pub(crate) struct ManagementDestinations(Vec<DestinationHash>);

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
) -> Result<ManagementDestinations, HostedServiceActivationFailed>
where
    R: RouteSet<DaemonRequestState>,
    F: FnMut(PrnsEvent<'_>, &DaemonRequestState),
    S: StorageLayout,
{
    let mut destinations = Vec::new();
    if let Some(allowed) = plan.remote_management.allowed() {
        match remote_management::activate(node, identity.clone(), allowed) {
            Ok(destination) => {
                destinations.push(destination);
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
                destinations.push(destination);
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
                destinations.push(destination);
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
    Ok(ManagementDestinations(destinations))
}

pub(crate) fn spawn_management_announcements(
    handle: PrnsNodeHandle,
    destinations: ManagementDestinations,
) -> Option<ManagementAnnounceTask> {
    management_announcements::spawn(handle, destinations.0)
}
