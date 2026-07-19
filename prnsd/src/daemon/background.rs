use std::collections::BTreeSet;
use std::time::Instant;

use personal_rns::config::DaemonPlan;
use personal_rns::from_plan::PlanRuntimeContext;
use personal_rns::interfaces::InterfaceId;
use personal_rns::runtime::request_router::RouteSet;
use personal_rns::runtime::{PrnsEvent, PrnsNode, PrnsNodeHandle};
use personal_rns::shared_instance::RnsBlackholeFiles;
use personal_rns::storage::StorageLayout;
use personal_rns::RunningTokioInterfaceDiscoveryPublisher;
use tokio::sync::watch;

use crate::interface_discovery::publication::PreparedDiscoveryPublisher;
use crate::interface_discovery::{
    BootstrapInterfaces, MonitoredInterfaces, PreparedDiscovery, RunningDiscovery,
};
use crate::observability::ObservabilityGuard;
use crate::services::{
    self, BlackholeUpdateTask, DaemonRequestState, ManagementAnnounceTask, ManagementDestinations,
};

use super::interface_ownership::{InterfaceOwnership, RoutingTableOwnership};

pub(super) struct BackgroundTasks {
    interface_failure_watch: watch::Receiver<BTreeSet<InterfaceId>>,
    discovery: Option<RunningDiscovery>,
    discovery_publication: Option<RunningTokioInterfaceDiscoveryPublisher>,
    management_announcements: Option<ManagementAnnounceTask>,
    blackhole_updates: Option<BlackholeUpdateTask>,
    #[cfg(feature = "otlp")]
    metrics: Option<crate::observability::RunningMetricsReporter>,
}

impl BackgroundTasks {
    pub(super) fn interface_failure_watch(&self) -> &watch::Receiver<BTreeSet<InterfaceId>> {
        &self.interface_failure_watch
    }

    pub(super) async fn shutdown(self) {
        if let Some(discovery) = self.discovery {
            discovery.shutdown().await;
        }
        if let Some(publisher) = self.discovery_publication {
            if let Err(error) = publisher.shutdown().await {
                tracing::warn!(event = "interface_discovery_publisher_task_failed", error = %error);
            }
        }
        if let Some(task) = self.management_announcements {
            task.shutdown().await;
        }
        if let Some(task) = self.blackhole_updates {
            task.shutdown().await;
        }
        #[cfg(feature = "otlp")]
        if let Some(metrics) = self.metrics {
            metrics.shutdown().await;
        }
    }
}

pub(super) struct BackgroundInputs<'a, R, F, S: StorageLayout> {
    pub(super) node: PrnsNode<DaemonRequestState, R, F, S>,
    pub(super) handle: &'a PrnsNodeHandle,
    pub(super) plan: &'a DaemonPlan,
    pub(super) interface_runtime: &'a PlanRuntimeContext,
    pub(super) ownership: InterfaceOwnership,
    pub(super) prepared_discovery: Option<PreparedDiscovery>,
    pub(super) prepared_discovery_publisher: Option<PreparedDiscoveryPublisher>,
    pub(super) blackhole_files: RnsBlackholeFiles,
    pub(super) management_destinations: ManagementDestinations,
    pub(super) observability: &'a ObservabilityGuard,
    pub(super) started: Instant,
}

pub(super) fn start<R, F, S>(
    inputs: BackgroundInputs<'_, R, F, S>,
) -> (PrnsNode<DaemonRequestState, R, F, S>, BackgroundTasks)
where
    R: RouteSet<DaemonRequestState>,
    F: FnMut(PrnsEvent<'_>, &DaemonRequestState),
    S: StorageLayout,
{
    let BackgroundInputs {
        mut node,
        handle,
        plan,
        interface_runtime,
        ownership,
        prepared_discovery,
        prepared_discovery_publisher,
        blackhole_files,
        management_destinations,
        observability,
        started,
    } = inputs;
    let management_announcements =
        services::spawn_management_announcements(handle.clone(), management_destinations);
    let (interface_failure_watch, discovery, discovery_publication, blackhole_updates) =
        match ownership.into_routing_tables() {
            Some(RoutingTableOwnership {
                configured_interfaces,
                bootstrap_attachments,
            }) => {
                let monitored_interfaces = MonitoredInterfaces::new(
                    configured_interfaces.iter().map(|interface| interface.id),
                );
                let interface_failure_watch = monitored_interfaces.subscribe();
                let bootstrap_interfaces = BootstrapInterfaces::prepare(
                    plan,
                    interface_runtime.clone(),
                    bootstrap_attachments,
                    monitored_interfaces,
                );
                let discovery = match prepared_discovery {
                    Some(discovery) => {
                        let observer = discovery.observer();
                        node = node.with_accepted_announce_observer(move |observation| {
                            observer.observe(observation);
                        });
                        let clock = node.clock();
                        Some(discovery.spawn(handle.clone(), clock, bootstrap_interfaces))
                    }
                    None => None,
                };
                let discovery_publication = prepared_discovery_publisher.and_then(|publisher| {
                    let clock = node.clock();
                    match publisher.spawn(handle.clone(), clock, configured_interfaces) {
                        Ok(task) => task,
                        Err(error) => {
                            tracing::error!(
                                event = "interface_discovery_publisher_start_failed",
                                error = %error,
                            );
                            None
                        }
                    }
                });
                let blackhole_updates = services::spawn_blackhole_updater(
                    handle.clone(),
                    node.clock(),
                    blackhole_files,
                    &plan.blackhole_exchange,
                );
                (
                    interface_failure_watch,
                    discovery,
                    discovery_publication,
                    blackhole_updates,
                )
            }
            None => {
                let monitored_interfaces = MonitoredInterfaces::new(std::iter::empty());
                (monitored_interfaces.subscribe(), None, None, None)
            }
        };
    #[cfg(feature = "otlp")]
    let metrics = observability.spawn_metrics_reporter(handle.clone(), started);
    #[cfg(not(feature = "otlp"))]
    let _ = (observability, started);
    (
        node,
        BackgroundTasks {
            interface_failure_watch,
            discovery,
            discovery_publication,
            management_announcements,
            blackhole_updates,
            #[cfg(feature = "otlp")]
            metrics,
        },
    )
}
