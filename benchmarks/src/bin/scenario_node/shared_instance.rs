use super::*;

pub(super) fn shared_instance_port() -> Option<u16> {
    std::env::var("MATCHUP_SHARED_PORT")
        .ok()
        .and_then(|raw| raw.parse().ok())
}

pub(super) fn build_bus_client_node<F>(
    single: PreConfiguredDestination<'static>,
    on_event: F,
) -> PrnsNode<(), (), F, NodeStorage>
where
    F: FnMut(PrnsEvent<'_>, &()),
{
    PrnsNode::new(PrnsNodeRecipe {
        transport_identity: None,
        pre_configured_destinations: [single],
        app_state: (),
        storage: NodeStorage::default(),
        routes: routes![],
        on_event,
        interfaces: Manual,
    })
}

pub(super) async fn join_bus(commands: &PrnsNodeHandle, port: u16) {
    let credentials = SharedInstanceCredentials::from_identity_secret(
        &[0xA2; personal_rns::identity::IDENTITY_SECRET_KEY_LEN],
    );
    let role = join_shared_instance(
        commands,
        SharedInstanceIntent {
            blackhole_source: credentials.transport_identity_hash(),
            transport_identity: credentials.transport_identity_hash(),
            network_identity: None,
            credentials,
            blackhole_files: RnsBlackholeFiles::new(
                std::env::temp_dir().join(std::format!("prns-scenario-{port}-blackhole")),
            ),
            ports: InstancePorts {
                bus: port,
                control: port + 1,
            },
            transport: personal_rns::shared_instance::SharedInstanceTransport::Tcp,
            policy: personal_rns::interfaces::shared_instance::core::configured_policy(
                Default::default(),
            ),
            on_existing: OnExisting::JoinAsClient,
        },
    )
    .await
    .expect("join the shared-instance bus");
    assert!(
        matches!(role, Role::JoinedAsClient { .. }),
        "expected to join a running host as a client, got {role:?}"
    );
}

struct RequestServed(Arc<AtomicU64>);

struct BenchRequestRoute;

impl RequestRoute<RequestServed> for BenchRequestRoute {
    const PATH: &'static str = REQUEST_PATH;
    const POLICY: RoutePolicy = RoutePolicy::AllowAll;
    async fn handle(mut cx: RequestContext<'_, RequestServed>) -> Result<(), Decline> {
        cx.state.0.fetch_add(1, Ordering::Relaxed);
        cx.respond(b"pong")
    }
}

pub(super) async fn run_request_bus_client(
    manifest: &Manifest,
    role: &str,
    duration: Duration,
    port: u16,
) {
    let aspect: &'static str = Box::leak(manifest.name.clone().into_boxed_str());
    let aspects: &'static [&'static str] = Box::leak(Box::new([aspect]));
    let announce_every = Duration::from_millis(manifest.profile.announce_every_ms);
    let single = PreConfiguredDestination::Single {
        app_name: "bench",
        aspects,
        identity: generate_identity_secret(),
        announce_app_data: b"",
        proof: ProofStrategy::ProveAll,
        link_requests: LinkRequestPolicy::AcceptAll,
        ratchet: RatchetPolicy::NoRatchets,
        resource_strategy: ResourceStrategy::AcceptNone,
        request_handlers: if role == "responder" {
            RequestHandlerRegistration::NodeRouteSet
        } else {
            RequestHandlerRegistration::None
        },
    };
    if role == "responder" {
        let served = Arc::new(AtomicU64::new(0));
        let destination = single.destination_hash().expect("valid bench destination");
        let node = PrnsNode::new(PrnsNodeRecipe {
            transport_identity: None,
            pre_configured_destinations: [single],
            app_state: RequestServed(Arc::clone(&served)),
            storage: NodeStorage::default(),
            routes: routes![BenchRequestRoute],
            on_event: |_event, _state| {},
            interfaces: Manual,
        });
        let commands = node.handle();
        join_bus(&commands, port).await;
        println!("READY role=responder addr=shared");
        let announcer = commands.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(announce_every);
            loop {
                ticker.tick().await;
                if announcer
                    .issue(EngineCommand::AnnounceNow(AnnounceNow {
                        destination,
                        target: AnnounceTarget::AllInterfaces,
                        app_data: AnnounceAppData::Registered,
                    }))
                    .is_none()
                {
                    break;
                }
            }
        });
        let report = async {
            tokio::time::sleep(duration + DRAIN_GRACE).await;
            let served = served.load(Ordering::Relaxed);
            println!("RESULT served={served} response_bytes={}", served * 4);
        };
        tokio::select! {
            result = node.run() => unreachable!("the responder's run loop returned: {result:?}"),
            () = report => {}
        }
    } else {
        let (heard_tx, mut heard_rx) = mpsc::unbounded_channel::<DestinationHash>();
        let node = build_bus_client_node(single, move |event, _state| {
            if let PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard { destination, .. }) = event {
                let _ = heard_tx.send(destination);
            }
        });
        let commands = node.handle();
        join_bus(&commands, port).await;
        println!("READY role=initiator");
        let firehose = async {
            let destination = heard_rx.recv().await.expect("hears the responder");
            let link_id = commands
                .establish_link(destination)
                .await
                .expect("link establishes");
            let path_hash = RequestPathHash::of(REQUEST_PATH);
            let started = tokio::time::Instant::now();
            let deadline = started + duration;
            let mut sent = 0u64;
            let mut delivered = 0u64;
            let mut timeouts = 0u64;
            let mut rtts: Vec<u64> = Vec::new();
            while tokio::time::Instant::now() < deadline {
                sent += 1;
                match commands.request(link_id, path_hash, b"ping").await {
                    Ok((_response, rtt)) => {
                        delivered += 1;
                        rtts.push(rtt.millis());
                    }
                    Err(_) => timeouts += 1,
                }
            }
            let elapsed_ms = started.elapsed().as_millis().max(1) as u64;
            rtts.sort_unstable();
            let per_sec = sent * 1000 / elapsed_ms;
            println!(
                "RESULT sent={sent} delivered={delivered} timeouts={timeouts} \
                 elapsed_ms={elapsed_ms} requests_per_sec={per_sec} \
                 rtt_p50_ms={:.0} rtt_p99_ms={:.0} build={BUILD_PROFILE}",
                percentile(&rtts, 0.50),
                percentile(&rtts, 0.99),
            );
        };
        tokio::select! {
            result = node.run() => unreachable!("the initiator's run loop returned: {result:?}"),
            () = firehose => {}
        }
    }
}

pub(super) async fn run_churn_bus_client(
    manifest: &Manifest,
    role: &str,
    duration: Duration,
    port: u16,
) {
    let aspect: &'static str = Box::leak(manifest.name.clone().into_boxed_str());
    let aspects: &'static [&'static str] = Box::leak(Box::new([aspect]));
    let announce_every = Duration::from_millis(manifest.profile.announce_every_ms);
    let single = PreConfiguredDestination::Single {
        app_name: "bench",
        aspects,
        identity: generate_identity_secret(),
        announce_app_data: b"",
        proof: ProofStrategy::ProveAll,
        link_requests: LinkRequestPolicy::AcceptAll,
        ratchet: RatchetPolicy::NoRatchets,
        resource_strategy: ResourceStrategy::AcceptNone,
        request_handlers: RequestHandlerRegistration::None,
    };
    if role == "responder" {
        let links = Arc::new(AtomicU64::new(0));
        let destination = single.destination_hash().expect("valid bench destination");
        let links_seen = Arc::clone(&links);
        let node = build_bus_client_node(single, move |event, _state| {
            if let PrnsEvent::Diagnostic(Diagnostic::LinkEstablished(_)) = event {
                links_seen.fetch_add(1, Ordering::Relaxed);
            }
        });
        let commands = node.handle();
        join_bus(&commands, port).await;
        println!("READY role=responder addr=shared");
        let announcer = commands.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(announce_every);
            loop {
                ticker.tick().await;
                if announcer
                    .issue(EngineCommand::AnnounceNow(AnnounceNow {
                        destination,
                        target: AnnounceTarget::AllInterfaces,
                        app_data: AnnounceAppData::Registered,
                    }))
                    .is_none()
                {
                    break;
                }
            }
        });
        let report = async {
            tokio::time::sleep(duration + DRAIN_GRACE).await;
            println!("RESULT received={}", links.load(Ordering::Relaxed));
        };
        tokio::select! {
            result = node.run() => unreachable!("the responder's run loop returned: {result:?}"),
            () = report => {}
        }
    } else {
        let (heard_tx, mut heard_rx) = mpsc::unbounded_channel::<DestinationHash>();
        let node = build_bus_client_node(single, move |event, _state| {
            if let PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard { destination, .. }) = event {
                let _ = heard_tx.send(destination);
            }
        });
        let commands = node.handle();
        join_bus(&commands, port).await;
        println!("READY role=initiator");
        let firehose = async {
            let destination = heard_rx.recv().await.expect("hears the responder");
            let started = tokio::time::Instant::now();
            let deadline = started + duration;
            let mut cycles = 0u64;
            let mut failures = 0u64;
            let mut establish_ms: Vec<u64> = Vec::new();
            while tokio::time::Instant::now() < deadline {
                let cycle_started = tokio::time::Instant::now();
                match commands.establish_link(destination).await {
                    Ok(link_id) => {
                        establish_ms.push(cycle_started.elapsed().as_millis() as u64);
                        commands.close_link(link_id);
                        cycles += 1;
                    }
                    Err(_) => failures += 1,
                }
            }
            let elapsed_ms = started.elapsed().as_millis().max(1) as u64;
            establish_ms.sort_unstable();
            let attempts = cycles + failures;
            let per_sec = cycles * 1000 / elapsed_ms;
            println!(
                "RESULT sent={attempts} delivered={cycles} timeouts={failures} cycles={cycles} \
                 failures={failures} elapsed_ms={elapsed_ms} cycles_per_sec={per_sec} \
                 establish_p50_ms={:.0} establish_p99_ms={:.0} build={BUILD_PROFILE}",
                percentile(&establish_ms, 0.50),
                percentile(&establish_ms, 0.99),
            );
        };
        tokio::select! {
            result = node.run() => unreachable!("the initiator's run loop returned: {result:?}"),
            () = firehose => {}
        }
    }
}

pub(super) async fn run_resource_bus_client(
    manifest: &Manifest,
    role: &str,
    duration: Duration,
    port: u16,
) {
    let aspect: &'static str = Box::leak(manifest.name.clone().into_boxed_str());
    let aspects: &'static [&'static str] = Box::leak(Box::new([aspect]));
    let announce_every = Duration::from_millis(manifest.profile.announce_every_ms);
    let single = PreConfiguredDestination::Single {
        app_name: "bench",
        aspects,
        identity: generate_identity_secret(),
        announce_app_data: b"",
        proof: ProofStrategy::ProveAll,
        link_requests: LinkRequestPolicy::AcceptAll,
        ratchet: RatchetPolicy::NoRatchets,
        resource_strategy: ResourceStrategy::AcceptNone,
        request_handlers: RequestHandlerRegistration::None,
    };
    if role == "responder" {
        run_resource_responder(
            single,
            responder_resource_strategy(&manifest.profile),
            port,
            announce_every,
            duration,
        )
        .await;
    } else {
        let (heard_tx, mut heard_rx) = mpsc::unbounded_channel::<DestinationHash>();
        let node = build_bus_client_node(single, move |event, _state| {
            if let PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard { destination, .. }) = event {
                let _ = heard_tx.send(destination);
            }
        });
        let commands = node.handle();
        join_bus(&commands, port).await;
        println!("READY role=initiator");
        let firehose = async {
            let destination = heard_rx.recv().await.expect("hears the responder");
            let link_id = commands
                .establish_link(destination)
                .await
                .expect("link establishes");
            let scratch = scenario_payload(
                &manifest.profile,
                manifest
                    .profile
                    .payload_max
                    .max(manifest.profile.payload_len),
            );
            let mut sizes = SizeSequence::new(
                manifest.profile.size_seed,
                manifest.profile.payload_min,
                manifest.profile.payload_max,
                manifest.profile.payload_len,
            );
            let compression = segment_compression(&manifest.profile);
            let started = tokio::time::Instant::now();
            let deadline = started + duration;
            let mut sent = 0u64;
            let mut settled = 0u64;
            let mut failures = 0u64;
            let mut payload_bytes = 0u64;
            let mut transfer_ms: Vec<u64> = Vec::new();
            while tokio::time::Instant::now() < deadline {
                let len = sizes.next_len();
                sent += 1;
                let transfer_started = tokio::time::Instant::now();
                match commands
                    .send_resource_with_compression(
                        link_id,
                        len as u64,
                        &scratch[..len],
                        compression,
                    )
                    .await
                {
                    Ok(()) => {
                        settled += 1;
                        payload_bytes += len as u64;
                        transfer_ms.push(transfer_started.elapsed().as_millis() as u64);
                    }
                    Err(_) => failures += 1,
                }
            }
            let elapsed_ms = started.elapsed().as_millis().max(1) as u64;
            transfer_ms.sort_unstable();
            let seconds = (elapsed_ms as f64 / 1000.0).max(f64::EPSILON);
            println!(
                "RESULT sent={sent} settled={settled} failures={failures} \
                 payload_bytes={payload_bytes} elapsed_ms={elapsed_ms} \
                 goodput_bytes_per_sec={:.0} goodput_mbits_per_sec={:.2} \
                 transfer_p50_ms={:.0} transfer_p99_ms={:.0} build={BUILD_PROFILE}",
                payload_bytes as f64 / seconds,
                payload_bytes as f64 * 8.0 / seconds / 1_000_000.0,
                percentile(&transfer_ms, 0.50),
                percentile(&transfer_ms, 0.99),
            );
        };
        tokio::select! {
            result = node.run() => unreachable!("the initiator's run loop returned: {result:?}"),
            () = firehose => {}
        }
    }
}

async fn run_resource_responder(
    single: PreConfiguredDestination<'static>,
    resource_strategy: ResourceStrategy,
    port: u16,
    announce_every: Duration,
    duration: Duration,
) {
    let received = Arc::new(AtomicU64::new(0));
    let bytes = Arc::new(AtomicU64::new(0));
    let destination = single.destination_hash().expect("valid bench destination");
    let received_cb = Arc::clone(&received);
    let bytes_cb = Arc::clone(&bytes);
    let node = build_bus_client_node(single, move |event, _state| {
        if let PrnsEvent::Message(Message::Resource { data, .. }) = event {
            received_cb.fetch_add(1, Ordering::Relaxed);
            bytes_cb.fetch_add(data.len() as u64, Ordering::Relaxed);
        }
    });
    let commands = node.handle();
    join_bus(&commands, port).await;
    let report = async {
        commands
            .set_resource_strategy(destination, resource_strategy)
            .await;
        println!("READY role=responder addr=shared");
        let announcer = commands.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(announce_every);
            loop {
                ticker.tick().await;
                if announcer
                    .issue(EngineCommand::AnnounceNow(AnnounceNow {
                        destination,
                        target: AnnounceTarget::AllInterfaces,
                        app_data: AnnounceAppData::Registered,
                    }))
                    .is_none()
                {
                    break;
                }
            }
        });
        tokio::time::sleep(duration + DRAIN_GRACE).await;
        println!(
            "RESULT received={} payload_bytes={}",
            received.load(Ordering::Relaxed),
            bytes.load(Ordering::Relaxed)
        );
    };
    tokio::select! {
        result = node.run() => unreachable!("the responder's run loop returned: {result:?}"),
        () = report => {}
    }
}

pub(super) async fn run_resource_fanout_bus_client(
    manifest: &Manifest,
    role: &str,
    duration: Duration,
    port: u16,
) {
    let aspect: &'static str = Box::leak(manifest.name.clone().into_boxed_str());
    let aspects: &'static [&'static str] = Box::leak(Box::new([aspect]));
    let announce_every = Duration::from_millis(manifest.profile.announce_every_ms);
    let single = PreConfiguredDestination::Single {
        app_name: "bench",
        aspects,
        identity: generate_identity_secret(),
        announce_app_data: b"",
        proof: ProofStrategy::ProveAll,
        link_requests: LinkRequestPolicy::AcceptAll,
        ratchet: RatchetPolicy::NoRatchets,
        resource_strategy: ResourceStrategy::AcceptNone,
        request_handlers: RequestHandlerRegistration::None,
    };
    if role == "responder" {
        run_resource_responder(
            single,
            responder_resource_strategy(&manifest.profile),
            port,
            announce_every,
            duration,
        )
        .await;
        return;
    }

    let (heard_tx, mut heard_rx) = mpsc::unbounded_channel::<DestinationHash>();
    let node = build_bus_client_node(single, move |event, _state| {
        if let PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard { destination, .. }) = event {
            let _ = heard_tx.send(destination);
        }
    });
    let commands = node.handle();
    join_bus(&commands, port).await;
    println!("READY role=initiator");
    let link_count = manifest.profile.link_count;
    let payload_min = manifest.profile.payload_min;
    let payload_max = manifest
        .profile
        .payload_max
        .max(manifest.profile.payload_len);
    let payload_len = manifest.profile.payload_len;
    let size_seed = manifest.profile.size_seed;
    let compression = segment_compression(&manifest.profile);
    let firehose = async {
        let destination = heard_rx.recv().await.expect("hears the responder");
        let mut links = Vec::with_capacity(link_count);
        for _ in 0..link_count {
            links.push(
                commands
                    .establish_link(destination)
                    .await
                    .expect("link establishes"),
            );
        }
        let scratch: Arc<Vec<u8>> = Arc::new(scenario_payload(&manifest.profile, payload_max));
        let started = tokio::time::Instant::now();
        let deadline = started + duration;
        let mut set: tokio::task::JoinSet<(u64, u64, u64, u64)> = tokio::task::JoinSet::new();
        for (index, link_id) in links.iter().copied().enumerate() {
            let commands = commands.clone();
            let scratch = Arc::clone(&scratch);
            let mut sizes = SizeSequence::new(
                size_seed ^ index as u64,
                payload_min,
                payload_max,
                payload_len,
            );
            set.spawn(async move {
                let mut sent = 0u64;
                let mut settled = 0u64;
                let mut failures = 0u64;
                let mut bytes = 0u64;
                while tokio::time::Instant::now() < deadline {
                    let len = sizes.next_len();
                    sent += 1;
                    match commands
                        .send_resource_with_compression(
                            link_id,
                            len as u64,
                            &scratch[..len],
                            compression,
                        )
                        .await
                    {
                        Ok(()) => {
                            settled += 1;
                            bytes += len as u64;
                        }
                        Err(_) => failures += 1,
                    }
                }
                (sent, settled, failures, bytes)
            });
        }
        let mut total_sent = 0u64;
        let mut total_settled = 0u64;
        let mut total_failures = 0u64;
        let mut total_bytes = 0u64;
        let mut silent_links = 0usize;
        while let Some(joined) = set.join_next().await {
            let (sent, settled, failures, bytes) = joined.expect("a resource task panicked");
            total_sent += sent;
            total_settled += settled;
            total_failures += failures;
            total_bytes += bytes;
            if settled == 0 {
                silent_links += 1;
            }
        }
        let elapsed_ms = started.elapsed().as_millis().max(1) as u64;
        for link_id in links {
            commands.close_link(link_id);
        }
        assert!(
            silent_links == 0,
            "{silent_links} of {link_count} links completed no resource transfer",
        );
        let seconds = (elapsed_ms as f64 / 1000.0).max(f64::EPSILON);
        println!(
            "RESULT sent={total_sent} settled={total_settled} failures={total_failures} \
             links={link_count} payload_bytes={total_bytes} elapsed_ms={elapsed_ms} \
             goodput_bytes_per_sec={:.0} goodput_mbits_per_sec={:.2} build={BUILD_PROFILE}",
            total_bytes as f64 / seconds,
            total_bytes as f64 * 8.0 / seconds / 1_000_000.0,
        );
    };
    tokio::select! {
        result = node.run() => unreachable!("the initiator's run loop returned: {result:?}"),
        () = firehose => {}
    }
}
