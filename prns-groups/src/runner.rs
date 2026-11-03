use crate::client::{Command, Reply};
use crate::{Client, Clock, Error, NetworkConfig};
use futures_util::{stream::FuturesUnordered, StreamExt};
use prns_groups_core::sync::{
    PathCapabilities, PeerSchedule, SyncActivity, SyncIssue, SyncPhase, SyncPolicy, SyncTuning,
};
use prns_groups_reticulum::{
    push_storage, reconcile_storage, serve_storage_with_options, Network, RemoteError, RequestJob,
    SyncBudget, SyncOptions, SyncStorage,
};
use prns_groups_service::{
    Endpoint, EnvelopeId, EnvelopeInfo, Fault, GroupId, Incoming, InventoryCursor, InventoryKind,
    InventoryPage, InventorySnapshot, RecordId,
};
use prns_groups_store::StoreIncarnation;
use prns_groups_store_redb::RedbError;
use std::{future::Future, pin::Pin, sync::Arc, time::Duration};
use tokio::sync::{mpsc, watch};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunnerControl {
    Running,
    Stop,
}
#[derive(Clone, Copy, Debug)]
pub struct RunnerOptions {
    /// Idle recovery polling floor. Durable work and authenticated recovery use
    /// the interface active spacing and `retry_initial` floor instead.
    pub poll_interval: Duration,
    pub retry_initial: Duration,
    /// Minimum backoff ceiling; raised when interface policy requires more time.
    pub retry_max: Duration,
    /// Hard work limits, preserved even when the path policy suggests more work.
    pub budget: SyncBudget,
}

/// Caller-owned future. Incoming requests and at most two outgoing exchanges
/// progress concurrently through the same serialized store lane. Dropping this
/// future releases group links; it never stops the borrowed Reticulum node.
pub async fn run(
    client: Client,
    network: Arc<Network>,
    mut jobs: mpsc::Receiver<RequestJob>,
    options: RunnerOptions,
    clock: Arc<dyn Clock>,
    mut stop: watch::Receiver<RunnerControl>,
) -> Result<(), Error> {
    if options.poll_interval.is_zero()
        || options.retry_initial.is_zero()
        || options.retry_max < options.retry_initial
        || options.budget.max_requests < 2
        || options.budget.max_envelopes == 0
        || options.budget.max_bytes == 0
        || tokio::time::Instant::now()
            .checked_add(options.poll_interval)
            .is_none()
        || tokio::time::Instant::now()
            .checked_add(options.retry_max)
            .is_none()
    {
        return Err(Error::Configuration);
    }
    if *stop.borrow() == RunnerControl::Stop {
        return Ok(());
    }
    let support = crate::ProfileSupport::new(
        network
            .limits()
            .max_envelope_bytes()
            .map_err(|error| Error::Transport(error.into()))?,
        usize::MAX,
    )
    .map_err(|_| Error::Configuration)?;
    tokio::select! {
        result = retry_busy(options.retry_initial, || client.bind_profile_support(support)) => result?,
        _ = stop.wait_for(|control| *control == RunnerControl::Stop) => return Ok(()),
    }
    let config = tokio::select! {
        result = retry_busy(options.retry_initial, || client.network_config()) => result?,
        _ = stop.wait_for(|control| *control == RunnerControl::Stop) => return Ok(()),
    };
    if network.local_identity() != config.local_reticulum_identity {
        return Err(Error::Configuration);
    }
    for peer in &config.peers {
        if network.peer(peer.endpoint) != Some(peer) {
            return Err(Error::Configuration);
        }
    }
    tokio::select! {
        result = network.authorize_peers() => result.map_err(|error| Error::Transport(error.into()))?,
        _ = stop.wait_for(|control| *control == RunnerControl::Stop) => return Ok(()),
    }
    let _progress_lease = client.progress.start()?;
    client.progress.configure(std::slice::from_ref(&config));
    let client = client.for_group(config.group).at_control(config.control);
    let inbound_client = SyncClient {
        client: client.clone(),
        config: config.clone(),
    };
    let outbound_client = SyncClient { client, config };
    let recoveries = crate::schedule::Recoveries::default();
    let inbound = async {
        let mut storage = inbound_client;
        while let Some(job) = jobs.recv().await {
            // Peer and malformed-packet failures apply only to this request.
            // Outbound attempts remain independent and recover on later polls.
            let receive_options = SyncOptions {
                incarnation: storage.config.incarnation,
                clock: clock.now(),
                budget: options.budget,
            };
            if serve_storage_with_options(&network, &mut storage, &job, receive_options)
                .await
                .is_ok()
            {
                if let Some(hint) =
                    crate::schedule::authenticated_recovery(&network, &storage, &job).await
                {
                    recoveries.offer(hint);
                }
            }
        }
        Ok(())
    };
    let outbound = outbound(
        &network,
        outbound_client,
        options,
        clock.clone(),
        &recoveries,
    );
    tokio::pin!(inbound, outbound);
    loop {
        if *stop.borrow() == RunnerControl::Stop {
            return Ok(());
        }
        tokio::select! {
            result = &mut inbound => return result,
            result = &mut outbound => return result,
            changed = stop.changed() => if changed.is_err() { return Ok(()); },
        }
    }
}

const MAX_OUTBOUND_EXCHANGES: usize = 2;
const STORE_WAIT: Duration = Duration::from_secs(10);
type SingleAttempt<'a> =
    Pin<Box<dyn Future<Output = (usize, Result<AttemptReport, Error>)> + Send + 'a>>;

struct Schedule {
    peer: Endpoint,
    diagnostic: Option<(GroupId, prns_host::IdentityHash)>,
    due: tokio::time::Instant,
    not_before: tokio::time::Instant,
    state: PeerSchedule,
    round: u32,
    active: bool,
    failed: bool,
    recovered: bool,
    push: crate::delivery::PushState,
    observer: Option<crate::progress::PeerObserver>,
    pending: Option<bool>,
    issue: Option<SyncIssue>,
}
impl Schedule {
    fn publish(&self) {
        if let Some(observer) = &self.observer {
            let phase = if self.active {
                if self.pending == Some(true) {
                    SyncPhase::Connecting
                } else {
                    SyncPhase::Synchronizing
                }
            } else if let Some(issue) = self.issue {
                crate::progress::issue_phase(issue)
            } else if self.failed {
                SyncPhase::RetryScheduled
            } else {
                SyncPhase::Waiting
            };
            observer.update(
                phase,
                self.pending,
                (!self.active).then_some(self.due),
                self.state.failures(),
                self.issue,
            );
        }
    }
    fn completed(
        &mut self,
        now: tokio::time::Instant,
        tuning: SyncTuning,
        activity: SyncActivity,
        local: Endpoint,
    ) {
        self.active = false;
        self.failed = activity == SyncActivity::Failed;
        self.recovered = false;
        self.round = self.round.wrapping_add(1);
        let delay =
            self.state
                .delay_millis(tuning, activity, peer_jitter(local, self.peer, self.round));
        self.due = now + Duration::from_millis(delay);
        self.not_before =
            now + Duration::from_millis(tuning.active_poll_millis.max(tuning.retry_initial_millis));
        self.publish();
        if let Some((group, identity)) = self.diagnostic {
            crate::diagnostics::schedule(
                &group.0,
                identity.as_bytes(),
                &self.state,
                self.due,
                self.not_before,
                crate::diagnostics::Decision {
                    category: "completed",
                    activity: Some(activity),
                    pending: None,
                    fresh: false,
                    repair: false,
                    accelerated: false,
                },
            );
        }
    }
}

// Scheduling-only mixing; neither randomness nor a security/identity claim.
fn peer_jitter(local: Endpoint, peer: Endpoint, round: u32) -> u32 {
    local
        .id
        .0
        .iter()
        .chain(peer.id.0.iter())
        .fold(round ^ 0x811c_9dc5, |value, byte| {
            (value ^ u32::from(*byte)).wrapping_mul(0x0100_0193)
        })
}

fn millis_ceil(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos().div_ceil(1_000_000)).unwrap_or(u64::MAX)
}

fn runner_tuning(policy: SyncPolicy, path: PathCapabilities, options: RunnerOptions) -> SyncTuning {
    let mut tuning = policy.tuning(path);
    tuning.active_poll_millis = tuning
        .active_poll_millis
        .max(millis_ceil(options.retry_initial));
    tuning.idle_poll_millis = tuning
        .idle_poll_millis
        .max(tuning.active_poll_millis)
        .max(millis_ceil(options.poll_interval));
    tuning.max_idle_millis = tuning.max_idle_millis.max(tuning.idle_poll_millis);
    tuning.retry_initial_millis = tuning
        .retry_initial_millis
        .max(millis_ceil(options.retry_initial));
    tuning.retry_max_millis = tuning
        .retry_max_millis
        .max(millis_ceil(options.retry_max))
        .max(tuning.active_poll_millis)
        .max(tuning.idle_poll_millis)
        .max(tuning.retry_initial_millis);
    tuning
}

#[cfg(test)]
fn next_due(peers: &[Schedule], next_peer: usize, now: tokio::time::Instant) -> Option<usize> {
    (0..peers.len())
        .map(|offset| (next_peer + offset) % peers.len())
        .find(|index| !peers[*index].active && peers[*index].due <= now)
}

pub(crate) struct AttemptReport {
    pub(crate) path: PathCapabilities,
    pub(crate) activity: SyncActivity,
    pub(crate) unsupported: bool,
    pub(crate) push_exchanged: bool,
    pub(crate) issue: Option<SyncIssue>,
}

#[allow(clippy::too_many_arguments)]
async fn attempt(
    network: &Network,
    mut store: SyncClient,
    options: RunnerOptions,
    clock: Arc<dyn Clock>,
    index: usize,
    peer: Endpoint,
    pending: Option<EnvelopeInfo>,
    push_enabled: bool,
) -> (usize, Result<AttemptReport, Error>) {
    let network = network.clone().with_push_enabled(push_enabled);
    let sync_options = SyncOptions {
        incarnation: store.config.incarnation,
        clock: clock.now(),
        budget: options.budget,
    };
    (
        index,
        exchange(
            &network,
            &mut store,
            peer,
            sync_options,
            pending,
            options.retry_initial,
        )
        .await,
    )
}

/// Shared by the standalone and app runners. A successful direct delivery does
/// not open another Link just to poll; inventory repair resumes on later turns.
pub(crate) async fn exchange(
    network: &Network,
    store: &mut SyncClient,
    peer: Endpoint,
    options: SyncOptions,
    pending: Option<EnvelopeInfo>,
    retry_delay: Duration,
) -> Result<AttemptReport, Error> {
    let binding = network.peer(peer).ok_or(Error::Configuration)?;
    let mut path = network.path_capabilities(binding).await.unwrap_or_default();
    let group = store.config.group;
    let mut unsupported = false;
    if let Some(info) = pending {
        let mut trace = crate::diagnostics::DeliveryTrace::start(
            &group.0,
            &info.record.0,
            binding.identity.as_bytes(),
            info.bytes,
        );
        match push_storage(network, store, peer, group, info, options).await {
            Ok(report) if report.supported => {
                trace.finish(
                    if report.rejected > 0 {
                        "rejected"
                    } else {
                        "submitted"
                    },
                    report.accepted_receipts,
                );
                return Ok(AttemptReport {
                    path: report.path,
                    activity: if report.rejected > 0 || report.submitted == 0 {
                        SyncActivity::Failed
                    } else {
                        SyncActivity::Progress
                    },
                    unsupported: false,
                    push_exchanged: true,
                    issue: (report.rejected > 0 || report.submitted == 0)
                        .then_some(SyncIssue::Rejected),
                });
            }
            Ok(report) => {
                trace.finish("temporarily_pull_only", 0);
                path = report.path;
                unsupported = true;
            }
            Err(error) if push_requires_pull(&error) => {
                // Submit has more framing overhead than Fetch, and a remote
                // endpoint may advertise a smaller Submit limit. Retain the
                // obligation and use the bounded recovery protocol instead.
                trace.finish("deferred_to_pull", 0);
            }
            Err(error) => {
                let error = Error::from(error);
                trace.finish(crate::diagnostics::error_category(&error), 0);
                return Err(error);
            }
        }
    }
    // An unsupported probe must not be repeated by this fallback exchange.
    let fallback = if unsupported {
        network.clone().with_push_enabled(false)
    } else {
        network.clone()
    };
    let cursor = retry_busy(retry_delay, || store.client.load_cursor(peer)).await?;
    match reconcile_storage(&fallback, store, peer, group, options, cursor).await {
        Ok(report) => {
            retry_busy(retry_delay, || {
                store.client.save_cursor(peer, report.cursor)
            })
            .await?;
            Ok(AttemptReport {
                path: report.path,
                activity: if report.fetched > 0 || report.has_more() {
                    SyncActivity::Progress
                } else {
                    SyncActivity::Idle
                },
                unsupported: unsupported || report.push_supported == Some(false),
                push_exchanged: false,
                issue: None,
            })
        }
        Err(prns_groups_reticulum::Error::Remote(RemoteError::RestartInventory)) => {
            retry_busy(retry_delay, || store.client.save_cursor(peer, None)).await?;
            Ok(AttemptReport {
                path,
                activity: SyncActivity::Progress,
                unsupported,
                push_exchanged: false,
                issue: None,
            })
        }
        Err(error) => {
            let error = Error::from(error);
            crate::diagnostics::emit(
                crate::diagnostics::Event::SyncOutcome,
                crate::diagnostics::error_category(&error),
                0,
            );
            Ok(AttemptReport {
                path,
                activity: SyncActivity::Failed,
                unsupported,
                push_exchanged: false,
                issue: Some(crate::progress::issue(&error)),
            })
        }
    }
}

fn push_requires_pull<E>(error: &prns_groups_reticulum::Error<E>) -> bool {
    matches!(
        error,
        prns_groups_reticulum::Error::Protocol(prns_groups_reticulum::ProtocolError::Oversized)
            | prns_groups_reticulum::Error::Remote(RemoteError::OverBudget)
    )
}

fn completed_attempt(
    index: usize,
    result: Result<AttemptReport, Error>,
    peers: &mut [Schedule],
    policy: SyncPolicy,
    options: RunnerOptions,
    local: Endpoint,
) {
    let report = match result {
        Ok(report) => report,
        Err(error) => {
            crate::diagnostics::emit(
                crate::diagnostics::Event::SyncOutcome,
                crate::diagnostics::error_category(&error),
                0,
            );
            AttemptReport {
                path: PathCapabilities::default(),
                activity: SyncActivity::Failed,
                unsupported: false,
                push_exchanged: false,
                issue: Some(crate::progress::issue(&error)),
            }
        }
    };
    peers[index].issue = report.issue;
    if report.push_exchanged {
        peers[index].push.exchanged();
    }
    if report.unsupported {
        peers[index].push.unsupported(tokio::time::Instant::now());
    }
    peers[index].completed(
        tokio::time::Instant::now(),
        runner_tuning(policy, report.path, options),
        report.activity,
        local,
    );
}

#[allow(clippy::too_many_arguments)]
async fn alongside_attempts<F: Future>(
    action: F,
    attempts: &mut FuturesUnordered<SingleAttempt<'_>>,
    peers: &mut [Schedule],
    policy: SyncPolicy,
    options: RunnerOptions,
    local: Endpoint,
) -> F::Output {
    tokio::pin!(action);
    loop {
        tokio::select! {
            value = &mut action => return value,
            Some((index, result)) = attempts.next(), if !attempts.is_empty() => {
                completed_attempt(index, result, peers, policy, options, local);
            }
        }
    }
}

async fn outbound(
    network: &Network,
    store: SyncClient,
    options: RunnerOptions,
    clock: Arc<dyn Clock>,
    recoveries: &crate::schedule::Recoveries,
) -> Result<(), Error> {
    let mut peers: Vec<_> = store
        .config
        .peers
        .iter()
        .map(|peer| {
            let now = tokio::time::Instant::now();
            Schedule {
                peer: peer.endpoint,
                diagnostic: Some((store.config.group, peer.identity)),
                due: now,
                not_before: now,
                state: PeerSchedule::default(),
                round: 0,
                active: false,
                failed: false,
                recovered: false,
                push: crate::delivery::PushState::default(),
                observer: Some(crate::progress::PeerObserver::new(
                    Arc::clone(&store.client.progress),
                    &store.config,
                    peer.endpoint,
                )),
                pending: None,
                issue: None,
            }
        })
        .collect();
    let mut next_peer = 0usize;
    let mut attempts: FuturesUnordered<SingleAttempt<'_>> = FuturesUnordered::new();
    let idle_tick = Duration::from_millis(
        runner_tuning(network.sync_policy(), PathCapabilities::default(), options)
            .active_poll_millis,
    );
    loop {
        // Recovery repairs received-but-not-yet-emitted receipts even when the
        // source remains offline and no inventory revisit occurs.
        alongside_attempts(
            async {
                let _ = tokio::time::timeout(STORE_WAIT, store.client.maintain(clock.now())).await;
                let _ = tokio::time::timeout(STORE_WAIT, store.client.repair_receipts()).await;
            },
            &mut attempts,
            &mut peers,
            network.sync_policy(),
            options,
            store.config.local,
        )
        .await;
        let hints = recoveries.take();
        let current = if hints.is_empty() {
            None
        } else {
            alongside_attempts(
                tokio::time::timeout(STORE_WAIT, store.client.network_config()),
                &mut attempts,
                &mut peers,
                network.sync_policy(),
                options,
                store.config.local,
            )
            .await
            .ok()
            .and_then(Result::ok)
        };
        for hint in hints {
            let Some(current) = current.as_ref() else {
                hint.trace(crate::schedule::RecoveryDisposition::ScopeUnavailable);
                continue;
            };
            if !hint.current(&store.config) || !hint.current(current) {
                hint.trace(crate::schedule::RecoveryDisposition::ScopeStale);
                continue;
            }
            let Some(peer) = peers.iter_mut().find(|peer| peer.peer == hint.peer) else {
                hint.trace(crate::schedule::RecoveryDisposition::CandidateMissing);
                continue;
            };
            if peer.active {
                hint.trace(crate::schedule::RecoveryDisposition::PeerActive);
                continue;
            }
            peer.recovered |= peer.failed;
            hint.trace(if peer.failed {
                crate::schedule::RecoveryDisposition::Credited
            } else {
                crate::schedule::RecoveryDisposition::NotFailed
            });
        }
        while attempts.len() < MAX_OUTBOUND_EXCHANGES {
            let now = tokio::time::Instant::now();
            let mut selected = None;
            for offset in 0..peers.len() {
                let index = (next_peer + offset) % peers.len();
                if peers[index].active {
                    continue;
                }
                let push = peers[index].push;
                let endpoint = peers[index].peer;
                let pending = if push.enabled(now) {
                    match alongside_attempts(
                        tokio::time::timeout(
                            STORE_WAIT,
                            push.pending(&store.client, endpoint, clock.now()),
                        ),
                        &mut attempts,
                        &mut peers,
                        network.sync_policy(),
                        options,
                        store.config.local,
                    )
                    .await
                    {
                        Ok(Ok(pending)) => pending,
                        Ok(Err(Error::Busy)) | Err(_) => {
                            peers[index].pending = None;
                            peers[index].issue = Some(SyncIssue::StorageBusy);
                            peers[index].publish();
                            continue;
                        }
                        Ok(Err(error)) => return Err(error),
                    }
                } else {
                    None
                };
                let peer = &mut peers[index];
                peer.pending = push.enabled(now).then_some(pending.is_some());
                let now = tokio::time::Instant::now();
                let fresh = pending.is_some_and(|info| peer.push.is_new(info));
                let recovered = std::mem::take(&mut peer.recovered);
                let mut accelerated = false;
                if let Some(evidence) = peer.push.wake_evidence(pending, recovered) {
                    accelerated = crate::schedule::expedite(
                        &peer.state,
                        &mut peer.due,
                        peer.not_before,
                        evidence,
                        now,
                    );
                    if accelerated {
                        trace_schedule(
                            network,
                            &store,
                            peer,
                            crate::diagnostics::Decision {
                                category: if fresh {
                                    "fresh_work"
                                } else if recovered {
                                    "peer_recovered"
                                } else {
                                    "pending_work"
                                },
                                activity: None,
                                pending: Some(true),
                                fresh,
                                repair: push.repair_due(),
                                accelerated,
                            },
                        );
                    }
                }
                if recovered {
                    if let Some((group, identity)) = peer.diagnostic {
                        crate::diagnostics::recovery(
                            &group.0,
                            identity.as_bytes(),
                            crate::schedule::RecoveryDisposition::consumed(
                                pending.is_some(),
                                push.enabled(now),
                                fresh,
                                accelerated,
                            ),
                        );
                    }
                }
                peer.publish();
                if peer.due <= now {
                    let selected_record = pending.filter(|_| !push.repair_due());
                    trace_schedule(
                        network,
                        &store,
                        peer,
                        crate::diagnostics::Decision {
                            category: "admitted",
                            activity: None,
                            pending: Some(pending.is_some()),
                            fresh,
                            repair: selected_record.is_none(),
                            accelerated: false,
                        },
                    );
                    selected = Some((index, selected_record));
                    break;
                }
            }
            let Some((index, pending)) = selected else {
                break;
            };
            if let Some(info) = pending {
                peers[index].push.attempted(info);
            } else {
                peers[index].push.repaired();
            }
            peers[index].recovered = false;
            peers[index].active = true;
            peers[index].publish();
            next_peer = (index + 1) % peers.len();
            attempts.push(Box::pin(attempt(
                network,
                SyncClient {
                    client: store.client.clone(),
                    config: store.config.clone(),
                },
                options,
                clock.clone(),
                index,
                peers[index].peer,
                pending,
                peers[index].push.enabled(now),
            )));
        }
        let next = peers
            .iter()
            .filter(|peer| !peer.active)
            .map(|peer| {
                peer.due
                    .max(tokio::time::Instant::now() + options.retry_initial)
            })
            .min()
            .unwrap_or_else(|| tokio::time::Instant::now() + idle_tick);
        tokio::select! {
            Some((index, result)) = attempts.next(), if !attempts.is_empty() => {
                completed_attempt(index, result, &mut peers, network.sync_policy(), options, store.config.local);
            },
            _ = tokio::time::sleep_until(next), if attempts.len() < MAX_OUTBOUND_EXCHANGES => {},
            _ = store.client.wake.notified() => {},
            _ = recoveries.changed.notified() => {},
        }
    }
}

fn trace_schedule(
    network: &Network,
    store: &SyncClient,
    peer: &Schedule,
    decision: crate::diagnostics::Decision,
) {
    if let Some(binding) = network.peer(peer.peer) {
        crate::diagnostics::schedule(
            &store.config.group.0,
            binding.identity.as_bytes(),
            &peer.state,
            peer.due,
            peer.not_before,
            decision,
        );
    }
}

pub(crate) struct SyncClient {
    pub(crate) client: Client,
    pub(crate) config: NetworkConfig,
}
type TransportResult<T> = Result<T, prns_groups_reticulum::Error<RedbError>>;
fn transport_error(error: Error) -> prns_groups_reticulum::Error<RedbError> {
    match error {
        Error::Transport(error) => error,
        Error::Service(error) => error.into(),
        Error::Storage(error) => {
            prns_groups_service::Error::Store(prns_groups_store::StoreError::Backend(error)).into()
        }
        _ => prns_groups_service::Error::Fault(Fault::ApplicationStorageFault).into(),
    }
}
impl SyncStorage for SyncClient {
    type Error = RedbError;
    fn group(&self) -> GroupId {
        self.config.group
    }
    fn local_endpoint(&self) -> Endpoint {
        self.config.local
    }
    fn is_reader(&self, endpoint: Endpoint) -> bool {
        self.config.readers.contains(&endpoint)
    }
    async fn snapshot(&mut self) -> TransportResult<Option<InventorySnapshot>> {
        match self
            .client
            .call(Command::Snapshot)
            .await
            .map_err(transport_error)?
        {
            Reply::Snapshot(value) => Ok(value),
            _ => Err(transport_error(Error::Corrupt)),
        }
    }
    async fn inventory(
        &mut self,
        snapshot: InventorySnapshot,
        after: InventoryCursor,
        kind: InventoryKind,
        recipient: Endpoint,
        scan_limit: usize,
    ) -> TransportResult<InventoryPage> {
        match self
            .client
            .call(Command::Inventory {
                snapshot,
                after,
                kind,
                recipient,
                scan_limit,
            })
            .await
            .map_err(transport_error)?
        {
            Reply::Inventory(value) => Ok(value),
            _ => Err(transport_error(Error::Corrupt)),
        }
    }
    async fn envelope(&mut self, id: EnvelopeId) -> TransportResult<Option<Vec<u8>>> {
        match self
            .client
            .call(Command::Envelope(id))
            .await
            .map_err(transport_error)?
        {
            Reply::Envelope(value) => Ok(value),
            _ => Err(transport_error(Error::Corrupt)),
        }
    }
    async fn receipt(&mut self, record: RecordId) -> TransportResult<Option<Vec<u8>>> {
        match self
            .client
            .call(Command::Receipt(record))
            .await
            .map_err(transport_error)?
        {
            Reply::Envelope(value) => {
                if value.is_some() {
                    crate::diagnostics::receipt_transition(
                        &self.config.group.0,
                        &record.0,
                        "retained_receipt_ready",
                    );
                }
                Ok(value)
            }
            _ => Err(transport_error(Error::Corrupt)),
        }
    }
    async fn known(
        &mut self,
        info: EnvelopeInfo,
        incarnation: StoreIncarnation,
    ) -> TransportResult<bool> {
        match self
            .client
            .call(Command::Known { info, incarnation })
            .await
            .map_err(transport_error)?
        {
            Reply::Known(value) => Ok(value),
            _ => Err(transport_error(Error::Corrupt)),
        }
    }
    async fn receive(&mut self, wire: &[u8], options: SyncOptions) -> TransportResult<Incoming> {
        match self
            .client
            .call(Command::Receive {
                wire: wire.to_vec(),
                options,
            })
            .await
            .map_err(transport_error)?
        {
            Reply::Incoming(value) => {
                match value {
                    Incoming::Record(record) => crate::diagnostics::receipt_transition(
                        &self.config.group.0,
                        &record.0,
                        "durable_record_received",
                    ),
                    Incoming::Receipt(receipt) => crate::diagnostics::receipt_transition(
                        &self.config.group.0,
                        &receipt.record.0,
                        "authenticated_receipt_applied",
                    ),
                    Incoming::Rejected(_) => {}
                }
                Ok(value)
            }
            _ => Err(transport_error(Error::Corrupt)),
        }
    }
    fn envelope_id(&self, wire: &[u8]) -> EnvelopeId {
        prns_groups_service::envelope_id(self.config.group, wire)
    }
}

// Lane saturation is ordinary flow control, never a reason to stop the shared
// node or lose an already committed reconciliation checkpoint.
pub(crate) async fn retry_busy<T, F, Fut>(delay: Duration, mut action: F) -> Result<T, Error>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, Error>>,
{
    loop {
        match action().await {
            Err(Error::Busy) => tokio::time::sleep(delay).await,
            result => return result,
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use crate::Submit;
    use std::sync::atomic::{AtomicUsize, Ordering};
    fn peer(id: u8) -> Endpoint {
        Endpoint {
            id: prns_groups_core::EndpointId([id; 32]),
            incarnation: prns_groups_core::EndpointIncarnation::new(1).unwrap(),
        }
    }
    fn schedule(id: u8, now: tokio::time::Instant) -> Schedule {
        Schedule {
            peer: peer(id),
            diagnostic: None,
            due: now,
            not_before: now,
            state: PeerSchedule::default(),
            round: 0,
            active: false,
            failed: false,
            recovered: false,
            push: crate::delivery::PushState::default(),
            observer: None,
            pending: None,
            issue: None,
        }
    }
    #[test]
    fn local_and_remote_submit_size_limits_fall_back_to_pull_without_claiming_delivery() {
        let local: prns_groups_reticulum::Error<RedbError> =
            prns_groups_reticulum::ProtocolError::Oversized.into();
        let remote = prns_groups_reticulum::Error::Remote(RemoteError::OverBudget);
        let wrong: prns_groups_reticulum::Error<RedbError> =
            prns_groups_reticulum::ProtocolError::WrongEnvelope.into();
        assert!(push_requires_pull(&local));
        assert!(push_requires_pull::<RedbError>(&remote));
        assert!(!push_requires_pull(&wrong));
    }

    #[tokio::test]
    async fn standalone_storage_wait_keeps_an_active_link_progressing() {
        let now = tokio::time::Instant::now();
        let mut peers = vec![schedule(1, now)];
        peers[0].active = true;
        let mut attempts: FuturesUnordered<SingleAttempt<'_>> = FuturesUnordered::new();
        let (finished, unblock) = tokio::sync::oneshot::channel();
        attempts.push(Box::pin(async move {
            let _ = finished.send(());
            (
                0,
                Ok(AttemptReport {
                    path: PathCapabilities::default(),
                    activity: SyncActivity::Progress,
                    unsupported: false,
                    push_exchanged: true,
                    issue: None,
                }),
            )
        }));
        let options = RunnerOptions {
            poll_interval: Duration::from_secs(1),
            retry_initial: Duration::from_secs(1),
            retry_max: Duration::from_secs(300),
            budget: SyncBudget {
                max_requests: 4,
                max_envelopes: 1,
                max_bytes: 8192,
            },
        };
        tokio::time::timeout(
            Duration::from_secs(1),
            alongside_attempts(
                async {
                    unblock.await.unwrap();
                },
                &mut attempts,
                &mut peers,
                SyncPolicy::default(),
                options,
                peer(2),
            ),
        )
        .await
        .unwrap();
        assert!(!peers[0].active);
        assert!(attempts.is_empty());
        assert!(peers[0].due > now);
    }

    #[test]
    fn retained_work_preserves_failure_backoff_and_minimum_contact_spacing() {
        let now = tokio::time::Instant::now();
        let tuning = SyncPolicy::default().tuning(PathCapabilities::default());
        let mut peer = schedule(1, now);
        peer.completed(now, tuning, SyncActivity::Idle, self::peer(2));
        let idle_due = peer.due;
        assert!(crate::schedule::expedite(
            &peer.state,
            &mut peer.due,
            peer.not_before,
            prns_groups_core::sync::SyncWake::PendingWork,
            now
        ));
        assert_eq!(
            peer.due,
            now + Duration::from_millis(tuning.active_poll_millis)
        );
        assert!(peer.due < idle_due);
        peer.completed(now, tuning, SyncActivity::Failed, self::peer(2));
        let first_failure = peer.due;
        assert!(!crate::schedule::expedite(
            &peer.state,
            &mut peer.due,
            peer.not_before,
            prns_groups_core::sync::SyncWake::PendingWork,
            now
        ));
        assert_eq!(peer.due, first_failure);
        peer.completed(now, tuning, SyncActivity::Failed, self::peer(2));
        assert!(peer.due > first_failure);
        assert_ne!(
            peer_jitter(self::peer(1), self::peer(2), 1),
            peer_jitter(self::peer(1), self::peer(3), 1)
        );
    }
    #[test]
    fn configured_timing_floors_survive_a_faster_interface_policy() {
        let options = RunnerOptions {
            poll_interval: Duration::from_secs(400),
            retry_initial: Duration::from_secs(500),
            retry_max: Duration::from_secs(600),
            budget: SyncBudget {
                max_requests: 2,
                max_envelopes: 1,
                max_bytes: 1024,
            },
        };
        let tuning = runner_tuning(SyncPolicy::default(), PathCapabilities::default(), options);
        let mut state = PeerSchedule::default();
        assert!(state.delay_millis(tuning, SyncActivity::Progress, 0) >= 400_000);
        assert!(state.delay_millis(tuning, SyncActivity::Idle, 0) >= 400_000);
        assert!(state.delay_millis(tuning, SyncActivity::Failed, 0) >= 500_000);
        assert_eq!(tuning.retry_max_millis, 600_000);
        assert_eq!(millis_ceil(Duration::from_nanos(1_000_001)), 2);
    }
    #[test]
    fn round_robin_skips_inflight_peers_and_honors_the_next_due_time() {
        let now = tokio::time::Instant::now();
        let mut peers = vec![schedule(1, now), schedule(2, now), schedule(3, now)];
        peers[0].active = true;
        assert_eq!(next_due(&peers, 0, now), Some(1));
        peers[1].active = true;
        assert_eq!(next_due(&peers, 2, now), Some(2));
        peers[2].due = now + Duration::from_secs(1);
        assert_eq!(next_due(&peers, 0, now), None);
        peers[0].active = false;
        assert_eq!(next_due(&peers, 2, now), Some(0));
    }
    struct Saturated {
        budget: crate::InputBudget,
        load: AtomicUsize,
        save: AtomicUsize,
        config: AtomicUsize,
        support: AtomicUsize,
    }
    impl Submit for Saturated {
        fn input_budget(&self) -> &crate::InputBudget {
            &self.budget
        }
        fn submit(&self, job: crate::Job) -> Result<(), Error> {
            let counter = match &job.command {
                Command::LoadCursor(_) => &self.load,
                Command::SaveCursor(_, _) => &self.save,
                Command::Config => &self.config,
                Command::ProfileSupport(_) => &self.support,
                _ => {
                    job.fail(Error::InvalidInput);
                    return Ok(());
                }
            };
            if counter.fetch_add(1, Ordering::SeqCst) < 2 {
                return Err(Error::Busy);
            }
            // Once admission succeeds this independent owner chooses a terminal
            // result. Busy must not escape early or obscure that result.
            job.fail(Error::NotProvisioned);
            Ok(())
        }
    }
    #[tokio::test]
    async fn config_and_checkpoint_backpressure_retries_without_losing_terminal_result() {
        let owner = Arc::new(Saturated {
            budget: Default::default(),
            load: AtomicUsize::new(0),
            save: AtomicUsize::new(0),
            config: AtomicUsize::new(0),
            support: AtomicUsize::new(0),
        });
        let client = Client::new(owner.clone());
        let peer = Endpoint {
            id: prns_groups_core::EndpointId([1; 32]),
            incarnation: prns_groups_core::EndpointIncarnation::new(1).unwrap(),
        };
        assert!(matches!(
            retry_busy(Duration::from_millis(1), || client.network_config()).await,
            Err(Error::NotProvisioned)
        ));
        assert!(matches!(
            retry_busy(Duration::from_millis(1), || client.bind_profile_support(
                crate::ProfileSupport::new(65_536, 131_072).unwrap()
            ))
            .await,
            Err(Error::NotProvisioned)
        ));
        assert!(matches!(
            retry_busy(Duration::from_millis(1), || client.load_cursor(peer)).await,
            Err(Error::NotProvisioned)
        ));
        assert!(matches!(
            retry_busy(Duration::from_millis(1), || client.save_cursor(peer, None)).await,
            Err(Error::NotProvisioned)
        ));
        assert_eq!(
            [
                owner.config.load(Ordering::SeqCst),
                owner.load.load(Ordering::SeqCst),
                owner.save.load(Ordering::SeqCst),
                owner.support.load(Ordering::SeqCst)
            ],
            [3; 4]
        );
    }
}
