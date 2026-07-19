use super::*;

pub(super) async fn run_resource_endpoint(
    manifest: &Manifest,
    role: &str,
    addr: &str,
    duration: Duration,
) {
    let aspect: &'static str = Box::leak(manifest.name.clone().into_boxed_str());
    let aspects: &'static [&'static str] = Box::leak(Box::new([aspect]));
    let announce_every = Duration::from_millis(manifest.profile.announce_every_ms);
    let initiators = manifest.profile.initiator_count;
    let resource_strategy = if role == "responder" {
        responder_resource_strategy(&manifest.profile)
    } else {
        ResourceStrategy::AcceptNone
    };
    let single = PreConfiguredDestination::Single {
        app_name: "bench",
        aspects,
        identity: generate_identity_secret(),
        announce_app_data: b"",
        proof: ProofStrategy::ProveAll,
        link_requests: LinkRequestPolicy::AcceptAll,
        ratchet: RatchetPolicy::NoRatchets,
        resource_strategy,
        request_handlers: RequestHandlerRegistration::None,
    };
    let destination = single
        .destination_hash()
        .expect("the bench destination name is valid");

    let (event_tx, event_rx) = mpsc::unbounded_channel::<Event>();
    let on_event = move |event: PrnsEvent<'_>, _state: &()| {
        let mapped = match event {
            PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard { destination, .. }) => {
                Some(Event::Heard(destination))
            }
            PrnsEvent::Diagnostic(Diagnostic::LinkEstablished(_)) => Some(Event::LinkUp),
            PrnsEvent::Diagnostic(Diagnostic::LinkClosed { .. }) => Some(Event::Closed),
            PrnsEvent::Message(Message::Resource { data, .. }) => {
                Some(Event::ResourceIn(data.len()))
            }
            PrnsEvent::Diagnostic(Diagnostic::ResourceAssembled { total_size, .. }) => {
                Some(Event::ResourceIn(total_size as usize))
            }
            _ => None,
        };
        if let Some(event) = mapped {
            let _ = event_tx.send(event);
        }
    };

    if role == "responder" {
        let (node, bound) =
            build_responder_node(single, (), routes![], on_event, manifest, addr, None).await;
        let commands = node.handle();
        println!("READY role=responder addr={bound}");
        let firehose = respond_resource_runtime(
            destination,
            announce_every,
            duration,
            initiators,
            &commands,
            event_rx,
        );
        tokio::select! {
            result = node.run() => unreachable!("the responder's run loop returned: {result:?}"),
            () = firehose => {}
        }
    } else if role == "initiator" {
        let node = build_initiator_node(single, on_event, manifest, addr).await;
        let commands = node.handle();
        println!("READY role=initiator");
        let firehose = async {
            initiate_resource_runtime(&manifest.profile, duration, &commands, event_rx).await;
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

async fn respond_resource_runtime(
    destination: DestinationHash,
    announce_every: Duration,
    duration: Duration,
    initiator_count: usize,
    commands: &PrnsNodeHandle,
    mut events: mpsc::UnboundedReceiver<Event>,
) {
    let mut links_up = 0usize;
    let mut closed_links = 0usize;
    let mut announce = tokio::time::interval(announce_every);
    let mut announcing = true;
    let report_at = tokio::time::Instant::now() + duration + DRAIN_GRACE;
    let mut received = 0u64;
    let mut payload_bytes = 0u64;
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
                println!("RESULT received={received} payload_bytes={payload_bytes}");
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
                    Some(Event::ResourceIn(bytes)) => {
                        received += 1;
                        payload_bytes += bytes as u64;
                    }
                    Some(Event::Closed) if closed_links + 1 < initiator_count => {
                        closed_links += 1;
                    }
                    Some(Event::Closed) | None => {
                        println!("RESULT received={received} payload_bytes={payload_bytes}");
                        return;
                    }
                    Some(_) => {}
                }
            }
        }
    }
}

struct CyclingSource<'a> {
    block: &'a [u8],
    pos: usize,
    remaining: usize,
}

impl<'a> CyclingSource<'a> {
    fn new(block: &'a [u8], total_len: usize) -> Self {
        Self {
            block,
            pos: 0,
            remaining: total_len,
        }
    }
}

impl AsyncRead for CyclingSource<'_> {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let this = self.get_mut();
        while this.remaining > 0 && buf.remaining() > 0 {
            if this.pos == this.block.len() {
                this.pos = 0;
            }
            let take = this
                .remaining
                .min(buf.remaining())
                .min(this.block.len() - this.pos);
            buf.put_slice(&this.block[this.pos..this.pos + take]);
            this.pos += take;
            this.remaining -= take;
        }
        std::task::Poll::Ready(Ok(()))
    }
}

async fn initiate_resource_runtime(
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
    let block = scenario_payload(profile, MAX_EFFICIENT_SIZE);
    let compression = segment_compression(profile);
    let mut sizes = SizeSequence::new(
        profile.size_seed,
        profile.payload_min,
        profile.payload_max,
        profile.payload_len,
    );
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
                CyclingSource::new(&block, len),
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
    commands.close_link(link_id);
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
}
