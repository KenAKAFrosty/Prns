//! Opt-in real managed MLS membership workloads with four representative hosts.
//! Membership count and concurrent authenticated-Link host count are distinct.
use super::{journey, wire};
use prns_groups::{
    announce_control,
    enrollment::{CatalogLimits, InvitationState, Work, WorkPage},
    owned::{LocalOptions, LocalOwner},
    run_multi_scoped_with_announcements, Client, Clock, ConversationMember, GroupLifecycle,
    GroupProfile, IdentityHash, Intent, MemberPage, Outcome, PageLimits, PublicationIntent,
    SyncPolicy, SystemClock, Targets,
};
use prns_groups_core::{Endpoint, GroupId, RecordId};
use prns_groups_store_redb::RedbLimits;
use std::{
    collections::BTreeMap,
    mem::size_of,
    num::NonZeroUsize,
    os::unix::fs::PermissionsExt,
    path::Path,
    sync::atomic::Ordering,
    time::{Duration, Instant},
};
use tokio::{sync::watch, task::JoinSet};

const PROFILE: GroupProfile = GroupProfile {
    max_members: 256,
    max_record_body_bytes: 2054,
    max_plaintext_bytes: 16_384,
    max_wire_bytes: 65_536,
    max_snapshot_bytes: 262_144,
    max_enrollment_frame_bytes: 524_288,
};
const PAGE: PageLimits = PageLimits {
    max_rows: 17,
    max_bytes: 4096,
};
const WORK: PageLimits = PageLimits {
    max_rows: 4,
    max_bytes: 2 * 1024 * 1024 + 4096,
};

fn options(validation_cache_entries: Option<NonZeroUsize>) -> LocalOptions {
    let options = LocalOptions::new(NonZeroUsize::new(32).unwrap(), 1024 * 1024)
        .with_catalog_limits(CatalogLimits {
            max_groups: None,
            max_catalog_bytes: 256 * 1024 * 1024,
            max_enrollment_frame_bytes: PROFILE.max_enrollment_frame_bytes,
            max_row_bytes: 1024 * 1024,
            max_transaction_rows: 512,
            max_transaction_bytes: 8 * 1024 * 1024,
            max_maintenance_rows: 32,
            max_incoming_invitations: 4,
            max_incoming_per_peer: 2,
            namespace: RedbLimits {
                max_changes: 512,
                max_key_bytes: 256,
                max_value_bytes: 4 * 1024 * 1024,
                max_staged_bytes: 8 * 1024 * 1024,
                max_live_objects: 65_536,
                max_live_bytes: 256 * 1024 * 1024,
                max_receipts: 16_384,
            },
        })
        .with_archive(prns_groups::archive::Limits {
            max_records: 256,
            max_bytes: 8 * 1024 * 1024,
        });
    match validation_cache_entries {
        Some(entries) => options.with_validation_cache_entries(entries),
        None => options,
    }
}

#[derive(Default, serde::Serialize)]
struct Relay {
    pages: u64,
    frames: u64,
    bytes: u64,
    max_owned_page_bytes: usize,
    maintenance_calls: u64,
    maintenance_micros: u128,
    work_read_micros: u128,
    handle_micros: u128,
    complete_micros: u128,
    convergence_checks: u64,
    convergence_micros: u128,
}
impl Relay {
    /// Only routes genuine public Work by the source owner's authenticated
    /// identity binding. It never constructs protocol messages or commits state.
    async fn sweep(&mut self, clients: &[Client], routes: &BTreeMap<[u8; 16], usize>) -> usize {
        let before = self.frames;
        for (source, client) in clients.iter().enumerate() {
            let maintenance = Instant::now();
            client.maintain(SystemClock.now()).await.unwrap();
            self.maintenance_calls += 1;
            self.maintenance_micros += maintenance.elapsed().as_micros();
            let identity = client.identity().await.unwrap();
            assert_eq!(routes[identity.as_bytes()], source);
            let mut after = None;
            loop {
                let read = Instant::now();
                let page = client
                    .enrollment_work(journey::now(), after, WORK)
                    .await
                    .unwrap();
                self.work_read_micros += read.elapsed().as_micros();
                let owned = size_of::<WorkPage>()
                    + page.items.capacity() * size_of::<Work>()
                    + page
                        .items
                        .iter()
                        .map(|row| row.frame.capacity())
                        .sum::<usize>();
                assert!(owned as u64 <= WORK.max_bytes);
                assert!(page.items.len() <= WORK.max_rows as usize);
                self.max_owned_page_bytes = self.max_owned_page_bytes.max(owned);
                self.pages += 1;
                for work in page.items {
                    let destination = routes[work.recipient.as_bytes()];
                    assert!(
                        destination < clients.len(),
                        "work cannot target an unadmitted peer"
                    );
                    assert_ne!(source, destination);
                    self.bytes += work.frame.len() as u64;
                    self.frames += 1;
                    let handling = Instant::now();
                    let response = clients[destination]
                        .enrollment_handle(identity, work.frame, journey::now())
                        .await
                        .unwrap();
                    self.handle_micros += handling.elapsed().as_micros();
                    let completion = Instant::now();
                    client
                        .enrollment_complete(work.id, response, journey::now())
                        .await
                        .unwrap();
                    self.complete_micros += completion.elapsed().as_micros();
                }
                match page.next {
                    Some(next) => {
                        assert!(after.is_none_or(|after| next > after));
                        after = Some(next);
                    }
                    None => break,
                }
            }
        }
        (self.frames - before) as usize
    }

    async fn until<F, Done>(
        &mut self,
        clients: &[Client],
        routes: &BTreeMap<[u8; 16], usize>,
        group: GroupId,
        stage: &str,
        mut complete: F,
    ) where
        F: FnMut() -> Done,
        Done: std::future::Future<Output = bool>,
    {
        // Active memberships intentionally keep producing Sync probes. Deliver
        // them all, but use authoritative public state to establish completion.
        for _ in 0..128 {
            self.sweep(clients, routes).await;
            let query = Instant::now();
            let finished = complete().await;
            self.convergence_checks += 1;
            self.convergence_micros += query.elapsed().as_micros();
            if finished {
                return;
            }
        }
        for (index, client) in clients.iter().enumerate() {
            eprintln!(
                "member-lifecycle-failure stage={stage} client={index} detail={:?}",
                client.conversation(group, PAGE).await
            );
        }
        panic!("public enrollment/maintenance did not converge during {stage}");
    }
}

async fn converged(
    clients: &[Client],
    group: GroupId,
    members: usize,
    removed: &[usize],
    closed: bool,
) -> bool {
    let mut control = None;
    for (index, client) in clients.iter().enumerate() {
        let detail = match client.conversation(group, PAGE).await {
            Ok(detail) => detail,
            Err(prns_groups::ApiError::NotProvisioned) => return false,
            Err(error) => panic!("exact public membership read failed: {error:?}"),
        };
        // Authenticated group Close also supersedes an earlier local Remove.
        // Removed peers retain their earlier roster/control, not Active state.
        let expected = if closed {
            GroupLifecycle::Closed
        } else if removed.contains(&index) {
            GroupLifecycle::Removed
        } else {
            GroupLifecycle::Active
        };
        if detail.lifecycle != expected {
            return false;
        }
        if expected == GroupLifecycle::Active {
            if detail.member_count != members as u64
                || control.is_some_and(|current| current != detail.control)
            {
                return false;
            }
            control = Some(detail.control);
        }
    }
    true
}

async fn invitation(client: &Client, id: prns_groups::InvitationId) -> InvitationState {
    let mut after = None;
    loop {
        let page = client
            .invitations(journey::now(), after, PAGE)
            .await
            .unwrap();
        if let Some(found) = page.items.iter().find(|row| row.id == id) {
            return found.state;
        }
        match page.next {
            Some(next) => {
                assert!(after.is_none_or(|after| next > after));
                after = Some(next);
            }
            None => panic!("real retained invitation is missing"),
        }
    }
}

async fn roster(client: &Client, group: GroupId, expected: usize) -> BTreeMap<[u8; 16], Endpoint> {
    let detail = client.conversation(group, PAGE).await.unwrap();
    assert_eq!(detail.profile, Some(PROFILE));
    assert_eq!(detail.member_count, expected as u64);
    let mut after = None;
    let mut result = BTreeMap::new();
    let mut pages = 0;
    loop {
        let page = client.members(group, after, PAGE).await.unwrap();
        let owned =
            size_of::<MemberPage>() + page.members.capacity() * size_of::<ConversationMember>();
        assert!(owned as u64 <= PAGE.max_bytes);
        assert!(page.members.len() <= PAGE.max_rows as usize);
        assert_eq!(
            page.control, detail.control,
            "quiescent roster control remains exact"
        );
        assert!(page
            .members
            .windows(2)
            .all(|rows| rows[0].endpoint < rows[1].endpoint));
        for member in page.members {
            assert!(after.is_none_or(|after| member.endpoint > after));
            assert!(result
                .insert(*member.identity.as_bytes(), member.endpoint)
                .is_none());
        }
        pages += 1;
        match page.next {
            Some(next) => {
                assert!(after.is_none_or(|after| next > after));
                after = Some(next);
            }
            None => break,
        }
    }
    assert_eq!(result.len(), expected);
    assert!(
        expected <= PAGE.max_rows as usize || pages > 1,
        "larger selected roster is actually paginated"
    );
    result
}

async fn delivered(client: &Client, group: GroupId, record: RecordId, receipt: bool) {
    loop {
        let page = client
            .records(group, None, 16, SystemClock.now())
            .await
            .unwrap();
        if page.records.iter().any(|row| {
            row.record.metadata.record == record
                && (!receipt || row.progress.summary.all_received())
        }) {
            return;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

struct NetworkDiagnostics;
impl log::Log for NetworkDiagnostics {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.target().starts_with("prns.groups.")
    }
    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            eprintln!(
                "member-network-diagnostic {} {}",
                record.target(),
                record.args()
            );
        }
    }
    fn flush(&self) {}
}

struct NetworkMeasurement {
    elapsed_ms: u128,
    link_frames: usize,
    resources: usize,
    resource_proofs: usize,
    records: [RecordId; 2],
}

async fn network_stage<T>(
    stage: &str,
    tasks: &mut JoinSet<Result<(), prns_groups::Error>>,
    nodes: &wire::Nodes,
    clients: &[Client],
    group: GroupId,
    operation: impl std::future::Future<Output = T>,
) -> T {
    eprintln!("member-network-progress stage={stage} state=started");
    let result = tokio::select! {
        result = tokio::time::timeout(Duration::from_secs(30), operation) => result,
        task = tasks.join_next() => panic!("network runner exited during {stage}: {task:?}"),
    };
    let frames: usize = nodes
        .state
        .frames
        .iter()
        .map(|n| n.load(Ordering::SeqCst))
        .sum();
    eprintln!(
        "member-network-progress stage={stage} completed={} frames={frames} resources={} proofs={}",
        result.is_ok(),
        nodes.state.resources.load(Ordering::SeqCst),
        nodes.state.resource_proofs.load(Ordering::SeqCst)
    );
    if result.is_err() {
        for (index, scope) in nodes.scopes.iter().enumerate() {
            eprintln!(
                "member-network-failure stage={stage} host={index} scope={:?}",
                scope.check_open()
            );
        }
        for (index, client) in clients.iter().enumerate() {
            eprintln!(
                "member-network-failure stage={stage} client={index} progress={:?}",
                client.synchronization_progress(group)
            );
        }
    }
    result.unwrap_or_else(|_| panic!("network stage {stage} exceeded 30seconds"))
}

fn disk_bytes(paths: &[impl AsRef<Path>]) -> u64 {
    paths
        .iter()
        .map(|path| std::fs::metadata(path).unwrap().len())
        .sum()
}

/// Public read costs only: no storage-commit/validation count is inferred from
/// latency. Active Sync work is read but not delivered during this idle phase.
async fn idle_reads(owner: &Client, late: &Client, group: GroupId) -> serde_json::Value {
    let mut results = Vec::new();
    for (role, client) in [("owner", owner), ("late_member", late)] {
        let mut detail_micros = 0_u128;
        let mut member_micros = 0_u128;
        let mut work_micros = 0_u128;
        let mut work_pages = 0_u64;
        let mut work_frames = 0_usize;
        let mut member_pages = 0_u64;
        for _ in 0..10 {
            let detail = Instant::now();
            let selected = client.conversation(group, PAGE).await.unwrap();
            detail_micros += detail.elapsed().as_micros();
            let members = Instant::now();
            let mut after = None;
            loop {
                let page = client.members(group, after, PAGE).await.unwrap();
                assert_eq!(page.control, selected.control);
                member_pages += 1;
                match page.next {
                    Some(next) => {
                        assert!(after.is_none_or(|after| next > after));
                        after = Some(next);
                    }
                    None => break,
                }
            }
            member_micros += members.elapsed().as_micros();
            let work = Instant::now();
            let mut after = None;
            loop {
                let page = client
                    .enrollment_work(journey::now(), after, WORK)
                    .await
                    .unwrap();
                work_pages += 1;
                work_frames += page.items.len();
                match page.next {
                    Some(next) => {
                        assert!(after.is_none_or(|after| next > after));
                        after = Some(next);
                    }
                    None => break,
                }
            }
            work_micros += work.elapsed().as_micros();
        }
        results.push(serde_json::json!({
            "client": role,
            "detail": { "calls": 10, "total_micros": detail_micros, "mean_micros": detail_micros / 10 },
            "member_traversal": { "calls": 10, "total_micros": member_micros, "mean_micros": member_micros / 10 },
            "member_pages": member_pages,
            "work_poll": { "calls": 10, "total_micros": work_micros, "mean_micros": work_micros / 10 },
            "work_pages": work_pages,
            "work_frames_observed_not_delivered": work_frames,
        }));
    }
    serde_json::json!({
        "clients": results, "iterations_each": 10,
        "scope": "per-client public query elapsed time; no inferred internal storage counters"
    })
}

/// Optional full target; smaller representative workloads are sufficient for
/// current development acceptance. No device/RF or 256-carrier claim is made.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "optional 256 real managed memberships / four authenticated-Link hosts"]
async fn two_hundred_fifty_six_members_public_enrollment_and_late_member_links() {
    qualify_members(256, NonZeroUsize::new(1), true, true).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "explicit representative membership and UX measurement"]
async fn eight_members_representative_workload() {
    qualify_members(8, NonZeroUsize::new(1), false, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "explicit representative membership and UX measurement"]
async fn sixteen_members_representative_workload() {
    qualify_members(16, NonZeroUsize::new(1), false, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "explicit representative membership and UX measurement"]
async fn thirty_two_members_representative_workload() {
    qualify_members(32, NonZeroUsize::new(1), false, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "explicit representative membership and UX measurement"]
async fn sixteen_strict_members_representative_workload() {
    qualify_members(16, None, false, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "explicit four-Link journey, separate from representative query timings"]
async fn eight_members_four_link_messaging_journey() {
    qualify_members(8, NonZeroUsize::new(1), true, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "explicit management lifecycle; separate from performance and Link journey"]
async fn eight_members_management_lifecycle() {
    qualify_members(8, NonZeroUsize::new(1), false, true).await;
}

async fn qualify_members(
    member_count: usize,
    validation_cache_entries: Option<NonZeroUsize>,
    with_network: bool,
    with_lifecycle: bool,
) {
    assert!((4..=PROFILE.max_members as usize).contains(&member_count));
    let online = [0, member_count - 3, member_count - 2, member_count - 1];
    let removed_index = member_count - 3;
    let late = member_count - 1;
    let removed_members = [removed_index];
    let departed_members = [removed_index, late];
    PROFILE.validate().unwrap();
    let total = Instant::now();
    let directory = tempfile::tempdir().unwrap();
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let transport = prns_groups_reticulum::Limits {
        max_peers: member_count,
        exchange_timeout: Duration::from_secs(2),
        ..wire::limits()
    };
    let enrollment = prns_groups_reticulum::enrollment::Limits {
        max_frame_bytes: PROFILE.max_enrollment_frame_bytes as usize,
        exchange_timeout: Duration::from_secs(2),
        ..wire::enrollment_limits()
    };
    let mut nodes = wire::Nodes::open_scoped_with_limits(transport, enrollment).await;
    let mut routes = BTreeMap::new();
    let mut identities = Vec::new();
    let mut owners = Vec::new();
    let mut paths = Vec::new();
    let startup = Instant::now();
    for index in 0..member_count {
        let identity = if let Some(slot) = online.iter().position(|candidate| *candidate == index) {
            nodes.scopes[slot].identity_hash()
        } else {
            let mut bytes = [0xCD; 16];
            bytes[..8].copy_from_slice(&(index as u64).to_be_bytes());
            IdentityHash::new(bytes)
        };
        assert!(routes.insert(*identity.as_bytes(), index).is_none());
        let path = directory.path().join(format!("member-{index:03}.redb"));
        owners.push(
            LocalOwner::create_new(&path, identity, options(validation_cache_entries)).unwrap(),
        );
        paths.push(path);
        identities.push(identity);
    }
    let startup_ms = startup.elapsed().as_millis();
    eprintln!(
        "member-stage-result {}",
        serde_json::json!({
            "stage": "startup", "members": member_count, "elapsed_ms": startup_ms,
            "validation_cache_entries_per_owner": validation_cache_entries.map(NonZeroUsize::get),
            "profile": PROFILE,
        })
    );
    let clients: Vec<_> = owners.iter().map(LocalOwner::client).collect();
    let Outcome::Created(group) = journey::execute(
        &clients[0],
        Intent::Create {
            title: format!("{member_count} real managed memberships"),
            profile: PROFILE,
        },
    )
    .await
    else {
        panic!("real public Create")
    };
    let mut relay = Relay::default();
    let admission = Instant::now();
    let mut joins = Vec::with_capacity(member_count - 1);
    for index in 1..member_count {
        let join = Instant::now();
        let frames_before = relay.frames;
        let offer = journey::invite(&clients[0], group, identities[index]).await;
        relay.sweep(&clients[..=index], &routes).await;
        assert_eq!(
            invitation(&clients[index], offer).await,
            InvitationState::Pending
        );
        let before = clients[index].conversations(None, PAGE).await.unwrap();
        assert!(
            before.items.is_empty() && before.next.is_none(),
            "no membership before consent"
        );
        journey::execute(
            &clients[index],
            Intent::Respond {
                invitation: offer,
                accept: true,
            },
        )
        .await;
        relay
            .until(&clients[..=index], &routes, group, "join", || async {
                converged(&clients[..=index], group, index + 1, &[], false).await
                    && invitation(&clients[0], offer).await == InvitationState::Joined
                    && invitation(&clients[index], offer).await == InvitationState::Joined
            })
            .await;
        assert_eq!(
            invitation(&clients[0], offer).await,
            InvitationState::Joined
        );
        assert_eq!(
            invitation(&clients[index], offer).await,
            InvitationState::Joined
        );
        assert_eq!(
            clients[0]
                .conversation(group, PAGE)
                .await
                .unwrap()
                .member_count,
            (index + 1) as u64
        );
        joins.push(serde_json::json!({
            "members": index + 1,
            "elapsed_ms": join.elapsed().as_millis(),
            "frames": relay.frames - frames_before,
        }));
        if [8, 16, 32, 64, 128, 256].contains(&(index + 1)) {
            eprintln!("member-scale-progress members={} elapsed_ms={} frames={} frame_bytes={} database_bytes={}",
                index + 1, admission.elapsed().as_millis(), relay.frames, relay.bytes, disk_bytes(&paths));
        }
    }
    let admission_ms = admission.elapsed().as_millis();
    let admission_relay = serde_json::to_value(&relay).unwrap();
    eprintln!(
        "member-stage-result {}",
        serde_json::json!({
            "stage": "admission", "members": member_count, "elapsed_ms": admission_ms,
            "joins": joins, "public_work": admission_relay,
        })
    );
    let inspection = Instant::now();
    let head = clients[0].conversation(group, PAGE).await.unwrap().control;
    for client in &clients {
        let detail = client.conversation(group, PAGE).await.unwrap();
        assert_eq!(detail.member_count, member_count as u64);
        assert_eq!(detail.control, head);
        assert_eq!(detail.profile, Some(PROFILE));
    }
    let members = roster(&clients[0], group, member_count).await;
    assert_eq!(roster(&clients[late], group, member_count).await, members);
    let inspection_ms = inspection.elapsed().as_millis();
    let idle = idle_reads(&clients[0], &clients[late], group).await;
    eprintln!(
        "member-stage-result {}",
        serde_json::json!({
            "stage": "inspection_and_idle", "inspection_ms": inspection_ms, "idle_reads": idle,
        })
    );

    let network_result = if with_network {
        if std::env::var_os("PRNS_GROUP_FIXTURE_DIAGNOSTICS").is_some() {
            static LOGGER: NetworkDiagnostics = NetworkDiagnostics;
            static INIT: std::sync::Once = std::sync::Once::new();
            INIT.call_once(|| {
                log::set_logger(&LOGGER).unwrap();
                log::set_max_level(log::LevelFilter::Info);
                prns_groups::enable_metadata_diagnostics();
            });
        }
        // The actual host count stays four; all other members are deliberately
        // offline. Short deadlines are an explicit in-process fixture policy.
        let policy = SyncPolicy {
            min_exchange_millis: 200,
            max_exchange_millis: 2000,
            unknown_bitrate_bps: 1_000_000,
            unknown_rtt_millis: 50,
            fast_poll_millis: 100,
            constrained_poll_millis: 200,
            max_fast_idle_millis: 500,
            max_idle_millis: 2000,
            announce_interval_millis: 60_000,
            ..SyncPolicy::default()
        };
        assert!(policy.is_valid());
        let before_frames: usize = nodes
            .state
            .frames
            .iter()
            .map(|n| n.load(Ordering::SeqCst))
            .sum();
        let mut stops = Vec::new();
        let mut tasks = JoinSet::new();
        for (slot, index) in online.into_iter().enumerate() {
            let (stop, receiver) = watch::channel(false);
            stops.push(stop);
            let (_, announcements) = announce_control(None).unwrap();
            tasks.spawn(run_multi_scoped_with_announcements(
                clients[index].clone(),
                nodes.scopes[slot].clone(),
                prns_groups_reticulum::destination_for(identities[index]).unwrap(),
                prns_groups_reticulum::enrollment_destination_for(identities[index]).unwrap(),
                nodes.jobs[slot].take().unwrap(),
                nodes.enrollment_jobs[slot].take().unwrap(),
                transport,
                enrollment,
                policy,
                announcements,
                receiver,
            ));
        }
        let network = Instant::now();
        let mut outgoing = PublicationIntent::text(group, "selected late member to owner").unwrap();
        outgoing.targets = Targets::Explicit(vec![members[identities[0].as_bytes()]]);
        let from_late = network_stage(
            "late_publish",
            &mut tasks,
            &nodes,
            &clients,
            group,
            journey::publish(&clients[late], outgoing),
        )
        .await;
        network_stage(
            "owner_record",
            &mut tasks,
            &nodes,
            &clients,
            group,
            delivered(&clients[0], group, from_late, false),
        )
        .await;
        network_stage(
            "late_receipt",
            &mut tasks,
            &nodes,
            &clients,
            group,
            delivered(&clients[late], group, from_late, true),
        )
        .await;
        let mut outgoing = PublicationIntent::text(group, "owner to selected late member").unwrap();
        outgoing.targets = Targets::Explicit(vec![members[identities[late].as_bytes()]]);
        let from_owner = network_stage(
            "owner_publish",
            &mut tasks,
            &nodes,
            &clients,
            group,
            journey::publish(&clients[0], outgoing),
        )
        .await;
        network_stage(
            "late_record",
            &mut tasks,
            &nodes,
            &clients,
            group,
            delivered(&clients[late], group, from_owner, false),
        )
        .await;
        network_stage(
            "owner_receipt",
            &mut tasks,
            &nodes,
            &clients,
            group,
            delivered(&clients[0], group, from_owner, true),
        )
        .await;
        let network_ms = network.elapsed().as_millis();
        eprintln!(
            "member-stage-result {}",
            serde_json::json!({
                "stage": "two_way_text_and_receipts", "elapsed_ms": network_ms,
            })
        );
        for stop in stops {
            stop.send(true).unwrap();
        }
        while let Some(task) = tokio::time::timeout(Duration::from_secs(15), tasks.join_next())
            .await
            .unwrap()
        {
            task.unwrap().unwrap();
        }
        let after_frames: usize = nodes
            .state
            .frames
            .iter()
            .map(|n| n.load(Ordering::SeqCst))
            .sum();
        assert!(after_frames > before_frames);
        let resources = nodes.state.resources.load(Ordering::SeqCst);
        let proofs = nodes.state.resource_proofs.load(Ordering::SeqCst);
        assert!(resources > 0 && proofs > 0);
        Some(NetworkMeasurement {
            elapsed_ms: network_ms,
            link_frames: after_frames - before_frames,
            resources,
            resource_proofs: proofs,
            records: [from_late, from_owner],
        })
    } else {
        None
    };
    nodes.stop().await;

    let (lifecycle_ms, reopen_ms) = if with_lifecycle {
        let lifecycle = Instant::now();
        journey::execute(
            &clients[0],
            Intent::RemoveMember {
                group,
                member: members[identities[removed_index].as_bytes()],
            },
        )
        .await;
        relay
            .until(&clients, &routes, group, "remove", || {
                converged(&clients, group, member_count - 1, &removed_members, false)
            })
            .await;
        assert_eq!(
            clients[removed_index]
                .conversation(group, PAGE)
                .await
                .unwrap()
                .lifecycle,
            GroupLifecycle::Removed
        );
        journey::execute(&clients[late], Intent::Leave { group }).await;
        relay
            .until(&clients, &routes, group, "leave", || {
                converged(&clients, group, member_count - 2, &departed_members, false)
            })
            .await;
        assert_eq!(
            clients[late]
                .conversation(group, PAGE)
                .await
                .unwrap()
                .lifecycle,
            GroupLifecycle::Removed
        );
        assert_eq!(
            clients[0]
                .conversation(group, PAGE)
                .await
                .unwrap()
                .member_count,
            (member_count - 2) as u64
        );
        journey::execute(&clients[0], Intent::Close { group }).await;
        relay
            .until(&clients, &routes, group, "close", || {
                converged(&clients, group, member_count - 2, &departed_members, true)
            })
            .await;
        assert_eq!(
            clients[member_count - 2]
                .conversation(group, PAGE)
                .await
                .unwrap()
                .lifecycle,
            GroupLifecycle::Closed
        );
        let lifecycle_ms = lifecycle.elapsed().as_millis();
        eprintln!(
            "member-stage-result {}",
            serde_json::json!({
                "stage": "remove_leave_close", "elapsed_ms": lifecycle_ms,
            })
        );
        for owner in &owners {
            owner.close().await.unwrap();
        }
        drop(owners);
        let reopen = Instant::now();
        for index in online {
            let owner = LocalOwner::open_existing(
                &paths[index],
                identities[index],
                options(validation_cache_entries),
            )
            .unwrap();
            let client = owner.client();
            assert_eq!(
                client.conversation(group, PAGE).await.unwrap().lifecycle,
                GroupLifecycle::Closed
            );
            if let Some(network) = &network_result {
                if [0, late].contains(&index) {
                    let records = client
                        .records(group, None, 16, SystemClock.now())
                        .await
                        .unwrap();
                    for expected in network.records {
                        assert!(records
                            .records
                            .iter()
                            .any(|row| row.record.metadata.record == expected));
                    }
                }
            }
            journey::execute(&client, Intent::DeleteLocal { group }).await;
            let page = client.conversations(None, PAGE).await.unwrap();
            assert!(page.items.is_empty() && page.next.is_none());
            owner.close().await.unwrap();
        }
        let reopen_ms = reopen.elapsed().as_millis();
        (Some(lifecycle_ms), Some(reopen_ms))
    } else {
        for owner in &owners {
            owner.close().await.unwrap();
        }
        (None, None)
    };
    eprintln!(
        "member-scale-result {}",
        serde_json::json!({
            "members": member_count, "real_hosts": online.len(), "selected_cache_bytes": member_count * 1024 * 1024,
            "validation_cache_entries_per_owner": validation_cache_entries.map(NonZeroUsize::get),
            "joins": joins, "last_join": joins.last(), "idle_reads": idle,
            "admission_public_work": admission_relay,
            "startup_ms": startup_ms, "admission_ms": admission_ms, "inspection_ms": inspection_ms,
            "network": network_result.as_ref().map(|result| serde_json::json!({
                "elapsed_ms": result.elapsed_ms, "link_frames": result.link_frames,
                "resources": result.resources, "resource_proofs": result.resource_proofs,
            })),
            "lifecycle_ms": lifecycle_ms, "reopen_ms": reopen_ms,
            "elapsed_ms": total.elapsed().as_millis(), "direct_enrollment": relay,
            "database_bytes": disk_bytes(&paths), "profile": PROFILE,
            "scope": "host-only real admission and bounded queries; network/lifecycle/reopen fields are null unless explicitly selected; no phone/RF/background claim"
        })
    );
}
