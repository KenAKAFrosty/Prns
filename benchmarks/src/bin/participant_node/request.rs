use super::*;

pub(super) struct RequestServer {
    pub(super) served: Arc<AtomicU64>,
    pub(super) response_bytes: Arc<AtomicU64>,
    pub(super) scratch: Arc<Vec<u8>>,
}

pub(super) struct BenchSizedRequestRoute;

impl RequestRoute<RequestServer> for BenchSizedRequestRoute {
    const PATH: &'static str = REQUEST_PATH;
    const POLICY: RoutePolicy = RoutePolicy::AllowAll;
    async fn handle(mut cx: RequestContext<'_, RequestServer>) -> Result<(), Decline> {
        let wanted = msgpack_bin_payload(cx.data)
            .get(..2)
            .map(|len| u16::from_be_bytes([len[0], len[1]]) as usize)
            .unwrap_or(0)
            .min(cx.state.scratch.len());
        let mut framed = Vec::with_capacity(wanted + 3);
        begin_msgpack_bin(wanted, &mut framed);
        framed.extend_from_slice(&cx.state.scratch[..wanted]);
        cx.state.served.fetch_add(1, Ordering::Relaxed);
        cx.state
            .response_bytes
            .fetch_add(wanted as u64, Ordering::Relaxed);
        cx.respond(&framed)
    }
}

pub(super) async fn run_request_endpoint(
    manifest: &Manifest,
    role: &str,
    addr: &str,
    duration: Duration,
) {
    let aspect: &'static str = Box::leak(manifest.name.clone().into_boxed_str());
    let aspects: &'static [&'static str] = Box::leak(Box::new([aspect]));
    let announce_every = Duration::from_millis(manifest.profile.announce_every_ms);
    let initiators = manifest.profile.initiator_count;
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
    let destination = single
        .destination_hash()
        .expect("the bench destination name is valid");

    if role == "responder" {
        let served = Arc::new(AtomicU64::new(0));
        let response_bytes = Arc::new(AtomicU64::new(0));
        let app_state = RequestServer {
            served: Arc::clone(&served),
            response_bytes: Arc::clone(&response_bytes),
            scratch: Arc::new(incompressible_payload(512)),
        };
        let (event_tx, event_rx) = mpsc::unbounded_channel::<Event>();
        let on_event = move |event: PrnsEvent<'_>, _state: &RequestServer| {
            let mapped = match event {
                PrnsEvent::Diagnostic(Diagnostic::LinkEstablished(_)) => Some(Event::LinkUp),
                PrnsEvent::Diagnostic(Diagnostic::LinkClosed { .. }) => Some(Event::Closed),
                _ => None,
            };
            if let Some(event) = mapped {
                let _ = event_tx.send(event);
            }
        };
        let (node, bound) = build_responder_node(
            single,
            app_state,
            routes![BenchSizedRequestRoute],
            on_event,
            manifest,
            addr,
        )
        .await;
        let commands = node.handle();
        println!("READY role=responder addr={bound}");
        let firehose = respond_request_runtime(
            destination,
            announce_every,
            duration,
            initiators,
            &served,
            &response_bytes,
            &commands,
            event_rx,
        );
        tokio::select! {
            result = node.run() => unreachable!("the responder's run loop returned: {result:?}"),
            () = firehose => {}
        }
    } else if role == "initiator" {
        let (event_tx, event_rx) = mpsc::unbounded_channel::<Event>();
        let on_event = move |event: PrnsEvent<'_>, _state: &()| {
            let mapped = match event {
                PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard { destination, .. }) => {
                    Some(Event::Heard(destination))
                }
                PrnsEvent::Diagnostic(Diagnostic::CommandSettled { id, settlement }) => {
                    Some(Event::Settled(id, settlement))
                }
                PrnsEvent::Message(Message::Response { data, .. }) => {
                    Some(Event::Response(msgpack_bin_payload(data).len()))
                }
                _ => None,
            };
            if let Some(event) = mapped {
                let _ = event_tx.send(event);
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
    duration: Duration,
    initiator_count: usize,
    served: &AtomicU64,
    response_bytes: &AtomicU64,
    commands: &PrnsNodeHandle,
    mut events: mpsc::UnboundedReceiver<Event>,
) {
    let mut links_up = 0usize;
    let mut closed_links = 0usize;
    let mut announce = tokio::time::interval(announce_every);
    let mut announcing = true;
    let report_at = tokio::time::Instant::now() + duration + DRAIN_GRACE;
    loop {
        tokio::select! {
            _ = announce.tick(), if announcing => {
                if commands
                    .issue(EngineCommand::AnnounceNow(AnnounceNow {
                        destination,
                        target: AnnounceTarget::AllInterfaces,
                        app_data: AnnounceAppData::Registered,
                    }))
                    .is_none()
                {
                    return;
                }
            }
            _ = tokio::time::sleep_until(report_at) => {
                println!(
                    "RESULT served={} response_bytes={}",
                    served.load(Ordering::Relaxed),
                    response_bytes.load(Ordering::Relaxed)
                );
                return;
            }
            event = events.recv() => {
                match event {
                    Some(Event::LinkUp) => {
                        links_up += 1;
                        if links_up >= initiator_count {
                            announcing = false;
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

pub(super) async fn initiate_request_runtime(
    profile: &Profile,
    duration: Duration,
    commands: &PrnsNodeHandle,
    mut events: mpsc::UnboundedReceiver<Event>,
) {
    let destination = loop {
        match events.recv().await.expect("reactor alive") {
            Event::Heard(destination) => break destination,
            _ => {}
        }
    };
    let link_id = commands
        .establish_link(destination)
        .await
        .expect("link establishes");

    let scratch = incompressible_payload(profile.request_max.max(2));
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
    let started = tokio::time::Instant::now();
    let deadline = started + duration;
    let mut sent = 0u64;
    let mut delivered = 0u64;
    let mut delivered_after_reconnect = 0u64;
    let reconnect_after = Duration::from_millis(profile.reconnect_at_ms.saturating_add(1000));
    let mut timeouts = 0u64;
    let mut in_flight = 0usize;
    let mut request_bytes = 0u64;
    let mut response_bytes = 0u64;
    let mut rtts: Vec<f64> = Vec::new();
    let mut request_started = std::collections::HashMap::new();
    let mut framed = Vec::with_capacity(profile.request_max + 3);
    let mut send_one =
        |in_flight: &mut usize,
         sent: &mut u64,
         request_started: &mut std::collections::HashMap<u64, tokio::time::Instant>| {
            let request_len = request_sizes.next_len();
            let wanted = response_sizes.next_len() as u16;
            begin_msgpack_bin(request_len, &mut framed);
            framed.extend_from_slice(&wanted.to_be_bytes());
            framed.extend_from_slice(&scratch[..request_len - 2]);
            request_bytes += request_len as u64;
            let started = tokio::time::Instant::now();
            if let Some(command_id) = commands.issue(EngineCommand::SendRequest(SendRequest {
                link_id,
                path_hash,
                data: SendRequestData::from_slice(&framed).expect("request fits"),
                response_timeout: Default::default(),
            })) {
                request_started.insert(command_id.0, started);
                *sent += 1;
                *in_flight += 1;
            }
        };

    for _ in 0..profile.window {
        send_one(&mut in_flight, &mut sent, &mut request_started);
    }
    let drain_deadline = deadline + DRAIN_GRACE;
    let failure_streak_limit = failure_streak_limit(profile.window);
    let mut failure_streak = 0u64;
    let mut died = false;
    while in_flight > 0 {
        let event = tokio::time::timeout_at(drain_deadline, events.recv()).await;
        let Ok(Some(event)) = event else { break };
        match event {
            Event::Settled(command_id, Settlement::SendRequest(result)) => {
                in_flight -= 1;
                let wall_rtt_ms = request_started
                    .remove(&command_id.0)
                    .map(|started| started.elapsed().as_secs_f64() * 1_000.0);
                match result {
                    Ok(receipt) => {
                        failure_streak = 0;
                        delivered += 1;
                        if profile.reconnect_at_ms > 0 && started.elapsed() > reconnect_after {
                            delivered_after_reconnect += 1;
                        }
                        // Use the same wall-clock definition as compiled RNS. The engine's
                        // receipt exposes whole milliseconds, which turns fast loopback RTTs
                        // into a misleading literal zero before the renderer ever sees them.
                        rtts.push(wall_rtt_ms.unwrap_or(receipt.rtt.millis() as f64));
                    }
                    Err(_) => {
                        timeouts += 1;
                        failure_streak += 1;
                    }
                }
                if !died && failure_streak >= failure_streak_limit {
                    died = true;
                    eprintln!("DIED mechanism=request failure_streak={failure_streak}");
                }
                if !died && tokio::time::Instant::now() < deadline {
                    send_one(&mut in_flight, &mut sent, &mut request_started);
                }
            }
            Event::Response(bytes) => {
                response_bytes += bytes as u64;
            }
            _ => {}
        }
    }
    timeouts += in_flight as u64;
    let elapsed_ms = started.elapsed().as_millis() as u64;

    commands.close_link(link_id);
    let close_deadline = tokio::time::Instant::now() + DRAIN_GRACE;
    loop {
        match tokio::time::timeout_at(close_deadline, events.recv()).await {
            Ok(Some(Event::Settled(_, Settlement::CloseLink(_)))) | Ok(None) | Err(_) => break,
            Ok(Some(_)) => {}
        }
    }

    rtts.sort_by(f64::total_cmp);
    let seconds = (elapsed_ms as f64 / 1000.0).max(f64::EPSILON);
    let reconnect_field = if profile.reconnect_at_ms > 0 {
        format!(" delivered_after_reconnect={delivered_after_reconnect}")
    } else {
        String::new()
    };
    println!(
        "RESULT sent={sent} delivered={delivered} timeouts={timeouts} \
         request_bytes={request_bytes} response_bytes={response_bytes} \
         elapsed_ms={elapsed_ms} requests_per_sec={:.1} \
         rtt_p50_ms={:.3} rtt_p99_ms={:.3}{}{reconnect_field} build={BUILD_PROFILE}",
        delivered as f64 / seconds,
        percentile_f64(&rtts, 0.50),
        percentile_f64(&rtts, 0.99),
        died_marker(died),
    );
}
