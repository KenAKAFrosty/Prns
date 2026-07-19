use std::path::PathBuf;
use std::sync::mpsc::{self, Sender};

use personal_rns::interface_discovery::{
    DiscoveryArchive, DiscoveryArchiveError, DiscoveryArchiveRecord, DiscoveryCatalogUpdate,
    LoadedDiscoveryArchive,
};
use personal_rns::TokioDiscoveryEvent;

pub(super) async fn load(path: PathBuf) -> Option<LoadedDiscoveryArchive> {
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

pub(super) struct ArchiveSink {
    path: PathBuf,
    records: Sender<DiscoveryArchiveRecord>,
}

impl ArchiveSink {
    pub(super) fn record(&self, event: &TokioDiscoveryEvent<'_>) {
        let Some(record) = archive_record(event) else {
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

pub(super) struct ArchiveWorker {
    path: PathBuf,
    task: tokio::task::JoinHandle<()>,
}

impl ArchiveWorker {
    pub(super) async fn finish(self) {
        if let Err(error) = self.task.await {
            tracing::warn!(
                event = "interface_discovery_archive_worker_failed",
                path = %self.path.display(),
                error = %error,
            );
        }
    }
}

pub(super) fn start(mut archive: DiscoveryArchive) -> (ArchiveSink, ArchiveWorker) {
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
        ArchiveSink {
            path: path.clone(),
            records,
        },
        ArchiveWorker { path, task },
    )
}

pub(super) fn archive_record(event: &TokioDiscoveryEvent<'_>) -> Option<DiscoveryArchiveRecord> {
    let TokioDiscoveryEvent::CatalogUpdated { update, record } = event else {
        return None;
    };
    match update {
        DiscoveryCatalogUpdate::Added { .. } | DiscoveryCatalogUpdate::Refreshed { .. } => {}
        DiscoveryCatalogUpdate::IgnoredOutOfOrder { .. } => return None,
    }
    Some(DiscoveryArchiveRecord::from(*record))
}
