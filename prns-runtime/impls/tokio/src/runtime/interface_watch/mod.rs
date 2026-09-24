use futures_util::stream::{FuturesUnordered, StreamExt};
use futures_util::FutureExt;
use std::collections::HashMap;
use std::future::{poll_fn, Future};
use std::panic::AssertUnwindSafe;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::Poll;
use std::time::Duration;
use tokio::time::Instant;

use tokio::io::AsyncWriteExt;
use tokio::sync::oneshot;

use crate::identity::IdentityHash;
use crate::interfaces::{
    ConnectionState, InterfaceGravity, InterfaceId, InterfaceMode, Membership, PeerDetails,
};
use crate::remote_control::{
    RemoteControlControllerGrantTable, RemoteControlRequestKind, RemoteControlStreamEvent,
    RemoteControlStreamEventError, REMOTE_CONTROL_STREAM_EVENT_LEN,
};
use crate::routing::links::channel::byte_stream::StreamId;
use crate::routing::links::LinkId;

use super::node_facade::PrnsNodeHandle;

const MAX_INTERFACE_WATCHES: usize = 8;
const WATCH_POLL_INTERVAL: Duration = Duration::from_millis(500);
const WATCH_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);
const WATCH_WRITE_DEADLINE: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WatchReserveFailure {
    Full,
    Duplicate,
}

pub(super) enum WatchAdmission {
    Admitted,
    Withdrawn,
}

struct WatchLease;

pub(super) struct WatchReservation {
    link_id: LinkId,
    stream_id: StreamId,
    lease: Arc<WatchLease>,
    start: oneshot::Sender<()>,
}

impl WatchReservation {
    pub(super) fn start(self) {
        let _ = self.start.send(());
    }
}

struct Watch {
    lease: Arc<WatchLease>,
    controller: IdentityHash,
    stop: WatchCancellation,
}

struct WatchCompletion {
    link_id: LinkId,
    stream_id: StreamId,
    lease: Arc<WatchLease>,
}

type WatchWorker = Pin<Box<dyn Future<Output = WatchCompletion> + Send>>;

enum WatchCancellation {
    Available(oneshot::Sender<()>),
    Requested,
}

impl Watch {
    fn stop(&mut self) {
        if let WatchCancellation::Available(stop) =
            std::mem::replace(&mut self.stop, WatchCancellation::Requested)
        {
            let _ = stop.send(());
        }
    }
}

pub(super) struct InterfaceWatchRegistry {
    watches: Mutex<HashMap<(LinkId, StreamId), Watch>>,
    workers: Mutex<FuturesUnordered<WatchWorker>>,
}

impl Default for InterfaceWatchRegistry {
    fn default() -> Self {
        Self {
            watches: Mutex::new(HashMap::new()),
            workers: Mutex::new(FuturesUnordered::new()),
        }
    }
}

impl InterfaceWatchRegistry {
    pub(super) async fn run(&self) -> core::convert::Infallible {
        loop {
            self.next_completion().await;
        }
    }

    pub(super) async fn next_completion(&self) {
        let completion = poll_fn(|cx| {
            let mut workers = self
                .workers
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            // The owned watch driver is the sole poller. Release the lock after each poll.
            match workers.poll_next_unpin(cx) {
                Poll::Ready(Some(completion)) => Poll::Ready(completion),
                Poll::Ready(None) | Poll::Pending => Poll::Pending,
            }
        })
        .await;
        let mut watches = self
            .watches
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let key = (completion.link_id, completion.stream_id);
        if watches
            .get(&key)
            .is_some_and(|watch| Arc::ptr_eq(&watch.lease, &completion.lease))
        {
            watches.remove(&key);
        }
    }

    pub(super) fn admission(&self, reservation: &WatchReservation) -> WatchAdmission {
        let watches = self
            .watches
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        match watches.get(&(reservation.link_id, reservation.stream_id)) {
            Some(watch)
                if Arc::ptr_eq(&watch.lease, &reservation.lease)
                    && matches!(watch.stop, WatchCancellation::Available(_)) =>
            {
                WatchAdmission::Admitted
            }
            _ => WatchAdmission::Withdrawn,
        }
    }
    pub(super) fn reserve(
        &self,
        node: PrnsNodeHandle,
        link_id: LinkId,
        stream_id: StreamId,
        controller: IdentityHash,
    ) -> Result<WatchReservation, WatchReserveFailure> {
        let mut watches = self
            .watches
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if watches.contains_key(&(link_id, stream_id)) {
            return Err(WatchReserveFailure::Duplicate);
        }
        if watches.len() >= MAX_INTERFACE_WATCHES {
            return Err(WatchReserveFailure::Full);
        }
        let lease = Arc::new(WatchLease);
        let (start, ready) = oneshot::channel();
        let (stop, mut cancellation) = oneshot::channel();
        let worker_lease = lease.clone();
        let worker = Box::pin(async move {
            let watched = async {
                tokio::select! {
                    biased;
                    _ = &mut cancellation => {},
                    started = ready => {
                        if started.is_ok() {
                            run_watch(&node, link_id, stream_id, cancellation).await;
                        }
                    }
                }
            };
            if AssertUnwindSafe(watched).catch_unwind().await.is_err() {
                node.close_link(link_id);
            }
            WatchCompletion {
                link_id,
                stream_id,
                lease: worker_lease,
            }
        });
        watches.insert(
            (link_id, stream_id),
            Watch {
                lease: lease.clone(),
                controller,
                stop: WatchCancellation::Available(stop),
            },
        );
        self.workers
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(worker);
        Ok(WatchReservation {
            link_id,
            stream_id,
            lease,
            start,
        })
    }

    pub(super) fn cancel(&self, reservation: &WatchReservation) {
        let mut watches = self
            .watches
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(watch) = watches.get_mut(&(reservation.link_id, reservation.stream_id)) {
            if Arc::ptr_eq(&watch.lease, &reservation.lease) {
                watch.stop();
            }
        }
    }

    pub(super) fn cancel_link(&self, link_id: LinkId) {
        let mut watches = self
            .watches
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        for ((link, _), watch) in watches.iter_mut() {
            if *link == link_id {
                watch.stop();
            }
        }
    }

    pub(super) fn cancel_all(&self) {
        let mut watches = self
            .watches
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        for watch in watches.values_mut() {
            watch.stop();
        }
    }

    pub(super) fn reconcile_grants(&self, grants: &impl RemoteControlControllerGrantTable) {
        let mut watches = self
            .watches
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        for watch in watches.values_mut() {
            if !grants.grant_for(&watch.controller).is_some_and(|grant| {
                grant
                    .effective_requests()
                    .supports(RemoteControlRequestKind::WatchInterfaces)
            }) {
                watch.stop();
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct StableInterface {
    id: InterfaceId,
    mode: InterfaceMode,
    gravity: InterfaceGravity,
    destinations: u32,
    links: u32,
    transported_links: u32,
    connection: ConnectionState,
    failure_reason: Option<&'static str>,
    membership: Membership,
    details: PeerDetails,
}

fn stable_interfaces(node: &PrnsNodeHandle) -> Vec<StableInterface> {
    let mut snapshots = node
        .interfaces()
        .into_iter()
        .map(|snapshot| StableInterface {
            id: snapshot.id,
            mode: snapshot.mode,
            gravity: snapshot.gravity,
            destinations: snapshot.destinations,
            links: snapshot.links,
            transported_links: snapshot.transported_links,
            connection: snapshot.connection,
            failure_reason: snapshot.failure_reason,
            membership: snapshot.membership,
            details: snapshot.details,
        })
        .collect::<Vec<_>>();
    snapshots.sort_unstable_by_key(|snapshot| *snapshot.id.as_bytes());
    snapshots
}

#[derive(Debug)]
enum WatchSendError {
    Cancelled,
    Frame(RemoteControlStreamEventError),
    Io(std::io::Error),
    Deadline,
}

impl std::fmt::Display for WatchSendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => f.write_str("interface watch cancelled"),
            Self::Frame(error) => write!(f, "interface watch encoding failed: {error:?}"),
            Self::Io(error) => write!(f, "interface watch write failed: {error}"),
            Self::Deadline => f.write_str("interface watch write deadline exceeded"),
        }
    }
}

async fn send_event(
    writer: &mut super::node_facade::ByteStreamWriter,
    event: RemoteControlStreamEvent,
    cancellation: &mut oneshot::Receiver<()>,
) -> Result<(), WatchSendError> {
    let mut frame = [0; REMOTE_CONTROL_STREAM_EVENT_LEN];
    event
        .write_into(&mut frame)
        .map_err(WatchSendError::Frame)?;
    tokio::select! {
        biased;
        _ = cancellation => Err(WatchSendError::Cancelled),
        sent = tokio::time::timeout(WATCH_WRITE_DEADLINE, writer.write_all(&frame)) => {
            sent.map_err(|_| WatchSendError::Deadline)?.map_err(WatchSendError::Io)
        }
    }
}

async fn run_watch(
    node: &PrnsNodeHandle,
    link_id: LinkId,
    stream_id: StreamId,
    mut cancellation: oneshot::Receiver<()>,
) {
    let mut writer = node.byte_stream_writer(link_id, stream_id);
    let result = run_watch_events(node, &mut writer, &mut cancellation).await;
    if let Err(_error) = result {
        #[cfg(feature = "tracing")]
        tracing::debug!(target: "prns.runtime", event = "interface_watch_stopped", ?link_id, ?stream_id, error = %_error);
    }
    // Cancellation and write failure both finish the stream, within a bounded deadline.
    match tokio::time::timeout(WATCH_WRITE_DEADLINE, writer.shutdown()).await {
        Ok(Ok(())) => {}
        Ok(Err(_)) | Err(_) => {
            node.close_link(link_id);
        }
    }
}

async fn run_watch_events(
    node: &PrnsNodeHandle,
    writer: &mut super::node_facade::ByteStreamWriter,
    cancellation: &mut oneshot::Receiver<()>,
) -> Result<(), WatchSendError> {
    let mut sequence = 1;
    // Capture before invalidation: changes during the initial send must remain visible.
    let mut previous = stable_interfaces(node);
    send_event(
        writer,
        RemoteControlStreamEvent::ResyncRequired { sequence },
        cancellation,
    )
    .await?;
    let mut last_sent = Instant::now();
    let mut interval = tokio::time::interval(WATCH_POLL_INTERVAL);
    interval.tick().await;
    loop {
        tokio::select! {
            biased;
            _ = &mut *cancellation => return Err(WatchSendError::Cancelled),
            _ = interval.tick() => {
                let current = stable_interfaces(node);
                let event = if current != previous {
                    previous = current;
                    Some(RemoteControlStreamEvent::ResyncRequired {
                        sequence: sequence.wrapping_add(1),
                    })
                } else if last_sent.elapsed() >= WATCH_HEARTBEAT_INTERVAL {
                    Some(RemoteControlStreamEvent::Heartbeat {
                        sequence: sequence.wrapping_add(1),
                    })
                } else {
                    None
                };
                if let Some(event) = event {
                    sequence = event.sequence();
                    send_event(writer, event, cancellation).await?;
                    last_sent = Instant::now();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
