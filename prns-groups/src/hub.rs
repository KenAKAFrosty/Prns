//! Caller-owned routing and bounded polling for several groups on one host.

use crate::diagnostics::{self, Event};
use crate::runner::{exchange, retry_busy, SyncClient};
use crate::{Client, Clock, Error, NetworkConfig, SystemClock};
use futures_util::{stream::FuturesUnordered, StreamExt};
use prns_groups_core::sync::{
    PathCapabilities, PeerSchedule, SyncActivity, SyncIssue, SyncPhase, SyncPolicy, SyncWake,
};
use prns_groups_reticulum::{
    enrollment, request_group, serve_storage_with_options, Limits, Network, RequestJob, SyncBudget,
    SyncOptions,
};
use prns_groups_service::{ControlReference, Endpoint, EnvelopeInfo, GroupId};
use prns_host::DestinationHash;
use prns_host_runtime::owner::HostClient;
use prns_host_runtime::service_scope::HostServiceScope;
use sha2::{Digest, Sha256};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use std::{future::Future, pin::Pin, sync::Arc};
use tokio::sync::{mpsc, watch};
use tokio::time::Instant;

#[derive(Clone)]
struct Carrier {
    host: HostClient,
    scope: Option<HostServiceScope>,
}
impl Carrier {
    fn identity(&self) -> Result<prns_host::IdentityHash, Error> {
        match &self.scope {
            Some(scope) => Ok(scope.identity_hash()),
            None => self.host.identity_hash().map_err(|_| Error::Unavailable),
        }
    }
    fn sync(
        &self,
        destination: DestinationHash,
        peers: Vec<prns_groups_reticulum::Peer>,
        limits: Limits,
    ) -> Result<Network, prns_groups_reticulum::NetworkError> {
        match &self.scope {
            Some(scope) => Network::scoped(scope.clone(), destination, peers, limits),
            None => Network::new(self.host.clone(), destination, peers, limits),
        }
    }
    fn enrollment(
        &self,
        destination: DestinationHash,
        limits: enrollment::Limits,
    ) -> Result<enrollment::Network, prns_groups_reticulum::NetworkError> {
        match &self.scope {
            Some(scope) => enrollment::Network::scoped(scope.clone(), destination, limits),
            None => enrollment::Network::new(self.host.clone(), destination, limits),
        }
    }
}

const POLL: Duration = Duration::from_secs(2);
const STORE_WAIT: Duration = Duration::from_secs(10);
const MAX_IN_FLIGHT: usize = 2;
const MAX_BACKOFFS: usize = 256;
const BUDGET: SyncBudget = SyncBudget {
    max_requests: 8,
    max_envelopes: 4,
    max_bytes: 262_144,
};

/// Multiplex identified enrollment and per-group synchronization on the caller's
/// existing host. Configuration is reloaded for every sync attempt and fenced by
/// control head on the storage lane. Stop/cancellation drops owned Link waiters,
/// but never cancels a database mutation already admitted by `Client`.
#[allow(clippy::too_many_arguments)]
pub async fn run_multi(
    client: Client,
    host: HostClient,
    sync_destination: DestinationHash,
    enrollment_destination: DestinationHash,
    sync_jobs: mpsc::Receiver<RequestJob>,
    enrollment_jobs: mpsc::Receiver<RequestJob>,
    limits: Limits,
    enrollment_limits: enrollment::Limits,
    stop: watch::Receiver<bool>,
) -> Result<(), Error> {
    run_multi_with_policy(
        client,
        host,
        sync_destination,
        enrollment_destination,
        sync_jobs,
        enrollment_jobs,
        limits,
        enrollment_limits,
        SyncPolicy::default(),
        stop,
    )
    .await
}

/// The same caller-owned runner with explicit bounded link/scheduling policy.
/// At most two outbound attempts progress concurrently; an unavailable peer
/// cannot monopolize the sole outbound lane. Stop drops both network waiters,
/// while admitted database transactions remain owned by the storage lane.
#[allow(clippy::too_many_arguments)]
pub async fn run_multi_with_policy(
    client: Client,
    host: HostClient,
    sync_destination: DestinationHash,
    enrollment_destination: DestinationHash,
    sync_jobs: mpsc::Receiver<RequestJob>,
    enrollment_jobs: mpsc::Receiver<enrollment::RequestJob>,
    limits: Limits,
    enrollment_limits: enrollment::Limits,
    policy: SyncPolicy,
    stop: watch::Receiver<bool>,
) -> Result<(), Error> {
    let (_control, announcements) = crate::announce_control(Some(policy.announce_interval_millis))?;
    run_multi_with_announcements(
        client,
        host,
        sync_destination,
        enrollment_destination,
        sync_jobs,
        enrollment_jobs,
        limits,
        enrollment_limits,
        policy,
        announcements,
        stop,
    )
    .await
}

/// Multi-group runner with live announcement configuration and a bounded manual
/// request lane. The caller owns the returned controller and this future.
#[allow(clippy::too_many_arguments)]
pub async fn run_multi_with_announcements(
    client: Client,
    host: HostClient,
    sync_destination: DestinationHash,
    enrollment_destination: DestinationHash,
    sync_jobs: mpsc::Receiver<RequestJob>,
    enrollment_jobs: mpsc::Receiver<enrollment::RequestJob>,
    limits: Limits,
    enrollment_limits: enrollment::Limits,
    policy: SyncPolicy,
    announcements: crate::AnnounceRequests,
    stop: watch::Receiver<bool>,
) -> Result<(), Error> {
    run_multi_impl(
        client,
        Carrier { host, scope: None },
        sync_destination,
        enrollment_destination,
        sync_jobs,
        enrollment_jobs,
        limits,
        enrollment_limits,
        policy,
        announcements,
        stop,
    )
    .await
}

/// Multiplex groups under one immutable identity generation on a shared host.
/// Stop/join this future, fence/drain the scope and storage lane, then complete
/// the endpoint teardown barrier before acknowledging lifecycle quiescence.
#[allow(clippy::too_many_arguments)]
pub async fn run_multi_scoped_with_announcements(
    client: Client,
    scope: HostServiceScope,
    sync_destination: DestinationHash,
    enrollment_destination: DestinationHash,
    sync_jobs: mpsc::Receiver<RequestJob>,
    enrollment_jobs: mpsc::Receiver<enrollment::RequestJob>,
    limits: Limits,
    enrollment_limits: enrollment::Limits,
    policy: SyncPolicy,
    announcements: crate::AnnounceRequests,
    stop: watch::Receiver<bool>,
) -> Result<(), Error> {
    let carrier = Carrier {
        host: scope.host().clone(),
        scope: Some(scope),
    };
    run_multi_impl(
        client,
        carrier,
        sync_destination,
        enrollment_destination,
        sync_jobs,
        enrollment_jobs,
        limits,
        enrollment_limits,
        policy,
        announcements,
        stop,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn run_multi_impl(
    client: Client,
    host: Carrier,
    sync_destination: DestinationHash,
    enrollment_destination: DestinationHash,
    sync_jobs: mpsc::Receiver<RequestJob>,
    enrollment_jobs: mpsc::Receiver<enrollment::RequestJob>,
    limits: Limits,
    enrollment_limits: enrollment::Limits,
    policy: SyncPolicy,
    announcements: crate::AnnounceRequests,
    mut stop: watch::Receiver<bool>,
) -> Result<(), Error> {
    if !policy.is_valid() {
        return Err(Error::Configuration);
    }
    if *stop.borrow() {
        return Ok(());
    }
    let support = crate::ProfileSupport::new(
        limits.max_envelope_bytes().map_err(carrier_error)?,
        enrollment_limits
            .max_application_frame_bytes()
            .map_err(carrier_error)?,
    )
    .map_err(|_| Error::Configuration)?;
    // Install on the actual owner lane before exposing runner readiness. The
    // same bound governs later Create/Offer/Join jobs, including other clients
    // and lexical views. A failed or cancelled waiter cannot undo installation.
    tokio::select! {
        result = retry_busy(POLL, || client.bind_profile_support(support)) => result?,
        _ = stop.wait_for(|value| *value) => return Ok(()),
    }
    let enrollment = host
        .enrollment(enrollment_destination, enrollment_limits)
        .and_then(|network| network.with_sync_policy(policy))
        .map_err(carrier_error)?;
    // This also validates sync limits and the destination when there are no
    // groups yet. Peer membership is loaded only for a concrete group attempt.
    host.sync(sync_destination, Vec::new(), limits)
        .map_err(carrier_error)?;
    let stored_identity = tokio::select! {
        result = tokio::time::timeout(STORE_WAIT, retry_busy(POLL, || async {
            client.identity().await.map_err(|error| match error {
                crate::ApiError::Busy => Error::Busy,
                error => Error::Api(error),
            })
        })) => result.map_err(|_| Error::Unavailable)??,
        _ = stop.wait_for(|value| *value) => return Ok(()),
    };
    if stored_identity != host.identity()? {
        return Err(Error::Configuration);
    }
    let _progress_lease = client.progress.start()?;
    let inbound_enrollment = inbound_enrollment(&client, &enrollment, enrollment_jobs);
    let recoveries = crate::schedule::Recoveries::default();
    let inbound_sync = inbound_sync(
        &client,
        &host,
        sync_destination,
        limits,
        policy,
        sync_jobs,
        &recoveries,
    );
    let outbound = outbound(
        &client,
        &host,
        sync_destination,
        enrollment_destination,
        limits,
        enrollment_limits,
        policy,
        &recoveries,
    );
    let announces = crate::announces::run(
        &host.host,
        host.scope.as_ref(),
        [sync_destination, enrollment_destination],
        announcements,
    );
    tokio::pin!(inbound_enrollment, inbound_sync, outbound, announces);
    loop {
        if *stop.borrow() {
            return Ok(());
        }
        tokio::select! {
            result = &mut inbound_enrollment => return result,
            result = &mut inbound_sync => return result,
            result = &mut outbound => return result,
            result = &mut announces => return result,
            changed = stop.changed() => if changed.is_err() { return Ok(()); },
        }
    }
}

fn carrier_error(error: prns_groups_reticulum::NetworkError) -> Error {
    Error::Transport(error.into())
}

fn now_seconds() -> Result<u64, Error> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|time| time.as_secs())
        .map_err(|_| Error::Clock)
}

async fn inbound_enrollment(
    client: &Client,
    network: &enrollment::Network,
    mut jobs: mpsc::Receiver<RequestJob>,
) -> Result<(), Error> {
    while let Some(job) = jobs.recv().await {
        diagnostics::emit(
            Event::EnrollmentReceived,
            "accepted_by_ingress",
            job.data().len(),
        );
        let response = tokio::time::timeout(STORE_WAIT, async {
            client
                .enrollment_handle(job.requester(), job.data().to_vec(), now_seconds()?)
                .await
        })
        .await;
        match response {
            Ok(Ok(response)) => {
                diagnostics::emit(Event::EnrollmentHandled, "committed", job.data().len());
                let category = match network.respond(&job, &response).await {
                    Ok(()) => "queued",
                    Err(ref error) => diagnostics::network_category(error),
                };
                diagnostics::emit(Event::EnrollmentResponse, category, response.len());
            }
            Ok(Err(error)) => diagnostics::emit(
                Event::EnrollmentHandled,
                diagnostics::error_category(&error),
                job.data().len(),
            ),
            Err(_) => {
                diagnostics::emit(Event::EnrollmentHandled, "store_timeout", job.data().len())
            }
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn inbound_sync(
    client: &Client,
    host: &Carrier,
    destination: DestinationHash,
    limits: Limits,
    policy: SyncPolicy,
    mut jobs: mpsc::Receiver<RequestJob>,
    recoveries: &crate::schedule::Recoveries,
) -> Result<(), Error> {
    while let Some(job) = jobs.recv().await {
        diagnostics::emit(Event::SyncReceived, "accepted_by_ingress", job.data().len());
        let Ok(group) = request_group(job.data()) else {
            diagnostics::emit(
                Event::SyncServed,
                "malformed_group_request",
                job.data().len(),
            );
            continue;
        };
        let mut trace =
            diagnostics::AttemptTrace::start("inbound", &group.0, job.requester().as_bytes());
        let result = async {
            let config = client.for_group(group).network_config().await?;
            if config.group != group {
                return Err(Error::Configuration);
            }
            let network = network(host, destination, limits, policy, &config).await?;
            let mut store = storage(client, config);
            let options = SyncOptions {
                incarnation: store.config.incarnation,
                clock: SystemClock.now(),
                budget: BUDGET,
            };
            serve_storage_with_options(&network, &mut store, &job, options)
                .await
                .map_err(Error::from)?;
            if let Some(hint) =
                crate::schedule::authenticated_recovery(&network, &store, &job).await
            {
                recoveries.offer(hint);
            }
            Ok(())
        }
        .await;
        trace.finish(match &result {
            Ok(()) => "served",
            Err(error) => diagnostics::error_category(error),
        });
        if let Err(error) = result {
            diagnostics::emit(
                Event::SyncServed,
                diagnostics::error_category(&error),
                job.data().len(),
            );
        }
    }
    Ok(())
}

fn storage(client: &Client, config: NetworkConfig) -> SyncClient {
    SyncClient {
        client: client.for_group(config.group).at_control(config.control),
        config,
    }
}

async fn network(
    host: &Carrier,
    destination: DestinationHash,
    limits: Limits,
    policy: SyncPolicy,
    config: &NetworkConfig,
) -> Result<Network, Error> {
    if host.identity()? != config.local_reticulum_identity {
        return Err(Error::Configuration);
    }
    let network = host
        .sync(destination, config.peers.clone(), limits)
        .and_then(|network| network.with_sync_policy(policy))
        .map_err(carrier_error)?;
    network.authorize_peers().await.map_err(carrier_error)?;
    Ok(network)
}

#[derive(Clone, PartialEq, Eq)]
struct SyncKey {
    group: GroupId,
    peer: Endpoint,
    identity: prns_host::IdentityHash,
    control: ControlReference,
    incarnation: prns_groups_store::StoreIncarnation,
}

#[derive(Clone, PartialEq, Eq)]
enum WorkKey {
    Enrollment([u8; 32], [u8; 32], prns_host::IdentityHash),
    Sync(SyncKey),
}

impl WorkKey {
    fn identity(&self) -> prns_host::IdentityHash {
        match self {
            Self::Enrollment(_, _, identity) => *identity,
            Self::Sync(key) => key.identity,
        }
    }
}

fn peer_active(active: &[WorkKey], key: &WorkKey) -> bool {
    active
        .iter()
        .any(|entry| entry.identity() == key.identity())
}

struct Schedule<K> {
    key: K,
    due: Instant,
    not_before: Instant,
    failed: bool,
    recovered: bool,
    state: PeerSchedule,
    push: crate::delivery::PushState,
    pending: Option<bool>,
    issue: Option<SyncIssue>,
}

/// Disposable, bounded scheduling metadata; never payloads, keys or receipts.
struct Backoffs<K> {
    entries: Vec<Schedule<K>>,
    progress: Option<(Arc<crate::progress::ProgressTracker>, u64)>,
}
impl<K: PartialEq> Backoffs<K> {
    fn new() -> Self {
        Self {
            entries: Vec::new(),
            progress: None,
        }
    }
    fn due(&self, key: &K, now: Instant) -> bool {
        self.entries
            .iter()
            .find(|entry| &entry.key == key)
            .is_none_or(|entry| entry.due <= now)
    }
    fn entry(&mut self, key: K, now: Instant) -> &mut Schedule<K> {
        let index = match self.entries.iter().position(|entry| entry.key == key) {
            Some(index) => index,
            None => {
                if self.entries.len() == MAX_BACKOFFS {
                    self.entries.remove(0);
                }
                self.entries.push(Schedule {
                    key,
                    due: now,
                    not_before: now,
                    failed: false,
                    recovered: false,
                    state: PeerSchedule::default(),
                    push: crate::delivery::PushState::default(),
                    pending: None,
                    issue: None,
                });
                self.entries.len() - 1
            }
        };
        &mut self.entries[index]
    }
    fn push_state(&self, key: &K) -> crate::delivery::PushState {
        self.entries
            .iter()
            .find(|entry| &entry.key == key)
            .map(|entry| entry.push)
            .unwrap_or_default()
    }
    fn accelerate(&mut self, key: K, evidence: SyncWake, now: Instant) -> bool {
        let entry = self.entry(key, now);
        crate::schedule::expedite(
            &entry.state,
            &mut entry.due,
            entry.not_before,
            evidence,
            now,
        )
    }
    fn begin(&mut self, key: K, pending: Option<EnvelopeInfo>, now: Instant) {
        let entry = self.entry(key, now);
        entry.recovered = false;
        if let Some(info) = pending {
            entry.push.attempted(info);
        } else {
            entry.push.repaired();
        }
    }
    fn unsupported(&mut self, key: K, now: Instant) {
        self.entry(key, now).push.unsupported(now);
    }
    fn record(
        &mut self,
        key: K,
        activity: SyncActivity,
        path: PathCapabilities,
        policy: SyncPolicy,
        now: Instant,
        jitter: u32,
    ) {
        let entry = self.entry(key, now);
        let tuning = policy.tuning(path);
        let delay = entry.state.delay_millis(tuning, activity, jitter);
        entry.failed = activity == SyncActivity::Failed;
        entry.recovered = false;
        entry.not_before = now + Duration::from_millis(tuning.active_poll_millis);
        entry.due = now + Duration::from_millis(delay);
    }
}

impl Backoffs<WorkKey> {
    fn publish(&self, key: &WorkKey, active: bool) {
        let (Some((progress, generation)), WorkKey::Sync(key)) = (&self.progress, key) else {
            return;
        };
        let Some(entry) = self
            .entries
            .iter()
            .find(|entry| entry.key == WorkKey::Sync(key.clone()))
        else {
            return;
        };
        let phase = if active {
            if entry.pending == Some(true) {
                SyncPhase::Connecting
            } else {
                SyncPhase::Synchronizing
            }
        } else if let Some(issue) = entry.issue {
            crate::progress::issue_phase(issue)
        } else if entry.failed {
            SyncPhase::RetryScheduled
        } else {
            SyncPhase::Waiting
        };
        progress.update(
            *generation,
            key.group,
            key.peer,
            key.control,
            key.incarnation,
            phase,
            entry.pending,
            (!active).then_some(entry.due),
            entry.state.failures(),
            entry.issue,
        );
    }

    fn recover(
        &mut self,
        hint: crate::schedule::RecoveryHint,
        configs: &[NetworkConfig],
        candidates: &[SyncKey],
        active: &[WorkKey],
        now: Instant,
    ) -> crate::schedule::RecoveryDisposition {
        use crate::schedule::RecoveryDisposition;
        if !configs.iter().any(|config| hint.current(config)) {
            return RecoveryDisposition::ScopeStale;
        }
        let Some(candidate) = candidates.iter().find(|key| {
            key.group == hint.group
                && key.control == hint.control
                && key.incarnation == hint.incarnation
                && key.peer == hint.peer
                && key.identity == hint.identity
        }) else {
            return RecoveryDisposition::CandidateMissing;
        };
        let key = WorkKey::Sync(candidate.clone());
        let occupied = peer_active(active, &key);
        self.align_sync(candidate);
        let entry = self.entry(key, now);
        entry.recovered |= entry.failed;
        if occupied {
            // Hold this exact-scope evidence until the shared physical-peer slot
            // is free. A later control/store change clears it in align_sync.
            return RecoveryDisposition::PeerActive;
        }
        if entry.failed {
            RecoveryDisposition::Credited
        } else {
            RecoveryDisposition::NotFailed
        }
    }

    /// Carry contact spacing and fresh-record watermarks across control heads.
    /// Only a different durable store incarnation invalidates cursor positions.
    fn align_sync(&mut self, key: &SyncKey) {
        if let Some(entry) = self.entries.iter_mut().find(|entry| matches!(&entry.key,
            WorkKey::Sync(old) if old.group == key.group && old.peer == key.peer && old.identity == key.identity)) {
            if let WorkKey::Sync(old) = &entry.key {
                if old.control != key.control || old.incarnation != key.incarnation {
                    entry.recovered = false;
                }
                if old.incarnation != key.incarnation {
                    entry.push = crate::delivery::PushState::default();
                }
            }
            entry.key = WorkKey::Sync(key.clone());
        }
    }
    fn trace(&self, key: &WorkKey, decision: diagnostics::Decision) {
        let WorkKey::Sync(key) = key else { return };
        if let Some(entry) = self
            .entries
            .iter()
            .find(|entry| entry.key == WorkKey::Sync(key.clone()))
        {
            diagnostics::schedule(
                &key.group.0,
                key.identity.as_bytes(),
                &entry.state,
                entry.due,
                entry.not_before,
                decision,
            );
        }
    }
}

struct AttemptResult {
    key: WorkKey,
    path: PathCapabilities,
    activity: SyncActivity,
    unsupported: bool,
    push_exchanged: bool,
    issue: Option<SyncIssue>,
}
type Attempt = Pin<Box<dyn Future<Output = AttemptResult> + Send>>;

fn completed_attempt(
    result: AttemptResult,
    active: &mut Vec<WorkKey>,
    backoff: &mut Backoffs<WorkKey>,
    policy: SyncPolicy,
    jitter: &mut u32,
) {
    active.retain(|key| key != &result.key);
    let entry = backoff.entry(result.key.clone(), Instant::now());
    entry.issue = result.issue;
    if result.push_exchanged {
        entry.push.exchanged();
    }
    if result.unsupported {
        backoff.unsupported(result.key.clone(), Instant::now());
    }
    backoff.record(
        result.key.clone(),
        result.activity,
        result.path,
        policy,
        Instant::now(),
        next_jitter(jitter),
    );
    backoff.publish(&result.key, false);
    backoff.trace(
        &result.key,
        diagnostics::Decision {
            category: "completed",
            activity: Some(result.activity),
            pending: None,
            fresh: false,
            repair: false,
            accelerated: false,
        },
    );
}

/// Database admission/refresh never stops polling an already active Link.
async fn alongside_attempts<F: Future>(
    action: F,
    pending: &mut FuturesUnordered<Attempt>,
    active: &mut Vec<WorkKey>,
    backoff: &mut Backoffs<WorkKey>,
    policy: SyncPolicy,
    jitter: &mut u32,
) -> F::Output {
    tokio::pin!(action);
    loop {
        tokio::select! {
            value = &mut action => return value,
            Some(result) = pending.next(), if !pending.is_empty() => {
                completed_attempt(result, active, backoff, policy, jitter);
            }
        }
    }
}

// Jitter is scheduling only, not cryptographic randomness. Mix a per-owner seed
// with a counter so peers do not synchronise their idle/retry cycles.
pub(crate) fn next_jitter(state: &mut u32) -> u32 {
    *state ^= *state << 13;
    *state ^= *state >> 17;
    *state ^= *state << 5;
    *state
}

// One catalog candidate and one roster are owned at a time. These are work
// budgets, not catalog or membership caps. Continuations survive unrelated
// writes, control changes and disposable status/backoff cache eviction.
const CONFIG_PAGE: crate::PageLimits = crate::PageLimits {
    max_rows: 1,
    max_bytes: 65_536,
};

#[derive(Default)]
struct GroupTraversal {
    after: Option<GroupId>,
    selected: Option<GroupId>,
    next: Option<GroupId>,
    peer_after: Option<Endpoint>,
}
impl GroupTraversal {
    fn select(&mut self, page: crate::ConversationPage) -> Result<(), Error> {
        if page.items.len() > 1 {
            return Err(Error::Corrupt);
        }
        self.next = page.next;
        self.selected = page
            .items
            .into_iter()
            .find(|row| {
                row.ready && row.lifecycle == crate::GroupLifecycle::Active && row.control.is_some()
            })
            .map(|row| row.group);
        self.peer_after = None;
        if self.selected.is_none() {
            self.advance();
        }
        Ok(())
    }
    fn advance(&mut self) {
        self.after = self.next.take();
        self.selected = None;
        self.peer_after = None;
    }
    fn unavailable(&mut self) {
        // A failed page read has not selected anything to skip. Retain its
        // exclusive cursor and retry after the ordinary poll delay.
        if self.selected.is_some() {
            self.advance();
        }
    }
    fn record_configuration(
        &mut self,
        progress: &crate::progress::ProgressTracker,
        result: Result<Option<NetworkConfig>, Error>,
    ) -> Option<NetworkConfig> {
        match result {
            Ok(config) => {
                // A catalog step observes one group, not the complete catalog.
                progress.configure(config.as_slice());
                config
            }
            Err(_) => {
                // One unavailable/deleted selection cannot block the rest of
                // the catalog, but its failure must not erase observed facts.
                if let Some(group) = self.selected {
                    progress.unavailable(group, SyncIssue::StorageUnavailable);
                }
                self.unavailable();
                None
            }
        }
    }
    fn visit(&mut self, peer: Endpoint) -> bool {
        if self.peer_after.is_some_and(|after| peer <= after) {
            return false;
        }
        self.peer_after = Some(peer);
        true
    }
    async fn configuration(&mut self, client: &Client) -> Result<Option<NetworkConfig>, Error> {
        if self.selected.is_none() {
            self.select(
                client
                    .conversations(self.after, CONFIG_PAGE)
                    .await
                    .map_err(Error::Api)?,
            )?;
        }
        let Some(group) = self.selected else {
            return Ok(None);
        };
        // Revalidate exact current control without resetting the endpoint
        // cursor. A newly inserted earlier peer is discovered on the next pass.
        let detail = client
            .conversation(group, CONFIG_PAGE)
            .await
            .map_err(Error::Api)?;
        if detail.lifecycle != crate::GroupLifecycle::Active {
            client.progress.forget(group);
            self.advance();
            return Ok(None);
        }
        let scoped = client.for_group(group).at_control(detail.control);
        scoped.maintain(SystemClock.now()).await?;
        let config = scoped.network_config().await?;
        Ok(Some(config))
    }
}

#[derive(Default)]
struct WorkTraversal {
    after: Option<[u8; 32]>,
    next: Option<[u8; 32]>,
    items: Vec<crate::enrollment::Work>,
    position: usize,
    loaded: bool,
}
impl WorkTraversal {
    fn select(&mut self, page: crate::enrollment::WorkPage) {
        self.items = page.items;
        self.next = page.next;
        self.position = 0;
        self.loaded = true;
    }
    fn finish(&mut self) {
        if self.loaded && self.position == self.items.len() {
            self.after = self.next.take();
            self.items.clear();
            self.loaded = false;
        }
    }
}

fn enrollment_page(limits: enrollment::Limits) -> Result<crate::PageLimits, Error> {
    let row = std::mem::size_of::<crate::enrollment::Work>()
        .checked_add(limits.max_frame_bytes)
        .ok_or(Error::Configuration)?;
    let bytes = row
        .checked_mul(MAX_IN_FLIGHT)
        .and_then(|bytes| bytes.checked_add(std::mem::size_of::<crate::enrollment::WorkPage>()))
        .ok_or(Error::Configuration)?;
    Ok(crate::PageLimits {
        max_rows: MAX_IN_FLIGHT as u32,
        max_bytes: u64::try_from(bytes).map_err(|_| Error::Configuration)?,
    })
}

#[allow(clippy::too_many_arguments)]
async fn outbound(
    client: &Client,
    host: &Carrier,
    sync_destination: DestinationHash,
    enrollment_destination: DestinationHash,
    limits: Limits,
    enrollment_limits: enrollment::Limits,
    policy: SyncPolicy,
    recoveries: &crate::schedule::Recoveries,
) -> Result<(), Error> {
    let mut backoff = Backoffs::new();
    backoff.progress = Some((Arc::clone(&client.progress), client.progress.generation()));
    let mut pending: FuturesUnordered<Attempt> = FuturesUnordered::new();
    let mut active = Vec::with_capacity(MAX_IN_FLIGHT);
    let mut work = WorkTraversal::default();
    let mut groups = GroupTraversal::default();
    let work_limit = enrollment_page(enrollment_limits)?;
    let mut prefer_enrollment = true;
    let mut refresh_due = Instant::now();
    let identity = host.identity()?;
    let mut jitter = u32::from_be_bytes(
        identity.as_bytes()[..4]
            .try_into()
            .map_err(|_| Error::Configuration)?,
    ) | 1;
    loop {
        let now = Instant::now();
        if now >= refresh_due && pending.len() < MAX_IN_FLIGHT {
            let config = alongside_attempts(
                async {
                    let _ =
                        tokio::time::timeout(STORE_WAIT, client.maintain(SystemClock.now())).await;
                    if !work.loaded {
                        if let Ok(now) = now_seconds() {
                            if let Ok(Ok(page)) = tokio::time::timeout(
                                STORE_WAIT,
                                client.enrollment_work(now, work.after, work_limit),
                            )
                            .await
                            {
                                work.select(page);
                            }
                        }
                    }
                    tokio::time::timeout(STORE_WAIT, groups.configuration(client)).await
                },
                &mut pending,
                &mut active,
                &mut backoff,
                policy,
                &mut jitter,
            )
            .await;
            let config_read = matches!(&config, Ok(Ok(_)));
            let config = groups
                .record_configuration(&client.progress, config.unwrap_or(Err(Error::Unavailable)));
            let configs = config.as_slice();
            let mut candidates: Vec<_> = configs
                .iter()
                .flat_map(|config| {
                    config
                        .peers
                        .iter()
                        .filter(|peer| peer.endpoint != config.local)
                        .map(move |peer| SyncKey {
                            group: config.group,
                            peer: peer.endpoint,
                            identity: peer.identity,
                            control: config.control,
                            incarnation: config.incarnation,
                        })
                })
                .collect();
            candidates.sort_unstable_by_key(|candidate| candidate.peer);
            if let Some(group) = groups.selected {
                for hint in recoveries.take_group(group) {
                    hint.trace(backoff.recover(
                        hint,
                        configs,
                        &candidates,
                        &active,
                        Instant::now(),
                    ));
                }
            }
            let work_before = work.position;
            // Alternate control and content admission; both share the same
            // bounded pool, while incoming jobs continue in separate futures.
            while pending.len() < MAX_IN_FLIGHT {
                let mut selected = None;
                for choose_enrollment in [prefer_enrollment, !prefer_enrollment] {
                    if choose_enrollment {
                        while work.position < work.items.len() {
                            let item = &work.items[work.position];
                            work.position += 1;
                            let key = WorkKey::Enrollment(
                                item.id,
                                <[u8; 32]>::from(Sha256::digest(&item.frame)),
                                item.recipient,
                            );
                            if peer_active(&active, &key) || !backoff.due(&key, Instant::now()) {
                                continue;
                            }
                            selected = Some((
                                key.clone(),
                                enrollment_attempt(
                                    client.clone(),
                                    host.clone(),
                                    enrollment_destination,
                                    enrollment_limits,
                                    policy,
                                    item.clone(),
                                    key,
                                ),
                            ));
                            break;
                        }
                    } else {
                        for candidate in &candidates {
                            if !groups.visit(candidate.peer) {
                                continue;
                            }
                            let key = WorkKey::Sync(candidate.clone());
                            if peer_active(&active, &key) {
                                continue;
                            }
                            backoff.align_sync(candidate);
                            let push = backoff.push_state(&key);
                            let now = Instant::now();
                            let pending_record = if push.enabled(now) {
                                let scoped = client
                                    .for_group(candidate.group)
                                    .at_control(candidate.control);
                                match alongside_attempts(
                                    tokio::time::timeout(
                                        STORE_WAIT,
                                        push.pending(&scoped, candidate.peer, SystemClock.now()),
                                    ),
                                    &mut pending,
                                    &mut active,
                                    &mut backoff,
                                    policy,
                                    &mut jitter,
                                )
                                .await
                                {
                                    Ok(Ok(pending)) => pending,
                                    error => {
                                        let issue = match error {
                                            Ok(Err(ref error)) => crate::progress::issue(error),
                                            _ => SyncIssue::StorageUnavailable,
                                        };
                                        let entry = backoff.entry(key.clone(), Instant::now());
                                        entry.pending = None;
                                        entry.issue = Some(issue);
                                        backoff.publish(&key, false);
                                        continue;
                                    }
                                }
                            } else {
                                None
                            };
                            let now = Instant::now();
                            backoff.entry(key.clone(), now).pending =
                                push.enabled(now).then_some(pending_record.is_some());
                            let fresh = pending_record.is_some_and(|info| push.is_new(info));
                            let recovered =
                                std::mem::take(&mut backoff.entry(key.clone(), now).recovered);
                            let mut accelerated = false;
                            if let Some(evidence) = push.wake_evidence(pending_record, recovered) {
                                accelerated = backoff.accelerate(key.clone(), evidence, now);
                                if accelerated {
                                    backoff.trace(
                                        &key,
                                        diagnostics::Decision {
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
                                diagnostics::recovery(
                                    &candidate.group.0,
                                    candidate.identity.as_bytes(),
                                    crate::schedule::RecoveryDisposition::consumed(
                                        pending_record.is_some(),
                                        push.enabled(now),
                                        fresh,
                                        accelerated,
                                    ),
                                );
                            }
                            backoff.publish(&key, false);
                            if !backoff.due(&key, now) {
                                continue;
                            }
                            let selected_record = pending_record.filter(|_| !push.repair_due());
                            backoff.begin(key.clone(), selected_record, now);
                            backoff.publish(&key, true);
                            backoff.trace(
                                &key,
                                diagnostics::Decision {
                                    category: "admitted",
                                    activity: None,
                                    pending: Some(pending_record.is_some()),
                                    fresh,
                                    repair: selected_record.is_none(),
                                    accelerated: false,
                                },
                            );
                            selected = Some((
                                key.clone(),
                                sync_attempt(
                                    client.clone(),
                                    host.clone(),
                                    sync_destination,
                                    limits,
                                    policy,
                                    candidate.clone(),
                                    key,
                                    selected_record,
                                    push.enabled(now),
                                ),
                            ));
                            break;
                        }
                    }
                    if selected.is_some() {
                        prefer_enrollment = !choose_enrollment;
                        break;
                    }
                }
                let Some((key, attempt)) = selected else {
                    break;
                };
                active.push(key);
                pending.push(attempt);
            }
            let work_progress =
                work.loaded && (work.items.is_empty() || work.position > work_before);
            work.finish();
            if groups.selected.is_some()
                && candidates
                    .last()
                    .is_none_or(|last| groups.peer_after.is_some_and(|after| after >= last.peer))
            {
                groups.advance();
            }
            // Move through empty/filtered pages and exhausted working sets
            // promptly; sleep at a full traversal boundary. The two active
            // Link futures keep being polled during every bounded store read.
            let more =
                (config_read && groups.after.is_some()) || (work_progress && work.after.is_some());
            refresh_due = Instant::now() + if more { Duration::ZERO } else { POLL };
        }
        // Do not spin on an elapsed timer while the bounded pool is full.
        let wake_at = if pending.len() == MAX_IN_FLIGHT {
            Instant::now() + POLL
        } else {
            refresh_due
        };
        tokio::select! {
            Some(result) = pending.next(), if !pending.is_empty() => {
                completed_attempt(result, &mut active, &mut backoff, policy, &mut jitter);
                refresh_due = Instant::now();
            },
            _ = tokio::time::sleep_until(wake_at) => {},
            _ = client.wake.notified() => { refresh_due = Instant::now(); },
            _ = recoveries.changed.notified() => { refresh_due = Instant::now(); },
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn enrollment_attempt(
    client: Client,
    host: Carrier,
    destination: DestinationHash,
    limits: enrollment::Limits,
    policy: SyncPolicy,
    item: crate::enrollment::Work,
    key: WorkKey,
) -> Attempt {
    Box::pin(async move {
        let mut path = PathCapabilities::default();
        diagnostics::emit(Event::EnrollmentAttempt, "started", item.frame.len());
        let result = async {
            let network = host
                .enrollment(destination, limits)
                .and_then(|network| network.with_sync_policy(policy))
                .map_err(carrier_error)?;
            path = network
                .path_capabilities(item.recipient)
                .await
                .unwrap_or_default();
            let (response, measured) = network
                .exchange_with_capabilities(item.recipient, &item.frame)
                .await
                .map_err(carrier_error)?;
            path = measured;
            tokio::time::timeout(
                STORE_WAIT,
                client.enrollment_complete(item.id, response, now_seconds()?),
            )
            .await
            .map_err(|_| Error::Unavailable)?
        }
        .await;
        AttemptResult {
            key,
            path,
            unsupported: false,
            push_exchanged: false,
            issue: None,
            activity: match result {
                Ok(crate::enrollment::CompletionActivity::Progress) => {
                    diagnostics::emit(Event::EnrollmentOutcome, "progress", item.frame.len());
                    SyncActivity::Progress
                }
                Ok(crate::enrollment::CompletionActivity::Idle) => {
                    diagnostics::emit(Event::EnrollmentOutcome, "idle", item.frame.len());
                    SyncActivity::Idle
                }
                Err(error) => {
                    diagnostics::emit(
                        Event::EnrollmentOutcome,
                        diagnostics::error_category(&error),
                        item.frame.len(),
                    );
                    SyncActivity::Failed
                }
            },
        }
    })
}

#[allow(clippy::too_many_arguments)]
fn sync_attempt(
    client: Client,
    host: Carrier,
    destination: DestinationHash,
    limits: Limits,
    policy: SyncPolicy,
    candidate: SyncKey,
    key: WorkKey,
    pending: Option<EnvelopeInfo>,
    push_enabled: bool,
) -> Attempt {
    Box::pin(async move {
        let mut path = PathCapabilities::default();
        let mut trace = diagnostics::AttemptTrace::start(
            "outbound",
            &candidate.group.0,
            candidate.identity.as_bytes(),
        );
        let result = sync_once(
            &client,
            &host,
            destination,
            limits,
            policy,
            &candidate,
            &mut path,
            &mut trace,
            pending,
            push_enabled,
        )
        .await;
        trace.finish(match &result {
            Ok(report) => match report.activity {
                SyncActivity::Progress => "progress",
                SyncActivity::Idle => "idle",
                SyncActivity::Failed => "failed",
            },
            Err(error) => diagnostics::error_category(error),
        });
        AttemptResult {
            key,
            path,
            unsupported: result.as_ref().is_ok_and(|report| report.unsupported),
            push_exchanged: result.as_ref().is_ok_and(|report| report.push_exchanged),
            issue: match &result {
                Ok(report) => report.issue,
                Err(error) => Some(crate::progress::issue(error)),
            },
            activity: result
                .map(|report| report.activity)
                .unwrap_or_else(|error| {
                    diagnostics::emit(Event::SyncOutcome, diagnostics::error_category(&error), 0);
                    SyncActivity::Failed
                }),
        }
    })
}

#[allow(clippy::too_many_arguments)]
async fn sync_once(
    client: &Client,
    host: &Carrier,
    destination: DestinationHash,
    limits: Limits,
    policy: SyncPolicy,
    key: &SyncKey,
    path: &mut PathCapabilities,
    trace: &mut diagnostics::AttemptTrace,
    pending: Option<EnvelopeInfo>,
    push_enabled: bool,
) -> Result<crate::runner::AttemptReport, Error> {
    let config = client
        .for_group(key.group)
        .at_control(key.control)
        .network_config()
        .await?;
    if config.group != key.group
        || config.control != key.control
        || config.incarnation != key.incarnation
    {
        return Err(Error::Configuration);
    }
    let network = network(host, destination, limits, policy, &config)
        .await?
        .with_push_enabled(push_enabled);
    let binding = network.peer(key.peer).ok_or(Error::Configuration)?;
    *path = network.path_capabilities(binding).await.unwrap_or_default();
    let mut store = storage(client, config);
    trace.phase("receipt_repair", "started");
    store.client.repair_receipts().await?;
    trace.phase("receipt_repair", "complete");
    let options = SyncOptions {
        incarnation: store.config.incarnation,
        clock: SystemClock.now(),
        budget: BUDGET,
    };
    trace.phase(
        "exchange",
        if pending.is_some() { "push" } else { "repair" },
    );
    let report = exchange(&network, &mut store, key.peer, options, pending, POLL).await?;
    *path = report.path;
    trace.phase("checkpoint", "complete");
    Ok(report)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn ordered_group(number: usize) -> GroupId {
        let mut bytes = [0; 32];
        bytes[..8].copy_from_slice(&(number as u64).to_be_bytes());
        GroupId(bytes)
    }
    fn summary_page(
        group: GroupId,
        next: Option<GroupId>,
        config: &NetworkConfig,
    ) -> crate::ConversationPage {
        crate::ConversationPage {
            items: vec![crate::ConversationSummary {
                group,
                title: String::new(),
                owner_identity: None,
                lifecycle: crate::GroupLifecycle::Active,
                ready: true,
                member_count: 2,
                control: Some(config.control),
                capabilities: crate::ConversationCapabilities::for_state(
                    None,
                    config.local_reticulum_identity,
                    crate::GroupLifecycle::Active,
                    true,
                    2,
                ),
            }],
            next,
        }
    }

    #[test]
    fn group_traversal_exceeds_disposable_status_and_backoff_capacity() {
        let mut cursor = GroupTraversal::default();
        let mut backoff = Backoffs::new();
        let progress = Arc::new(crate::progress::ProgressTracker::default());
        let _lease = progress.start().unwrap();
        let (_, mut config, _) = recovery_scope();
        // Deliberately exceeds both disposable tables, with two full passes.
        // Evicting scheduling evidence cannot remove a catalog candidate.
        let count = MAX_BACKOFFS + 41;
        for _ in 0..2 {
            for index in 0..count {
                let group = ordered_group(index + 1);
                assert_eq!(cursor.after, (index > 0).then(|| ordered_group(index)));
                config.group = group;
                cursor
                    .select(summary_page(
                        group,
                        (index + 1 < count).then_some(group),
                        &config,
                    ))
                    .unwrap();
                progress.configure(std::slice::from_ref(&config));
                backoff.entry(group, Instant::now());
                assert_eq!(cursor.selected, Some(group));
                assert!(cursor.visit(config.peers[0].endpoint));
                assert!(!cursor.visit(config.peers[0].endpoint));
                cursor.advance();
            }
            assert!(cursor.after.is_none());
            assert_eq!(backoff.entries.len(), MAX_BACKOFFS);
        }
    }

    #[test]
    fn failed_configuration_read_preserves_issue_across_unrelated_empty_steps() {
        let (_, config, _) = recovery_scope();
        let group = config.group;
        let progress = Arc::new(crate::progress::ProgressTracker::default());
        let _lease = progress.start().unwrap();
        let mut cursor = GroupTraversal::default();
        cursor
            .select(summary_page(group, Some(group), &config))
            .unwrap();
        assert!(cursor
            .record_configuration(&progress, Ok(Some(config)))
            .is_some());
        assert_eq!(progress.snapshot(group).peers.len(), 1);
        assert!(cursor
            .record_configuration(&progress, Err(Error::Unavailable))
            .is_none());
        let failed = progress.snapshot(group);
        assert!(failed.observed);
        assert_eq!(failed.peers.len(), 1);
        assert_eq!(
            failed.peers[0].last_issue,
            Some(SyncIssue::StorageUnavailable)
        );
        assert_eq!(cursor.after, Some(group));
        assert!(cursor.selected.is_none());
        assert!(cursor.record_configuration(&progress, Ok(None)).is_none());
        let empty = progress.snapshot(group);
        assert!(empty.observed);
        assert_eq!(
            empty.peers[0].last_issue,
            Some(SyncIssue::StorageUnavailable)
        );
    }

    #[test]
    fn endpoint_cursor_survives_control_and_roster_changes_until_group_exhaustion() {
        let (_, mut config, _) = recovery_scope();
        let mut cursor = GroupTraversal::default();
        cursor
            .select(summary_page(config.group, None, &config))
            .unwrap();
        let peer = |byte| Endpoint {
            id: prns_groups_core::EndpointId([byte; 32]),
            ..config.local
        };
        assert!(cursor.visit(peer(2)));
        config.control.head = prns_groups_core::ControlId([99; 32]);
        // A refreshed current roster inserts an earlier endpoint and removes
        // the old one. Re-reading control does not restart this ordered range.
        assert!(!cursor.visit(peer(1)));
        assert!(cursor.visit(peer(3)));
        assert!(!cursor.visit(peer(2)));
        cursor.advance();
        cursor
            .select(summary_page(config.group, None, &config))
            .unwrap();
        assert!(
            cursor.visit(peer(1)),
            "earlier insertion appears on the next traversal"
        );
    }

    #[test]
    fn unavailable_page_retains_cursor_but_unavailable_exact_group_does_not_block_following_groups()
    {
        let (_, config, _) = recovery_scope();
        let mut cursor = GroupTraversal {
            after: Some(ordered_group(3)),
            ..Default::default()
        };
        cursor.unavailable();
        assert_eq!(cursor.after, Some(ordered_group(3)));
        let group = ordered_group(4);
        cursor
            .select(summary_page(group, Some(group), &config))
            .unwrap();
        cursor.unavailable();
        assert_eq!(cursor.after, Some(group));
        assert!(cursor.selected.is_none());
    }

    #[test]
    fn empty_filtered_pages_advance_and_work_page_is_not_skipped_mid_batch() {
        let mut groups = GroupTraversal::default();
        groups
            .select(crate::ConversationPage {
                items: Vec::new(),
                next: Some(ordered_group(4)),
            })
            .unwrap();
        assert_eq!(groups.after, Some(ordered_group(4)));
        assert!(groups.selected.is_none());
        let mut work = WorkTraversal::default();
        work.select(crate::enrollment::WorkPage {
            items: Vec::new(),
            next: Some([1; 32]),
        });
        work.finish();
        assert_eq!(work.after, Some([1; 32]));
        assert!(!work.loaded);
        work.select(crate::enrollment::WorkPage {
            items: vec![crate::enrollment::Work {
                id: [2; 32],
                recipient: prns_host::IdentityHash::new([3; 16]),
                frame: vec![4],
            }],
            next: None,
        });
        work.finish();
        assert!(
            work.loaded,
            "an unvisited row stays in the bounded working set"
        );
        assert_eq!(work.after, Some([1; 32]));
        work.position += 1;
        work.finish();
        assert!(!work.loaded);
        assert!(work.after.is_none());
    }

    #[test]
    fn one_identity_cannot_occupy_both_slots_through_control_and_multiple_groups() {
        let bob = prns_host::IdentityHash::new([2; 16]);
        let carol = prns_host::IdentityHash::new([3; 16]);
        let active = vec![WorkKey::Enrollment([1; 32], [2; 32], bob)];
        // A distinct control action to Bob still shares Bob's bounded slot.
        assert!(peer_active(
            &active,
            &WorkKey::Enrollment([3; 32], [4; 32], bob)
        ));
        assert!(!peer_active(
            &active,
            &WorkKey::Enrollment([5; 32], [6; 32], carol)
        ));
        let endpoint = Endpoint {
            id: prns_groups_core::EndpointId([7; 32]),
            incarnation: prns_groups_core::EndpointIncarnation::new(1).unwrap(),
        };
        let control = ControlReference {
            era: prns_groups_core::AuthorizationEra(prns_groups_core::ControlId([8; 32])),
            head: prns_groups_core::ControlId([8; 32]),
        };
        for group in [GroupId([1; 32]), GroupId([2; 32])] {
            assert!(peer_active(
                &active,
                &WorkKey::Sync(SyncKey {
                    group,
                    peer: endpoint,
                    identity: bob,
                    control,
                    incarnation: prns_groups_store::StoreIncarnation::new([1; 24]).unwrap(),
                })
            ));
        }
    }

    #[tokio::test]
    async fn failing_work_backs_off_without_delaying_unrelated_work() {
        let mut backoff = Backoffs::new();
        let now = Instant::now();
        let policy = SyncPolicy::default();
        let path = PathCapabilities::default();
        let first = Duration::from_millis(policy.tuning(path).retry_initial_millis);
        let maximum = Duration::from_millis(policy.tuning(path).retry_max_millis);
        backoff.record(1, SyncActivity::Failed, path, policy, now, 0);
        assert!(!backoff.due(&1, now));
        assert!(backoff.due(&2, now));
        assert!(backoff.due(&1, now + first));
        for _ in 0..32 {
            backoff.record(1, SyncActivity::Failed, path, policy, now, 0);
        }
        assert!(!backoff.accelerate(1, SyncWake::PendingWork, now));
        assert!(!backoff.due(&1, now + maximum - Duration::from_millis(1)));
        assert!(backoff.due(&1, now + maximum));
        backoff.record(1, SyncActivity::Progress, path, policy, now, 0);
        assert!(backoff.due(&1, now + first));
    }

    #[tokio::test]
    async fn retained_work_wakes_idle_peers_without_bypassing_minimum_spacing() {
        let mut backoff = Backoffs::new();
        let now = Instant::now();
        let policy = SyncPolicy::default();
        let path = PathCapabilities::default();
        backoff.record(1, SyncActivity::Idle, path, policy, now, 0);
        assert!(backoff.accelerate(1, SyncWake::PendingWork, now));
        assert!(!backoff.due(&1, now));
        assert!(backoff.due(
            &1,
            now + Duration::from_millis(policy.tuning(path).active_poll_millis)
        ));
    }

    #[tokio::test]
    async fn blocked_storage_probe_keeps_active_network_attempts_progressing() {
        let key = WorkKey::Enrollment([1; 32], [2; 32], prns_host::IdentityHash::new([3; 16]));
        let mut active = vec![key.clone()];
        let mut attempts: FuturesUnordered<Attempt> = FuturesUnordered::new();
        let (complete, unblock) = tokio::sync::oneshot::channel();
        attempts.push(Box::pin(async move {
            let _ = complete.send(());
            AttemptResult {
                key,
                path: PathCapabilities::default(),
                activity: SyncActivity::Progress,
                unsupported: false,
                push_exchanged: true,
                issue: None,
            }
        }));
        let mut backoff = Backoffs::new();
        let mut jitter = 1;
        tokio::time::timeout(
            Duration::from_secs(1),
            alongside_attempts(
                async {
                    unblock.await.unwrap();
                },
                &mut attempts,
                &mut active,
                &mut backoff,
                SyncPolicy::default(),
                &mut jitter,
            ),
        )
        .await
        .unwrap();
        assert!(active.is_empty());
        assert!(attempts.is_empty());
        assert_eq!(backoff.entries.len(), 1);
    }

    fn envelope(position: u64) -> EnvelopeInfo {
        EnvelopeInfo {
            position: prns_groups_service::InventoryCursor(position),
            id: prns_groups_service::EnvelopeId([1; 32]),
            kind: prns_groups_service::InventoryKind::Record,
            record: prns_groups_service::RecordId([2; 32]),
            bytes: 128,
        }
    }

    fn recovery_scope() -> (SyncKey, NetworkConfig, crate::schedule::RecoveryHint) {
        let peer = Endpoint {
            id: prns_groups_core::EndpointId([2; 32]),
            incarnation: prns_groups_core::EndpointIncarnation::new(1).unwrap(),
        };
        let identity = prns_host::IdentityHash::new([3; 16]);
        let key = SyncKey {
            group: GroupId([1; 32]),
            peer,
            identity,
            control: ControlReference {
                head: prns_groups_core::ControlId([4; 32]),
                era: prns_groups_core::AuthorizationEra(prns_groups_core::ControlId([4; 32])),
            },
            incarnation: prns_groups_store::StoreIncarnation::new([5; 24]).unwrap(),
        };
        let config = NetworkConfig {
            group: key.group,
            control: key.control,
            incarnation: key.incarnation,
            local: Endpoint {
                id: prns_groups_core::EndpointId([6; 32]),
                ..peer
            },
            local_reticulum_identity: prns_host::IdentityHash::new([7; 16]),
            readers: vec![peer],
            peers: vec![prns_groups_reticulum::Peer {
                endpoint: peer,
                identity,
                destination: prns_host::DestinationHash::new([8; 16]),
            }],
        };
        let hint = crate::schedule::RecoveryHint {
            group: key.group,
            control: key.control,
            incarnation: key.incarnation,
            peer,
            identity,
        };
        (key, config, hint)
    }

    #[test]
    fn unrelated_group_cannot_accelerate_but_exact_hint_waits_for_busy_peer_slot() {
        use crate::schedule::RecoveryDisposition as D;
        let (candidate, config, hint) = recovery_scope();
        let now = Instant::now();
        let key = WorkKey::Sync(candidate.clone());
        let mut backoff = Backoffs::new();
        for position in 1..=4 {
            backoff.begin(key.clone(), Some(envelope(position)), now);
            backoff.entry(key.clone(), now).push.exchanged();
        }
        for _ in 0..8 {
            backoff.record(
                key.clone(),
                SyncActivity::Failed,
                PathCapabilities::default(),
                SyncPolicy::default(),
                now,
                0,
            );
        }
        assert_eq!(
            backoff.entry(key.clone(), now).due,
            now + Duration::from_secs(300)
        );
        let other = SyncKey {
            group: GroupId([9; 32]),
            ..candidate.clone()
        };
        let other_config = NetworkConfig {
            group: other.group,
            ..config.clone()
        };
        let other_hint = crate::schedule::RecoveryHint {
            group: other.group,
            ..hint
        };
        let configs = [config, other_config];
        let candidates = [candidate.clone(), other.clone()];
        assert_eq!(
            backoff.recover(other_hint, &configs, &candidates, &[], now),
            D::NotFailed
        );
        assert!(!backoff.entry(key.clone(), now).recovered);
        assert_eq!(
            backoff.recover(hint, &configs, &candidates, &[WorkKey::Sync(other)], now),
            D::PeerActive
        );
        assert!(backoff.entry(key.clone(), now).recovered);
        assert!(!backoff.accelerate(key.clone(), SyncWake::PendingWork, now));
        assert!(!backoff.due(&key, now + Duration::from_secs(299)));
        // The other group's attempt has now finished. No second inbound hint
        // is required: the already authenticated exact-scope evidence survived.
        let recovered = std::mem::take(&mut backoff.entry(key.clone(), now).recovered);
        let push = backoff.push_state(&key);
        let evidence = push.wake_evidence(Some(envelope(4)), recovered).unwrap();
        assert_eq!(evidence, SyncWake::PeerRecovered);
        assert!(backoff.accelerate(key.clone(), evidence, now));
        assert!(!backoff.due(&key, now + Duration::from_secs(14)));
        assert!(backoff.due(&key, now + Duration::from_secs(15)));
        assert_eq!(backoff.entry(key.clone(), now).state.failures(), 8);
        assert!(backoff.push_state(&key).repair_due());
        backoff.record(
            key.clone(),
            SyncActivity::Failed,
            PathCapabilities::default(),
            SyncPolicy::default(),
            now + Duration::from_secs(15),
            0,
        );
        assert_eq!(backoff.entry(key.clone(), now).state.failures(), 9);
        assert!(!backoff.due(&key, now + Duration::from_secs(314)));
        assert!(backoff.due(&key, now + Duration::from_secs(315)));
    }

    #[test]
    fn raw_recovery_dispositions_keep_authorization_and_current_candidate_fences() {
        use crate::schedule::RecoveryDisposition as D;
        let (candidate, config, hint) = recovery_scope();
        let mut backoff = Backoffs::new();
        let now = Instant::now();
        let candidates = [candidate];
        assert_eq!(
            backoff.recover(hint, &[], &candidates, &[], now),
            D::ScopeStale
        );
        let configs = [config];
        assert_eq!(
            backoff.recover(hint, &configs, &[], &[], now),
            D::CandidateMissing
        );
        for stale in [
            crate::schedule::RecoveryHint {
                group: GroupId([10; 32]),
                ..hint
            },
            crate::schedule::RecoveryHint {
                control: ControlReference {
                    head: prns_groups_core::ControlId([10; 32]),
                    ..hint.control
                },
                ..hint
            },
            crate::schedule::RecoveryHint {
                incarnation: prns_groups_store::StoreIncarnation::new([10; 24]).unwrap(),
                ..hint
            },
            crate::schedule::RecoveryHint {
                identity: prns_host::IdentityHash::new([10; 16]),
                ..hint
            },
        ] {
            assert_eq!(
                backoff.recover(stale, &configs, &candidates, &[], now),
                D::ScopeStale
            );
        }
        assert!(backoff.entries.is_empty());
        assert_eq!(
            backoff.recover(hint, &configs, &candidates, &[], now),
            D::NotFailed
        );
        assert!(!backoff.entries[0].recovered);
    }

    #[test]
    fn fresh_work_shortens_failures_at_floor_without_erasing_exponent() {
        let mut backoff = Backoffs::new();
        let now = Instant::now();
        let policy = SyncPolicy::default();
        let path = PathCapabilities::default();
        let floor = now + Duration::from_secs(15);
        for _ in 0..8 {
            backoff.record(1, SyncActivity::Failed, path, policy, now, 0);
        }
        backoff.begin(1, Some(envelope(1)), now);
        let push = backoff.push_state(&1);
        assert!(!push.is_new(envelope(1)));
        assert!(push.is_new(envelope(2)));
        assert!(!backoff.accelerate(1, SyncWake::PendingWork, now));
        assert!(backoff.accelerate(1, SyncWake::FreshWork, now));
        assert!(!backoff.due(&1, floor - Duration::from_millis(1)));
        assert!(backoff.due(&1, floor));
        assert!(!backoff.accelerate(1, SyncWake::PeerRecovered, now));
        backoff.record(1, SyncActivity::Failed, path, policy, floor, 0);
        assert!(!backoff.due(&1, floor + Duration::from_secs(299)));
        assert!(backoff.due(&1, floor + Duration::from_secs(300)));
    }

    #[test]
    fn repair_with_pending_work_resumes_at_floor_but_keeps_four_push_fence() {
        let mut backoff = Backoffs::new();
        let now = Instant::now();
        for position in 1..=4 {
            backoff.begin(1, Some(envelope(position)), now);
            backoff.entry(1, now).push.exchanged();
        }
        assert!(backoff.push_state(&1).repair_due());
        backoff.begin(1, None, now);
        assert!(!backoff.push_state(&1).repair_due());
        backoff.record(
            1,
            SyncActivity::Idle,
            PathCapabilities::default(),
            SyncPolicy::default(),
            now,
            0,
        );
        assert!(backoff.accelerate(1, SyncWake::PendingWork, now));
        assert!(!backoff.due(&1, now + Duration::from_secs(14)));
        assert!(backoff.due(&1, now + Duration::from_secs(15)));
        assert!(!backoff.push_state(&1).is_new(envelope(4)));
    }

    #[test]
    fn control_changes_preserve_retry_watermark_and_spacing() {
        let mut backoff = Backoffs::new();
        let now = Instant::now();
        let mut candidate = SyncKey {
            group: GroupId([1; 32]),
            peer: Endpoint {
                id: prns_groups_core::EndpointId([2; 32]),
                incarnation: prns_groups_core::EndpointIncarnation::new(1).unwrap(),
            },
            identity: prns_host::IdentityHash::new([3; 16]),
            control: ControlReference {
                head: prns_groups_core::ControlId([4; 32]),
                era: prns_groups_core::AuthorizationEra(prns_groups_core::ControlId([4; 32])),
            },
            incarnation: prns_groups_store::StoreIncarnation::new([5; 24]).unwrap(),
        };
        let old = WorkKey::Sync(candidate.clone());
        backoff.begin(old.clone(), Some(envelope(10)), now);
        for _ in 0..10 {
            backoff.record(
                old.clone(),
                SyncActivity::Failed,
                PathCapabilities::default(),
                SyncPolicy::default(),
                now,
                0,
            );
        }
        for head in 6..30 {
            // A validated hint can remain queued after a stale-control store
            // read fails. It must not cross the next authorization scope.
            backoff.entries[0].recovered = true;
            candidate.control.head = prns_groups_core::ControlId([head; 32]);
            backoff.align_sync(&candidate);
            assert!(
                !backoff.entries[0].recovered,
                "control change retained stale recovery credit"
            );
            let key = WorkKey::Sync(candidate.clone());
            assert!(!backoff.push_state(&key).is_new(envelope(10)));
            assert!(!backoff.accelerate(key.clone(), SyncWake::PendingWork, now));
            assert!(!backoff.due(&key, now + Duration::from_secs(299)));
        }
        assert_eq!(backoff.entries.len(), 1);
        backoff.entries[0].recovered = true;
        candidate.incarnation = prns_groups_store::StoreIncarnation::new([9; 24]).unwrap();
        backoff.align_sync(&candidate);
        assert!(
            !backoff.entries[0].recovered,
            "store change retained stale recovery credit"
        );
        let key = WorkKey::Sync(candidate);
        assert!(backoff.push_state(&key).is_new(envelope(1)));
        assert!(backoff.accelerate(key.clone(), SyncWake::FreshWork, now));
        assert!(!backoff.due(&key, now + Duration::from_secs(14)));
        assert!(backoff.due(&key, now + Duration::from_secs(15)));
    }

    #[tokio::test]
    async fn churn_does_not_grow_scheduling_memory_without_bound() {
        let mut backoff = Backoffs::new();
        for key in 0..MAX_BACKOFFS * 3 {
            backoff.record(
                key,
                SyncActivity::Failed,
                PathCapabilities::default(),
                SyncPolicy::default(),
                Instant::now(),
                0,
            );
        }
        assert_eq!(backoff.entries.len(), MAX_BACKOFFS);
        assert!(backoff.due(&0, Instant::now()));
    }

    struct DeferredOwner(mpsc::UnboundedSender<crate::Job>, crate::InputBudget);
    impl crate::Submit for DeferredOwner {
        fn input_budget(&self) -> &crate::InputBudget {
            &self.1
        }
        fn submit(&self, job: crate::Job) -> Result<(), Error> {
            self.0.send(job).map_err(|_| Error::Unavailable)
        }
    }

    #[tokio::test]
    async fn stop_drops_the_waiter_but_preserves_the_admitted_owner_job_and_host() {
        use prns_host::{
            DestinationConfig, HostConfig, HostRole, IdentityConfig, PersistenceConfig, PrnsLimits,
        };
        let sync_limits = Limits {
            max_frame_bytes: 8192,
            max_page_items: 8,
            max_peers: 16,
            inbound_jobs: 8,
            exchange_timeout: Duration::from_secs(1),
        };
        let enrollment_limits = enrollment::Limits {
            max_frame_bytes: enrollment::MAX_FRAME_BYTES,
            inbound_jobs: 8,
            exchange_timeout: Duration::from_secs(1),
        };
        let host = prns_host_runtime::owner::OwnedSession::open(HostConfig {
            identity: IdentityConfig::GenerateEphemeral,
            persistence: PersistenceConfig::Ephemeral,
            role: HostRole::Endpoint,
            destinations: vec![
                DestinationConfig::Single(
                    prns_groups_reticulum::destination_config(sync_limits).unwrap(),
                ),
                DestinationConfig::Single(
                    enrollment::destination_config(enrollment_limits).unwrap(),
                ),
            ],
            required_capabilities: vec![],
            limits: PrnsLimits::balanced(),
        })
        .await
        .unwrap();
        let identity = host.client().identity_hash().unwrap();
        let (admit, mut admitted) = mpsc::unbounded_channel();
        let client = Client::new(std::sync::Arc::new(DeferredOwner(
            admit,
            crate::InputBudget::default(),
        )));
        let (_failed_sync_sender, failed_sync) = mpsc::channel(1);
        let (_failed_enroll_sender, failed_enroll) = mpsc::channel(1);
        let (_failed_stop, failed_stopped) = watch::channel(false);
        let refused = run_multi(
            client.clone(),
            host.client(),
            prns_groups_reticulum::destination_for(identity).unwrap(),
            enrollment::destination_for(identity).unwrap(),
            failed_sync,
            failed_enroll,
            sync_limits,
            enrollment_limits,
            failed_stopped,
        );
        let reject_support = async {
            let job = admitted.recv().await.unwrap();
            assert!(matches!(
                job.command,
                crate::client::Command::ProfileSupport(_)
            ));
            job.fail(Error::Enrollment(
                crate::enrollment::EnrollmentError::UnsupportedProfile,
            ));
        };
        let (refused, ()) = tokio::time::timeout(Duration::from_secs(2), async {
            tokio::join!(refused, reject_support)
        })
        .await
        .unwrap();
        assert!(matches!(
            refused,
            Err(Error::Enrollment(
                crate::enrollment::EnrollmentError::UnsupportedProfile
            ))
        ));
        assert!(matches!(
            admitted.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
        let (_sync_sender, sync_jobs) = mpsc::channel(1);
        let (_enroll_sender, enroll_jobs) = mpsc::channel(1);
        let (stop, stopped) = watch::channel(false);
        let runner = run_multi(
            client,
            host.client(),
            prns_groups_reticulum::destination_for(identity).unwrap(),
            enrollment::destination_for(identity).unwrap(),
            sync_jobs,
            enroll_jobs,
            sync_limits,
            enrollment_limits,
            stopped,
        );
        let controller = async {
            let saturated = tokio::time::timeout(Duration::from_secs(2), admitted.recv())
                .await
                .unwrap()
                .unwrap();
            assert!(matches!(
                saturated.command,
                crate::client::Command::ProfileSupport(_)
            ));
            saturated.fail(Error::Busy);
            let support_job = tokio::time::timeout(Duration::from_secs(4), admitted.recv())
                .await
                .unwrap()
                .unwrap();
            assert!(matches!(
                support_job.command,
                crate::client::Command::ProfileSupport(_)
            ));
            let _ = support_job.reply.send(Ok(crate::client::Reply::Done));
            let identity_busy = tokio::time::timeout(Duration::from_secs(2), admitted.recv())
                .await
                .unwrap()
                .unwrap();
            assert!(matches!(
                identity_busy.command,
                crate::client::Command::Identity
            ));
            identity_busy.fail(Error::Busy);
            let identity_job = tokio::time::timeout(Duration::from_secs(4), admitted.recv())
                .await
                .unwrap()
                .unwrap();
            assert!(matches!(
                identity_job.command,
                crate::client::Command::Identity
            ));
            let _ = identity_job
                .reply
                .send(Ok(crate::client::Reply::Identity(identity)));
            let job = tokio::time::timeout(Duration::from_secs(2), admitted.recv())
                .await
                .unwrap()
                .unwrap();
            assert!(matches!(job.command, crate::client::Command::Maintain(_)));
            stop.send(true).unwrap();
            job
        };
        let (result, admitted_job) = tokio::time::timeout(Duration::from_secs(7), async {
            tokio::join!(runner, controller)
        })
        .await
        .expect("stop does not wait for the blocked database owner");
        result.unwrap();
        assert!(admitted_job.reply.is_closed());
        // The owner still holds the admitted job and chooses its terminal result.
        admitted_job.fail(Error::Unavailable);
        assert!(host.client().snapshot().await.unwrap().runtime.running);
        host.stop().await.unwrap();
    }
}
