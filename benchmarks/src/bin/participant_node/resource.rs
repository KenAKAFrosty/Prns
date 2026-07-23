use super::*;

pub(super) async fn run_resource_endpoint(
    manifest: &Manifest,
    role: &str,
    addr: &str,
    duration: Duration,
) {
    let aspect = manifest.name.as_str();
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

    let (event_tx, event_rx) = event_channel(&manifest.profile);
    let on_event = move |event: PrnsEvent<'_>, _state: &()| {
        let mapped = match event {
            PrnsEvent::Diagnostic(Diagnostic::AnnounceHeard { destination, .. }) => {
                Some(Event::Heard(destination))
            }
            PrnsEvent::Diagnostic(Diagnostic::LinkEstablished(_)) => Some(Event::LinkUp),
            PrnsEvent::Diagnostic(Diagnostic::LinkClosed { .. }) => Some(Event::Closed),
            PrnsEvent::Message(Message::Resource { link_id, data, .. }) => {
                Some(Event::ResourceIn {
                    link_id,
                    bytes: data.len(),
                })
            }
            PrnsEvent::Diagnostic(Diagnostic::ResourceAssembled {
                link_id,
                total_size,
                ..
            }) => Some(Event::ResourceIn {
                link_id,
                bytes: total_size as usize,
            }),
            PrnsEvent::Diagnostic(Diagnostic::ResourceFailed {
                link_id,
                hash,
                cause,
            }) => {
                eprintln!(
                    "RESOURCE_FAILURE kind=protocol role=responder link_id={:?} hash={:?} cause={cause:?}",
                    link_id.as_bytes(),
                    hash.as_bytes(),
                );
                None
            }
            PrnsEvent::Message(Message::Delivered(Delivery::Link(delivery))) => {
                parse_resource_ack(delivery.plaintext).map(Event::ResourceAck)
            }
            _ => None,
        };
        if let Some(event) = mapped {
            send_event(&event_tx, event);
        }
    };

    if role == "responder" {
        let (node, bound) =
            build_responder_node(single, (), routes![], on_event, manifest, addr).await;
        let commands = node.handle();
        println!("READY role=responder addr={bound}");
        let firehose = async {
            await_startup_go().await;
            let collection_target = collection_target_receiver();
            respond_resource_runtime(
                destination,
                announce_every,
                duration,
                drain_grace(&manifest.profile),
                initiators,
                &commands,
                event_rx,
                collection_target,
            )
            .await;
        };
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

pub(super) async fn respond_resource_runtime(
    destination: DestinationHash,
    announce_every: Duration,
    duration: Duration,
    drain: Duration,
    initiator_count: usize,
    commands: &PrnsNodeHandle,
    mut events: mpsc::Receiver<Event>,
    mut collection_target: tokio::sync::oneshot::Receiver<(u64, u64)>,
) {
    let mut links_up = 0usize;
    let mut measurement_ready = false;
    let mut announce = tokio::time::interval(announce_every);
    let mut announcing = true;
    let report_at = tokio::time::Instant::now() + duration + drain + DRAIN_GRACE;
    let mut received = 0u64;
    let mut payload_bytes = 0u64;
    let mut target = None;
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
            requested = &mut collection_target, if target.is_none() => {
                target = Some(requested.expect("runner supplied collection target"));
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
                    Some(Event::ResourceIn { link_id, bytes }) => {
                        received += 1;
                        payload_bytes += bytes as u64;
                        let ack = resource_ack_payload(received);
                        commands
                            .issue(EngineCommand::SendToLink(SendToLink {
                                link_id,
                                payload: SendToLinkPayload::from_slice(&ack)
                                    .expect("resource acknowledgement fits"),
                            }))
                            .expect("resource acknowledgement is accepted");
                    }
                    Some(Event::Closed) => {}
                    None => return,
                    Some(_) => {}
                }
            }
        }
        if target == Some((received, payload_bytes)) {
            println!("RESULT received={received} payload_bytes={payload_bytes}");
            return;
        }
    }
}

pub(super) struct CyclingSource<'a> {
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

pub(super) async fn initiate_resource_runtime(
    profile: &Profile,
    duration: Duration,
    commands: &PrnsNodeHandle,
    mut events: mpsc::Receiver<Event>,
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
    await_measurement_start().await;
    let started = tokio::time::Instant::now();
    let deadline = started + duration;
    let mut sent = 0u64;
    let mut settled = 0u64;
    let mut failures = 0u64;
    let mut protocol_failures = 0u64;
    let mut ack_timeouts = 0u64;
    let mut payload_bytes = 0u64;
    let mut transfer_ms: Vec<u64> = Vec::new();
    while tokio::time::Instant::now() < deadline {
        let len = sizes.next_len();
        sent += 1;
        let transfer_started = tokio::time::Instant::now();
        let settled_clean = match commands
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
                true
            }
            Err(error) => {
                failures += 1;
                protocol_failures += 1;
                eprintln!(
                    "RESOURCE_FAILURE kind=protocol role=initiator sequence={sent} error={error:?}"
                );
                false
            }
        };
        if !settled_clean {
            break;
        }
        let ack_deadline = tokio::time::Instant::now() + drain_grace(profile);
        let acknowledged = loop {
            match tokio::time::timeout_at(ack_deadline, events.recv()).await {
                Ok(Some(Event::ResourceAck(sequence))) if sequence == sent => break true,
                Ok(Some(_)) => {}
                Ok(None) | Err(_) => break false,
            }
        };
        if !acknowledged {
            failures += 1;
            ack_timeouts += 1;
            eprintln!(
                "RESOURCE_FAILURE kind=application-ack-timeout role=initiator sequence={sent} wait_ms={}",
                drain_grace(profile).as_millis(),
            );
            break;
        }
    }
    let elapsed_ms = started.elapsed().as_millis().max(1) as u64;
    println!("MEASURE_DONE");
    transfer_ms.sort_unstable();
    let seconds = (elapsed_ms as f64 / 1000.0).max(f64::EPSILON);
    println!(
        "RESULT sent={sent} settled={settled} failures={failures} \
         protocol_failures={protocol_failures} ack_timeouts={ack_timeouts} \
         payload_bytes={payload_bytes} elapsed_ms={elapsed_ms} \
         goodput_bytes_per_sec={:.0} goodput_mbits_per_sec={:.2} \
         transfer_p50_ms={:.0} transfer_p99_ms={:.0} build={BUILD_PROFILE}",
        payload_bytes as f64 / seconds,
        payload_bytes as f64 * 8.0 / seconds / 1_000_000.0,
        percentile(&transfer_ms, 0.50),
        percentile(&transfer_ms, 0.99),
    );
    tokio::task::spawn_blocking(await_collection_release)
        .await
        .expect("collection release task");
    commands.close_link(link_id);
}
