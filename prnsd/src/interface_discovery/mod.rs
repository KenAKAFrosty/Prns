use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};

use personal_rns::config::{DaemonPlan, PlannedMedium};
use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::interface_discovery::{
    DiscoveryArchive, DiscoveryArchiveError, DiscoveryArchiveRecord, DiscoveryCatalogRefresh,
    DiscoveryCatalogUpdate, LoadedDiscoveryArchive, DISCOVERED_INTERFACES_FILE,
};
use personal_rns::interfaces::InterfaceOriginKind;
use personal_rns::reactor::impls::tokio_reactor::TokioHost;
use personal_rns::routing::announce::AnnounceObservation;
use personal_rns::runtime::TokioPrnsHandle;
use personal_rns::{
    DiscoveryIngressOutcome, TokioDiscoveryEvent, TokioDiscoveryIngress, TokioInterfaceDiscovery,
};
use tokio::sync::oneshot;

pub(crate) mod publication;

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
                PlannedMedium::TcpClient { host, port, .. }
                | PlannedMedium::BackboneClient { host, port } => {
                    if let Err(error) = service.reserve_endpoint(host, *port) {
                        tracing::error!(
                            event = "interface_discovery_endpoint_reservation_failed",
                            host,
                            port,
                            error = %error,
                        );
                        return None;
                    }
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

    pub fn spawn(self, handle: TokioPrnsHandle, clock: TokioHost) -> RunningDiscovery {
        let (shutdown, shutdown_requested) = oneshot::channel();
        let task = tokio::spawn(self.run(handle, clock, shutdown_requested));
        RunningDiscovery { shutdown, task }
    }

    async fn run(
        mut self,
        handle: TokioPrnsHandle,
        clock: TokioHost,
        shutdown: oneshot::Receiver<()>,
    ) {
        let (archive_sink, archive_worker) = match load_discovery_archive(self.archive_path).await {
            Some(loaded) => {
                self.service.seed_catalog(loaded.catalog);
                let (sink, worker) = start_archive_worker(loaded.archive);
                (Some(sink), Some(worker))
            }
            None => (None, None),
        };
        tokio::select! {
            () = self.service.run(handle, clock, move |event| {
                trace_discovery_event(&event);
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
}

impl RunningDiscovery {
    pub async fn shutdown(self) {
        let _ = self.shutdown.send(());
        if let Err(error) = self.task.await {
            tracing::warn!(event = "interface_discovery_task_failed", error = %error);
        }
    }
}

async fn load_discovery_archive(path: PathBuf) -> Option<LoadedDiscoveryArchive> {
    let requested_path = path.clone();
    let loaded = tokio::task::spawn_blocking(move || {
        let loaded = DiscoveryArchive::load(path)?;
        let persist_error = loaded.archive.persist().err();
        Ok::<_, DiscoveryArchiveError>((loaded, persist_error))
    })
    .await;
    match loaded {
        Ok(Ok((loaded, persist_error))) => {
            tracing::info!(
                event = "interface_discovery_archive_loaded",
                path = %loaded.archive.path().display(),
                interfaces = loaded.archive.len(),
                file_state = ?loaded.file_state,
            );
            if let Some(error) = persist_error {
                tracing::warn!(
                    event = "interface_discovery_archive_write_failed",
                    path = %loaded.archive.path().display(),
                    error = %error,
                );
            }
            Some(loaded)
        }
        Ok(Err(error)) => {
            tracing::warn!(event = "interface_discovery_archive_unavailable", error = %error);
            None
        }
        Err(error) => {
            tracing::warn!(
                event = "interface_discovery_archive_load_worker_failed",
                path = %requested_path.display(),
                error = %error,
            );
            None
        }
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

struct DiscoveryArchiveSink {
    path: PathBuf,
    records: Sender<DiscoveryArchiveRecord>,
}

impl DiscoveryArchiveSink {
    fn record(&self, event: &TokioDiscoveryEvent<'_>) {
        let Some(record) = discovery_archive_record(event) else {
            return;
        };
        if self.records.send(record).is_err() {
            tracing::warn!(
                event = "interface_discovery_archive_writer_unavailable",
                path = %self.path.display(),
            );
        }
    }
}

struct DiscoveryArchiveWorker {
    path: PathBuf,
    task: tokio::task::JoinHandle<()>,
}

impl DiscoveryArchiveWorker {
    async fn finish(self) {
        if let Err(error) = self.task.await {
            tracing::warn!(
                event = "interface_discovery_archive_worker_failed",
                path = %self.path.display(),
                error = %error,
            );
        }
    }
}

fn start_archive_worker(
    mut archive: DiscoveryArchive,
) -> (DiscoveryArchiveSink, DiscoveryArchiveWorker) {
    let path = archive.path().to_path_buf();
    let (records, receiver) = mpsc::channel();
    let task = tokio::task::spawn_blocking(move || {
        for record in receiver {
            if let Err(error) = archive.record(record) {
                tracing::warn!(
                    event = "interface_discovery_archive_write_failed",
                    path = %archive.path().display(),
                    error = %error,
                );
            }
        }
    });
    (
        DiscoveryArchiveSink {
            path: path.clone(),
            records,
        },
        DiscoveryArchiveWorker { path, task },
    )
}

fn discovery_archive_record(event: &TokioDiscoveryEvent<'_>) -> Option<DiscoveryArchiveRecord> {
    let TokioDiscoveryEvent::CatalogUpdated { update, record } = event else {
        return None;
    };
    match update {
        DiscoveryCatalogUpdate::Added { .. } | DiscoveryCatalogUpdate::Refreshed { .. } => {}
        DiscoveryCatalogUpdate::IgnoredOutOfOrder { .. } => return None,
    }
    Some(DiscoveryArchiveRecord::from(*record))
}

fn trace_discovery_event(event: &TokioDiscoveryEvent<'_>) {
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
    }
}

#[cfg(test)]
mod tests;
