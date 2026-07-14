//! The daemon's persistence policy over the runtime's flush/seed mechanism: seed at boot, flush
//! on a tight interval, flush once more on shutdown.
//! RNS 1.3.5 persists every 12 hours (`Reticulum.PERSIST_INTERVAL`) because each of its persists
//! is an unconditional full rewrite; our interval flush skips regions whose sealed bytes did not
//! change, so a quiet tick costs 12 bytes of timebase and the interval affords 5 minutes — the
//! crash-loss window shrinks accordingly.
//! A shared-instance client neither seeds nor flushes (the reference's
//! `is_connected_to_shared_instance` gate): the instance owns the tables, so main only starts
//! this module's loops after winning the instance role or running standalone.

use core::time::Duration;
use std::path::{Path, PathBuf};

use personal_rns::persistence::FileStore;
use personal_rns::runtime::{FlushError, FlushMark, TokioPrnsHandle};

pub const PERSIST_INTERVAL: Duration = Duration::from_secs(5 * 60);

/// The snapshot directory: a `prns` subdir of the RNS storage dir, because a config dir shared
/// with stock RNS keeps its own msgpack `storage/tunnels`, and our sealed region of the same
/// name must never clobber it.
pub fn store_dir(storage_dir: &Path) -> PathBuf {
    storage_dir.join("prns")
}

pub async fn persist_loop(handle: TokioPrnsHandle, mut store: FileStore, interval: Duration) {
    let mut mark = FlushMark::default();
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    // The interval's first tick fires immediately, and boot just seeded from these bytes.
    ticker.tick().await;
    loop {
        ticker.tick().await;
        match handle.flush_changed_to_store(&mut store, &mut mark).await {
            Ok(_) => {}
            Err(FlushError::NodeStopped) => break,
            Err(FlushError::Store(error)) => eprintln!("RNSD_PERSIST_ERROR {error:?}"),
        }
    }
}

/// Resolves when the daemon should exit, after landing one final unconditional flush — the
/// reference's `Transport.exit_handler`. Runs as a sibling `select!` branch of `run()`, never
/// after it: the flush needs the reactor alive to serialize the snapshot.
pub async fn flush_on_shutdown(handle: TokioPrnsHandle, store_dir: Option<PathBuf>) {
    shutdown_signal().await;
    if let Some(dir) = store_dir {
        let mut store = FileStore::new(&dir);
        match handle.flush_to_store(&mut store).await {
            Ok(_) => println!("RNSD_PERSISTED"),
            Err(error) => eprintln!("RNSD_PERSIST_ERROR {error:?}"),
        }
    }
    println!("RNSD_SHUTDOWN");
}

#[cfg(unix)]
async fn shutdown_signal() {
    let mut terminate =
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(terminate) => terminate,
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
                return;
            }
        };
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = terminate.recv() => {}
    }
}

#[cfg(not(unix))]
async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}
