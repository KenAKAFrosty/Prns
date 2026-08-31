use std::path::{Path, PathBuf};

use personal_rns::config::{DaemonPlan, PlannedMedium};
use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::interface_discovery::DISCOVERED_INTERFACES_FILE;
use personal_rns::manifold::tokio::TokioHost;
use personal_rns::routing::announce::AnnounceObservation;
use personal_rns::runtime::PrnsNodeHandle;
use personal_rns::{TokioDiscoveryEvent, TokioDiscoveryIngress, TokioInterfaceDiscovery};
use tokio::sync::oneshot;

mod archive;
mod bootstrap;
mod events;

pub(crate) mod publication;

pub use bootstrap::{BootstrapInterfaces, MonitoredInterfaces};

pub struct PreparedDiscovery {
    service: TokioInterfaceDiscovery,
    ingress: TokioDiscoveryIngress,
    archive_path: PathBuf,
}

impl PreparedDiscovery {
    pub fn from_plan(
        plan: &DaemonPlan,
        network_identity: Option<Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>>,
        config_dir: &Path,
    ) -> Option<Self> {
        plan.discovery.enabled_policy()?;
        let (mut service, ingress) =
            TokioInterfaceDiscovery::new(plan.discovery.clone(), network_identity);
        for interface in &plan.interfaces {
            match &interface.medium {
                PlannedMedium::TcpClient { connection, .. }
                | PlannedMedium::BackboneClient { connection } => {
                    if let Err(error) = service.reserve_endpoint(&connection.host, connection.port)
                    {
                        tracing::error!(
                            event = "interface_discovery_endpoint_reservation_failed",
                            host = connection.host,
                            port = connection.port,
                            error = %error,
                        );
                        return None;
                    }
                }
                PlannedMedium::AutoWifi(_)
                | PlannedMedium::TcpServer { .. }
                | PlannedMedium::Udp { .. }
                | PlannedMedium::Serial { .. }
                | PlannedMedium::Kiss { .. }
                | PlannedMedium::Ax25Kiss { .. }
                | PlannedMedium::Rnode { .. }
                | PlannedMedium::RnodeMulti { .. }
                | PlannedMedium::Backbone { .. }
                | PlannedMedium::Pipe { .. }
                | PlannedMedium::I2p { .. }
                | PlannedMedium::Weave { .. }
                | PlannedMedium::PrnsUsbAuto
                | PlannedMedium::PrnsBluetoothAuto { .. }
                | PlannedMedium::PrnsWebSocketClient { .. }
                | PlannedMedium::PrnsWebSocketServer { .. } => {}
            }
        }
        Some(Self {
            service,
            ingress,
            archive_path: config_dir.join(DISCOVERED_INTERFACES_FILE),
        })
    }

    pub fn observer(&self) -> DiscoveryObserver {
        DiscoveryObserver {
            ingress: self.ingress.clone(),
        }
    }

    pub fn spawn(
        self,
        handle: PrnsNodeHandle,
        clock: TokioHost,
        bootstrap: Option<BootstrapInterfaces>,
    ) -> RunningDiscovery {
        let (shutdown, shutdown_requested) = oneshot::channel();
        let (capacities, capacity_events) = tokio::sync::watch::channel(None);
        let bootstrap_task =
            bootstrap.map(|bootstrap| tokio::spawn(bootstrap.run(handle.clone(), capacity_events)));
        let task = tokio::spawn(self.run(handle, clock, shutdown_requested, capacities));
        RunningDiscovery {
            shutdown,
            task,
            bootstrap_task,
        }
    }

    async fn run(
        mut self,
        handle: PrnsNodeHandle,
        clock: TokioHost,
        shutdown: oneshot::Receiver<()>,
        capacities: tokio::sync::watch::Sender<Option<bootstrap::AutoConnectCapacity>>,
    ) {
        let (archive_sink, archive_worker) = match archive::load(self.archive_path).await {
            Some(loaded) => {
                self.service.seed_catalog(loaded.catalog);
                let (sink, worker) = archive::start(loaded.archive);
                (Some(sink), Some(worker))
            }
            None => (None, None),
        };
        tokio::select! {
            () = self.service.run(handle, clock, move |event| {
                events::trace(&event);
                if let TokioDiscoveryEvent::AutoConnectCapacity { online, maximum } = &event {
                    capacities.send_replace(Some(bootstrap::AutoConnectCapacity {
                        online: *online,
                        maximum: *maximum,
                    }));
                }
                if let Some(archive_sink) = archive_sink.as_ref() {
                    archive_sink.record(&event);
                }
            }) => {}
            _ = shutdown => {}
        }
        if let Some(archive_worker) = archive_worker {
            archive_worker.finish().await;
        }
    }
}

pub struct RunningDiscovery {
    shutdown: oneshot::Sender<()>,
    task: tokio::task::JoinHandle<()>,
    bootstrap_task: Option<tokio::task::JoinHandle<()>>,
}

impl RunningDiscovery {
    pub async fn shutdown(self) {
        let _ = self.shutdown.send(());
        if let Err(error) = self.task.await {
            tracing::warn!(event = "interface_discovery_task_failed", error = %error);
        }
        if let Some(task) = self.bootstrap_task {
            if let Err(error) = task.await {
                tracing::warn!(event = "bootstrap_interface_task_failed", error = %error);
            }
        }
    }
}

#[derive(Clone)]
pub struct DiscoveryObserver {
    ingress: TokioDiscoveryIngress,
}

impl DiscoveryObserver {
    pub fn observe(&self, observation: AnnounceObservation<'_>) {
        events::trace_ingress(self.ingress.observe(observation));
    }
}

#[cfg(test)]
mod tests;
