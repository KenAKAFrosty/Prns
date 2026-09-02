use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::Instant;

use personal_rns::config::{ConfiguredInterfaceLifecycle, DaemonPlan};
use personal_rns::from_plan::{PlanAttachments, PlanRuntimeContext};
use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::interfaces::InterfaceId;
use personal_rns::manifold::tokio::TokioClock;
use personal_rns::runtime::request_endpoints::RequestEndpointSet;
use personal_rns::runtime::{PrnsEvent, PrnsNode, PrnsNodeHandle};
use personal_rns::shared_instance::RnsBlackholeFiles;
use personal_rns::storage::StorageLayout;
use personal_rns::RunningTokioInterfaceDiscoveryPublisher;
use prnsd_control::ReloadResult;
use tokio::sync::watch;

use crate::interface_discovery::publication::PreparedDiscoveryPublisher;
use crate::interface_discovery::{
    BootstrapInterfaces, DiscoveryObserver, MonitoredInterfaces, PreparedDiscovery,
    RunningDiscovery,
};
use crate::observability::ObservabilityGuard;
use crate::services::{
    self, BlackholeUpdateTask, DaemonRequestState, ManagementAnnounceTask, ManagementDestinations,
};

use super::interface_ownership::{InterfaceOwnership, RoutingTableOwnership};
use super::{configured_interfaces, configured_interfaces::ConfiguredInterfaceManager};

pub(super) struct BackgroundTasks {
    interface_failure_watch: watch::Receiver<BTreeSet<InterfaceId>>,
    discovery: Option<RunningDiscovery>,
    discovery_publication: Option<RunningTokioInterfaceDiscoveryPublisher>,
    management_announcements: Option<ManagementAnnounceTask>,
    blackhole_updates: Option<BlackholeUpdateTask>,
    configured_interfaces: Option<ConfiguredInterfaceManager>,
    bootstrap_attachments: PlanAttachments,
    monitored_interfaces: MonitoredInterfaces,
    active_plan: DaemonPlan,
    interface_runtime: PlanRuntimeContext,
    network_identity: Option<Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>>,
    config_dir: PathBuf,
    publisher_factory: Option<PreparedDiscoveryPublisher>,
    discovery_observer: Arc<RwLock<Option<DiscoveryObserver>>>,
    clock: TokioClock,
    #[cfg(feature = "otlp")]
    metrics: Option<crate::observability::RunningMetricsReporter>,
}

impl BackgroundTasks {
    pub(super) fn interface_failure_watch(&self) -> &watch::Receiver<BTreeSet<InterfaceId>> {
        &self.interface_failure_watch
    }

    pub(super) async fn apply_interfaces(
        &mut self,
        handle: &PrnsNodeHandle,
        plan: DaemonPlan,
    ) -> ReloadResult {
        if self.configured_interfaces.is_none() {
            return ReloadResult::NotInterfaceOwner;
        }
        if self.active_plan.interfaces == plan.interfaces {
            return ReloadResult::Unchanged;
        }
        self.stop_interface_services(handle).await;
        let previous = self.active_plan.clone();
        let Some(manager) = self.configured_interfaces.as_mut() else {
            return ReloadResult::NotInterfaceOwner;
        };
        let reconciliation = manager
            .reconcile(handle, &plan, &self.interface_runtime)
            .await;
        match reconciliation {
            configured_interfaces::ReconcileResult::RolledBack { rollback_failed } => {
                let restored = self.construct_bootstrap(handle, &previous).await;
                let services_failed = !self
                    .activate_interface_services(handle, &previous, restored)
                    .await;
                return ReloadResult::RolledBack {
                    rollback_failed: rollback_failed || services_failed,
                };
            }
            configured_interfaces::ReconcileResult::Applied
            | configured_interfaces::ReconcileResult::Unchanged => {}
        }
        let replacement = self.construct_bootstrap(handle, &plan).await;
        if replacement.startup.failed != 0 {
            replacement.into_attachments().detach(handle).await;
            let rollback_failed = match self.configured_interfaces.as_mut() {
                Some(manager) => !matches!(
                    manager
                        .reconcile(handle, &previous, &self.interface_runtime)
                        .await,
                    configured_interfaces::ReconcileResult::Applied
                        | configured_interfaces::ReconcileResult::Unchanged
                ),
                None => true,
            };
            let restored = self.construct_bootstrap(handle, &previous).await;
            let services_failed = !self
                .activate_interface_services(handle, &previous, restored)
                .await;
            return ReloadResult::RolledBack {
                rollback_failed: rollback_failed || services_failed,
            };
        }
        if !self
            .activate_interface_services(handle, &plan, replacement)
            .await
        {
            self.stop_interface_services(handle).await;
            let rollback_failed = match self.configured_interfaces.as_mut() {
                Some(manager) => !matches!(
                    manager
                        .reconcile(handle, &previous, &self.interface_runtime)
                        .await,
                    configured_interfaces::ReconcileResult::Applied
                        | configured_interfaces::ReconcileResult::Unchanged
                ),
                None => true,
            };
            let restored = self.construct_bootstrap(handle, &previous).await;
            let services_failed = !self
                .activate_interface_services(handle, &previous, restored)
                .await;
            return ReloadResult::RolledBack {
                rollback_failed: rollback_failed || services_failed,
            };
        }
        self.active_plan = plan;
        ReloadResult::Applied
    }

    async fn construct_bootstrap(
        &self,
        handle: &PrnsNodeHandle,
        plan: &DaemonPlan,
    ) -> configured_interfaces::ConstructedInterfaces {
        let mut bootstrap = plan.clone();
        bootstrap
            .interfaces
            .retain(|interface| interface.lifecycle == ConfiguredInterfaceLifecycle::BootstrapOnly);
        configured_interfaces::construct(handle, &bootstrap, &self.interface_runtime).await
    }

    async fn stop_interface_services(&mut self, handle: &PrnsNodeHandle) {
        if let Some(discovery) = self.discovery.take() {
            discovery.shutdown().await;
        }
        if let Some(publisher) = self.discovery_publication.take() {
            if let Err(error) = publisher.shutdown().await {
                tracing::warn!(event = "interface_discovery_publisher_task_failed", error = %error);
            }
        }
        let active = std::mem::take(&mut self.bootstrap_attachments);
        let interfaces = active.interfaces().collect::<Vec<_>>();
        self.monitored_interfaces.remove(interfaces);
        active.detach(handle).await;
        if let Ok(mut observer) = self.discovery_observer.write() {
            *observer = None;
        }
    }

    async fn activate_interface_services(
        &mut self,
        handle: &PrnsNodeHandle,
        plan: &DaemonPlan,
        bootstrap: configured_interfaces::ConstructedInterfaces,
    ) -> bool {
        let mut configured = self
            .configured_interfaces
            .as_ref()
            .map(ConfiguredInterfaceManager::attached)
            .unwrap_or_default();
        let bootstrap_attached = bootstrap.attached();
        self.monitored_interfaces
            .add(bootstrap_attached.iter().map(|interface| interface.id));
        configured.extend(bootstrap_attached);
        let bootstrap_attachments = bootstrap.into_attachments();
        let prepared_bootstrap = BootstrapInterfaces::prepare(
            plan,
            self.interface_runtime.clone(),
            bootstrap_attachments,
            self.monitored_interfaces.clone(),
        );
        let prepared_discovery =
            PreparedDiscovery::from_plan(plan, self.network_identity.clone(), &self.config_dir);
        match (prepared_discovery, prepared_bootstrap) {
            (Some(discovery), Ok(bootstrap)) => {
                if let Ok(mut observer) = self.discovery_observer.write() {
                    *observer = Some(discovery.observer());
                }
                self.discovery =
                    Some(discovery.spawn(handle.clone(), self.clock.clone(), Some(bootstrap)));
            }
            (Some(discovery), Err(attachments)) => {
                if let Ok(mut observer) = self.discovery_observer.write() {
                    *observer = Some(discovery.observer());
                }
                self.discovery = Some(discovery.spawn(handle.clone(), self.clock.clone(), None));
                self.bootstrap_attachments = attachments;
            }
            (None, Ok(bootstrap)) => {
                self.bootstrap_attachments = bootstrap.into_active();
            }
            (None, Err(attachments)) => {
                self.bootstrap_attachments = attachments;
            }
        }
        let Some(publisher) = self.publisher_factory.clone() else {
            return true;
        };
        match publisher.spawn(handle.clone(), self.clock.clone(), configured) {
            Ok(task) => {
                self.discovery_publication = task;
                true
            }
            Err(error) => {
                tracing::error!(
                    event = "interface_discovery_publisher_start_failed",
                    error = %error,
                );
                false
            }
        }
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
    pub(super) network_identity: Option<Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>>,
    pub(super) config_dir: PathBuf,
    pub(super) blackhole_files: RnsBlackholeFiles,
    pub(super) management_destinations: ManagementDestinations,
    pub(super) observability: &'a ObservabilityGuard,
    pub(super) started: Instant,
}

pub(super) fn start<R, F, S>(
    inputs: BackgroundInputs<'_, R, F, S>,
) -> (PrnsNode<DaemonRequestState, R, F, S>, BackgroundTasks)
where
    R: RequestEndpointSet<DaemonRequestState>,
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
        network_identity,
        config_dir,
        blackhole_files,
        management_destinations,
        observability,
        started,
    } = inputs;
    let management_announcements =
        services::spawn_management_announcements(handle.clone(), management_destinations);
    let clock = node.clock();
    let publisher_factory = prepared_discovery_publisher.clone();
    let discovery_observer = Arc::new(RwLock::new(None::<DiscoveryObserver>));
    let observer_slot = Arc::clone(&discovery_observer);
    node = node.with_accepted_announce_observer(move |observation| {
        if let Ok(observer) = observer_slot.read() {
            if let Some(observer) = observer.as_ref() {
                observer.observe(observation);
            }
        }
    });
    let (
        interface_failure_watch,
        discovery,
        discovery_publication,
        blackhole_updates,
        configured_interfaces,
        bootstrap_attachments,
        monitored_interfaces,
    ) = match ownership.into_routing_tables() {
        Some(RoutingTableOwnership { configured_units }) => {
            let configured_interfaces =
                configured_interfaces::attached_from_units(&configured_units);
            let monitored_interfaces = MonitoredInterfaces::new(
                configured_interfaces.iter().map(|interface| interface.id),
            );
            let interface_failure_watch = monitored_interfaces.subscribe();
            let (persistent_units, bootstrap_units) =
                configured_interfaces::partition_units(configured_units);
            let bootstrap_attachments =
                configured_interfaces::attachments_from_units(bootstrap_units);
            let (bootstrap_interfaces, bootstrap_attachments) = match BootstrapInterfaces::prepare(
                plan,
                interface_runtime.clone(),
                bootstrap_attachments,
                monitored_interfaces.clone(),
            ) {
                Ok(bootstrap) => (Some(bootstrap), PlanAttachments::default()),
                Err(attachments) => (None, attachments),
            };
            let discovery = match prepared_discovery {
                Some(discovery) => {
                    let observer = discovery.observer();
                    if let Ok(mut slot) = discovery_observer.write() {
                        *slot = Some(observer);
                    }
                    Some(discovery.spawn(handle.clone(), clock.clone(), bootstrap_interfaces))
                }
                None => None,
            };
            let discovery_publication = prepared_discovery_publisher.and_then(|publisher| {
                match publisher.spawn(handle.clone(), clock.clone(), configured_interfaces) {
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
                clock.clone(),
                blackhole_files,
                &plan.blackhole_exchange,
            );
            (
                interface_failure_watch,
                discovery,
                discovery_publication,
                blackhole_updates,
                Some(ConfiguredInterfaceManager::new(
                    persistent_units,
                    monitored_interfaces.clone(),
                )),
                bootstrap_attachments,
                monitored_interfaces,
            )
        }
        None => {
            let monitored_interfaces = MonitoredInterfaces::new(std::iter::empty());
            (
                monitored_interfaces.subscribe(),
                None,
                None,
                None,
                None,
                PlanAttachments::default(),
                monitored_interfaces,
            )
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
            configured_interfaces,
            bootstrap_attachments,
            monitored_interfaces,
            active_plan: plan.clone(),
            interface_runtime: interface_runtime.clone(),
            network_identity,
            config_dir,
            publisher_factory,
            discovery_observer,
            clock,
            #[cfg(feature = "otlp")]
            metrics,
        },
    )
}
