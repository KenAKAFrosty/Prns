use super::*;

pub(super) struct RequestServer {
    pub(super) served: Arc<AtomicU64>,
    pub(super) response_bytes: Arc<AtomicU64>,
    pub(super) scratch: Arc<Vec<u8>>,
}

pub(super) struct BenchSizedRequestEndpoint;

impl RequestEndpoint<RequestServer> for BenchSizedRequestEndpoint {
    const ENDPOINT_ID: &'static str = REQUEST_PATH;
    const POLICY: RequestEndpointPolicy = RequestEndpointPolicy::AllowAll;
    async fn handle(
        mut cx: RequestContext<'_, RequestServer>,
        _node: &impl personal_rns::runtime::PrnsNodeApi,
    ) -> Result<(), Decline> {
        let request = msgpack_bin_payload(cx.data);
        let wanted = request
            .get(..2)
            .map(|len| u16::from_be_bytes([len[0], len[1]]) as usize)
            .unwrap_or(0)
            .min(cx.state.scratch.len());
        let mut framed = Vec::with_capacity(wanted + 3);
        begin_msgpack_bin(wanted, &mut framed);
        framed.extend_from_slice(&cx.state.scratch[..wanted]);
        if request.get(2..6) != Some(b"WARM") {
            cx.state.served.fetch_add(1, Ordering::Relaxed);
            cx.state
                .response_bytes
                .fetch_add(wanted as u64, Ordering::Relaxed);
        }
        cx.respond(framed)
    }
}

pub(super) async fn run_request_endpoint(
    manifest: &Manifest,
    role: &str,
    addr: &str,
    duration: Duration,
) {
    let aspect = manifest.name.as_str();
    let aspects: &'static [&'static str] = Box::leak(Box::new([aspect]));
    let announce_every = Duration::from_millis(manifest.profile.announce_every_ms);
    let request_links = manifest.profile.request_link_count();
    let single = PreConfiguredDestination::Single {
        app_name: "bench",
        aspects,
        identity: generate_identity_secret(),
        announce_app_data: b"",
        proof: ProofStrategy::ProveAll,
        link_requests: LinkRequestPolicy::AcceptAll,
        ratchet: RatchetPolicy::NoRatchets,
        resource_strategy: ResourceStrategy::AcceptNone,
        maximum_request_bytes: Default::default(),
        request_endpoints: if role == "responder" {
            ServeMyRequestEndpoints::Yes
        } else {
            ServeMyRequestEndpoints::No
        },
    };
    let destination = single
        .destination_hash()
        .expect("the bench destination name is valid");

    if role == "responder" {
        let served = Arc::new(AtomicU64::new(0));
        let response_bytes = Arc::new(AtomicU64::new(0));
        let app_state = RequestServer {
            served: Arc::clone(&served),
            response_bytes: Arc::clone(&response_bytes),
            scratch: Arc::new(scenario_payload(
                &manifest.profile,
                manifest.profile.response_max,
            )),
        };
        let (event_tx, event_rx) = event_channel(&manifest.profile);
        let on_event = move |event: PrnsEvent<'_>, _state: &RequestServer| {
            let mapped = match event {
                PrnsEvent::Diagnostic(Diagnostic::LinkEstablished(_)) => Some(Event::LinkUp),
                PrnsEvent::Diagnostic(Diagnostic::LinkClosed { .. }) => Some(Event::Closed),
                _ => None,
            };
            if let Some(event) = mapped {
                send_event(&event_tx, event);
            }
        };
        let (node, bound) = build_responder_node(
            single,
            app_state,
            request_endpoints![BenchSizedRequestEndpoint],
            on_event,
            manifest,
            addr,
        )
        .await;
        let commands = node.handle();
        println!("READY role=responder addr={bound}");
        let firehose = async {
            await_startup_go().await;
            respond_request_runtime(
                destination,
                announce_every,
                request_links,
                &served,
                &response_bytes,
                &commands,
                event_rx,
            )
            .await;
        };
        tokio::select! {
            result = node.run() => unreachable!("the responder's run loop returned: {result:?}"),
            () = firehose => {}
        }
    } else if role == "initiator" {
        let (event_tx, event_rx) = event_channel(&manifest.profile);
        let on_event = move |event: PrnsEvent<'_>, _state: &()| {
            let mapped = match event {
                PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard { destination, .. }) => {
                    Some(Event::Heard(destination))
                }
                PrnsEvent::Diagnostic(Diagnostic::CommandSettled { id, settlement }) => {
                    Some(Event::Settled(id, settlement))
                }
                _ => None,
            };
            if let Some(event) = mapped {
                send_event(&event_tx, event);
            }
        };
        let node = build_initiator_node(single, on_event, manifest, addr).await;
        let commands = node.handle();
        println!("READY role=initiator");
        let firehose = async {
            initiate_request_runtime(&manifest.profile, duration, &commands, event_rx).await;
            tokio::time::sleep(Duration::from_millis(200)).await;
        };
        tokio::select! {
            result = node.run() => unreachable!("the initiator's run loop returned: {result:?}"),
            () = firehose => {}
        }
    } else {
        panic!("unknown role {role:?}");
    }
}

pub(super) async fn respond_request_runtime(
    destination: DestinationHash,
    announce_every: Duration,
    initiator_count: usize,
    served: &AtomicU64,
    response_bytes: &AtomicU64,
    commands: &PrnsNodeHandle,
    mut events: mpsc::Receiver<Event>,
) {
    let mut links_up = 0usize;
    let mut measurement_ready = false;
    let mut closed_links = 0usize;
    let mut announce = tokio::time::interval(announce_every);
    let mut announcing = true;
    loop {
        tokio::select! {
            _ = announce.tick(), if announcing => {
                if commands
                    .issue(PrnsCommand::AnnounceNow(AnnounceNow {
                        destination,
                        target: AnnounceTarget::AllInterfaces,
                        app_data: AnnounceAppData::Registered,
                    }))
                    .is_none()
                {
                    return;
                }
            }
            event = events.recv() => {
                match event {
                    Some(Event::LinkUp) => {
                        links_up += 1;
                        if links_up >= initiator_count && !measurement_ready {
                            announcing = false;
                            measurement_ready = true;
                            println!("MEASURE_READY");
                        }
                    }
                    Some(Event::Closed) if closed_links + 1 < initiator_count => {
                        closed_links += 1;
                    }
                    Some(Event::Closed) | None => {
                        println!(
                            "RESULT served={} response_bytes={}",
                            served.load(Ordering::Relaxed),
                            response_bytes.load(Ordering::Relaxed)
                        );
                        return;
                    }
                    Some(_) => {}
                }
            }
        }
    }
}

/// Small requests stay on the packet path while 1–4 KiB responses take the documented resource
/// path. That asynchronous response path gives each implementation time to register its public
/// request receipt before loopback completion. Four pre-established links provide four honest
/// concurrent lanes without sleeps, blocking handlers, private queue access, or reference patches.
pub(super) async fn initiate_request_runtime(
    profile: &Profile,
    duration: Duration,
    commands: &PrnsNodeHandle,
    mut events: mpsc::Receiver<Event>,
) {
    let destination = loop {
        if let Some(Event::Heard(destination)) = events.recv().await {
            break destination;
        }
    };
    let request_links = profile.request_link_count();
    let mut links = Vec::with_capacity(request_links);
    for _ in 0..request_links {
        links.push(
            commands
                .establish_link(destination)
                .await
                .expect("request link establishes"),
        );
    }

    let scratch = incompressible_payload(profile.request_max.max(2));
    for (index, &link_id) in links.iter().enumerate() {
        let mut warm = Vec::with_capacity(profile.request_min + 3);
        begin_msgpack_bin(profile.request_min, &mut warm);
        warm.extend_from_slice(&(profile.response_min as u16).to_be_bytes());
        warm.extend_from_slice(b"WARM");
        warm.extend_from_slice(&scratch[..profile.request_min - 6]);
        let mut armed = false;
        for attempt in 1..=3 {
            let result = commands
                .request_with_response_timeout(
                    link_id,
                    RequestPathHash::of(REQUEST_PATH),
                    &warm,
                    RequestResponseTimeout::Exact(DurationMillis(5_000)),
                )
                .await;
            armed = result.is_ok_and(|(response, _)| {
                msgpack_bin_payload(&response).len() == profile.response_min
            });
            println!(
                "STARTUP_ATTEMPT stage=request-link-arm link={} attempt={attempt} result={}",
                index + 1,
                if armed { "pass" } else { "fail" }
            );
            if armed {
                break;
            }
        }
        assert!(armed, "request link {} did not arm", index + 1);
    }
    let mut request_sizes = SizeSequence::new(
        profile.size_seed,
        profile.request_min.max(2),
        profile.request_max,
        profile.request_min.max(2),
    );
    let mut response_sizes = SizeSequence::new(
        profile.size_seed ^ 0xA5A5_A5A5_A5A5_A5A5,
        profile.response_min,
        profile.response_max,
        profile.response_min,
    );
    let path_hash = RequestPathHash::of(REQUEST_PATH);
    await_measurement_start().await;
    let started = tokio::time::Instant::now();
    let deadline = started + duration;
    let timeout = RequestResponseTimeout::Exact(DurationMillis(profile.drain_timeout_ms));
    let mut tasks = tokio::task::JoinSet::new();
    let mut available_links = links
        .iter()
        .copied()
        .collect::<std::collections::VecDeque<_>>();
    let mut sent = 0u64;
    let mut delivered = 0u64;
    let mut timeouts = 0u64;
    let mut request_bytes = 0u64;
    let mut response_bytes = 0u64;
    let mut expected_response_bytes = 0u64;
    let mut rtts = Vec::new();

    let mut launch = |link_id, tasks: &mut tokio::task::JoinSet<_>| {
        let request_len = request_sizes.next_len();
        let wanted = response_sizes.next_len() as u16;
        let mut framed = Vec::with_capacity(request_len + 3);
        begin_msgpack_bin(request_len, &mut framed);
        framed.extend_from_slice(&wanted.to_be_bytes());
        framed.extend_from_slice(&scratch[..request_len - 2]);
        let issued_at = tokio::time::Instant::now();
        let handle = commands.clone();
        tasks.spawn(async move {
            let result = handle
                .request_with_response_timeout(link_id, path_hash, &framed, timeout)
                .await;
            (link_id, request_len, u64::from(wanted), issued_at, result)
        });
        sent += 1;
        request_bytes += request_len as u64;
        expected_response_bytes += u64::from(wanted);
    };
    for _ in 0..profile.window {
        launch(
            available_links
                .pop_front()
                .expect("one link per request lane"),
            &mut tasks,
        );
    }

    let drain_deadline = deadline + drain_grace(profile);
    while !tasks.is_empty() {
        let Ok(Some(joined)) = tokio::time::timeout_at(drain_deadline, tasks.join_next()).await
        else {
            break;
        };
        let (link_id, _request_len, _wanted, issued_at, result) =
            joined.expect("request task remains alive");
        match result {
            Ok((response, _protocol_rtt)) => {
                delivered += 1;
                response_bytes += msgpack_bin_payload(&response).len() as u64;
                rtts.push(issued_at.elapsed().as_secs_f64() * 1_000.0);
            }
            Err(_) => timeouts += 1,
        }
        available_links.push_back(link_id);
        if tokio::time::Instant::now() < deadline {
            launch(
                available_links
                    .pop_front()
                    .expect("a settled lane returns one link"),
                &mut tasks,
            );
        }
    }
    timeouts += tasks.len() as u64;
    tasks.abort_all();
    let elapsed_ms = started.elapsed().as_millis().max(1) as u64;
    println!("MEASURE_DONE");

    for link_id in &links {
        commands.close_link(*link_id);
    }
    rtts.sort_by(f64::total_cmp);
    let seconds = (elapsed_ms as f64 / 1_000.0).max(f64::EPSILON);
    println!(
        "RESULT sent={sent} delivered={delivered} timeouts={timeouts} \
         request_bytes={request_bytes} response_bytes={response_bytes} \
         expected_response_bytes={expected_response_bytes} \
         elapsed_ms={elapsed_ms} requests_per_sec={:.1} \
         rtt_p50_ms={:.3} rtt_p99_ms={:.3} request_window={} request_links={} build={BUILD_PROFILE}",
        delivered as f64 / seconds,
        percentile_f64(&rtts, 0.50),
        percentile_f64(&rtts, 0.99),
        profile.window,
        links.len(),
    );
}
