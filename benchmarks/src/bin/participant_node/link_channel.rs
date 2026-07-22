use super::*;

pub(super) async fn run_runtime_endpoint(
    manifest: &Manifest,
    role: &str,
    addr: &str,
    duration: Duration,
) {
    let announce_every = Duration::from_millis(manifest.profile.announce_every_ms);
    let initiators = manifest.profile.initiator_count;

    // The recipe borrows its destination names for the node's whole life, and the node lives as
    // long as its `run` loop is driven, so the manifest-derived aspect is promoted to 'static.
    let aspect = manifest.name.as_str();
    let aspects: &'static [&'static str] = Box::leak(Box::new([aspect]));
    let identity_secret = generate_identity_secret();
    let single = PreConfiguredDestination::Single {
        app_name: "bench",
        aspects,
        identity: identity_secret,
        announce_app_data: b"",
        proof: ProofStrategy::ProveAll,
        link_requests: LinkRequestPolicy::AcceptAll,
        ratchet: RatchetPolicy::NoRatchets,
        resource_strategy: ResourceStrategy::AcceptNone,
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
            PrnsEvent::Diagnostic(Diagnostic::CommandSettled { id, settlement }) => {
                Some(Event::Settled(id, settlement))
            }
            PrnsEvent::Diagnostic(Diagnostic::LinkEstablished(_)) => Some(Event::LinkUp),
            PrnsEvent::Diagnostic(Diagnostic::LinkClosed { .. }) => Some(Event::Closed),
            PrnsEvent::Message(Message::Delivered(Delivery::Single(delivery))) => {
                Some(Event::Delivered(delivery.plaintext.len()))
            }
            PrnsEvent::Message(Message::Delivered(Delivery::Link(delivery))) => {
                Some(Event::Delivered(delivery.plaintext.len()))
            }
            _ => None,
        };
        if let Some(event) = mapped {
            let _ = event_tx.send(event);
        }
    };

    if role == "responder" {
        let (node, bound) =
            build_responder_node(single, (), routes![], on_event, manifest, addr).await;
        let commands = node.handle();
        println!("READY role=responder addr={bound}");
        let firehose = async {
            if manifest.name == ScenarioId::LinkMessageThroughput {
                respond_link(
                    destination,
                    announce_every,
                    duration,
                    drain_grace(&manifest.profile),
                    initiators,
                    &commands,
                    event_rx,
                )
                .await;
            } else {
                respond(destination, announce_every, duration, &commands, event_rx).await;
            }
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
            if manifest.name == ScenarioId::LinkMessageThroughput {
                initiate_link(&manifest.profile, duration, &commands, event_rx).await;
            } else {
                initiate(&manifest.profile, duration, &commands, event_rx).await;
            }
            // Close settlement is engine-state, not wire-state: give the egress lane a beat to
            // flush the close frame, or the responder only learns via its 10s stale reaper.
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

async fn respond(
    destination: DestinationHash,
    announce_every: Duration,
    duration: Duration,
    commands: &PrnsNodeHandle,
    mut events: mpsc::UnboundedReceiver<Event>,
) {
    let mut announce = tokio::time::interval(announce_every);
    let mut report_at = None;
    let mut delivered = 0u64;
    let mut payload_bytes = 0u64;
    loop {
        tokio::select! {
            _ = announce.tick(), if delivered == 0 => {
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
            _ = tokio::time::sleep_until(
                report_at.unwrap_or_else(|| tokio::time::Instant::now() + Duration::from_secs(86_400))
            ) => {
                println!("RESULT delivered={delivered} payload_bytes={payload_bytes}");
                return;
            }
            event = events.recv() => {
                match event {
                    Some(Event::Delivered(bytes)) => {
                        report_at.get_or_insert_with(|| tokio::time::Instant::now() + duration + DRAIN_GRACE);
                        delivered += 1;
                        payload_bytes += bytes as u64;
                    }
                    None => return,
                    Some(_) => {}
                }
            }
        }
    }
}

async fn initiate(
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

    let scratch = incompressible_payload(profile.payload_max.max(profile.payload_len));
    let mut sizes = SizeSequence::new(
        profile.size_seed,
        profile.payload_min,
        profile.payload_max,
        profile.payload_len,
    );
    await_measurement_start();
    let started = tokio::time::Instant::now();
    let deadline = started + duration;
    let mut sent = 0u64;
    let mut delivered = 0u64;
    let mut timeouts = 0u64;
    let mut in_flight = 0usize;
    let mut sent_sizes: std::collections::HashMap<u64, usize> = std::collections::HashMap::new();
    let mut delivered_bytes = 0u64;
    let mut rtts: Vec<u64> = Vec::new();
    let mut send_one =
        |in_flight: &mut usize,
         sent: &mut u64,
         sent_sizes: &mut std::collections::HashMap<u64, usize>| {
            let len = sizes.next_len();
            if let Some(id) = commands.issue(EngineCommand::SendSinglePacket(SendSinglePacket {
                destination,
                payload: SendSinglePacketPayload::from_slice(&scratch[..len])
                    .expect("payload fits"),
            })) {
                sent_sizes.insert(id.0, len);
                *sent += 1;
                *in_flight += 1;
            }
        };

    for _ in 0..profile.window {
        send_one(&mut in_flight, &mut sent, &mut sent_sizes);
    }
    let drain_deadline = deadline + DRAIN_GRACE;
    let failure_streak_limit = failure_streak_limit(profile.window);
    let mut failure_streak = 0u64;
    let mut died = false;
    while in_flight > 0 {
        let event = tokio::time::timeout_at(drain_deadline, events.recv()).await;
        let Ok(Some(event)) = event else { break };
        if let Event::Settled(id, Settlement::SendSinglePacket(result)) = event {
            in_flight -= 1;
            let size = sent_sizes.remove(&id.0).unwrap_or(0) as u64;
            match result {
                Ok(receipt) => {
                    failure_streak = 0;
                    delivered += 1;
                    delivered_bytes += size;
                    rtts.push(receipt.rtt.millis());
                }
                Err(_) => {
                    timeouts += 1;
                    failure_streak += 1;
                }
            }
            if !died && failure_streak >= failure_streak_limit {
                died = true;
                eprintln!("DIED mechanism=single failure_streak={failure_streak}");
            }
            if !died && tokio::time::Instant::now() < deadline {
                send_one(&mut in_flight, &mut sent, &mut sent_sizes);
            }
        }
    }
    // A bounded run must account for every issued receipt even if the drain deadline wins the
    // race with its final callback. These are expired proof waits, not silent omissions.
    timeouts += in_flight as u64;
    let elapsed_ms = started.elapsed().as_millis() as u64;
    println!("MEASURE_DONE");

    rtts.sort_unstable();
    let payload_bytes = delivered_bytes;
    let seconds = (elapsed_ms as f64 / 1000.0).max(f64::EPSILON);
    println!(
        "RESULT sent={sent} delivered={delivered} timeouts={timeouts} \
         payload_bytes={payload_bytes} elapsed_ms={elapsed_ms} \
         delivered_per_sec={:.1} goodput_bytes_per_sec={:.0} \
         rtt_p50_ms={:.0} rtt_p99_ms={:.0}{} build={BUILD_PROFILE}",
        delivered as f64 / seconds,
        payload_bytes as f64 / seconds,
        percentile(&rtts, 0.50),
        percentile(&rtts, 0.99),
        died_marker(died),
    );
}

async fn respond_link(
    destination: DestinationHash,
    announce_every: Duration,
    duration: Duration,
    drain: Duration,
    expected_links: usize,
    commands: &PrnsNodeHandle,
    mut events: mpsc::UnboundedReceiver<Event>,
) {
    let mut links_up = 0usize;
    let mut closed_links = 0usize;
    let mut announce = tokio::time::interval(announce_every);
    let mut announcing = true;
    let mut delivered = 0u64;
    let mut payload_bytes = 0u64;
    let report_at = tokio::time::Instant::now() + duration + drain + DRAIN_GRACE;
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
                println!("RESULT delivered={delivered} payload_bytes={payload_bytes}");
                return;
            }
            event = events.recv() => {
                match event {
                    Some(Event::LinkUp) => {
                        links_up += 1;
                        if links_up >= expected_links {
                            announcing = false;
                        }
                    }
                    Some(Event::Delivered(bytes)) => {
                        delivered += 1;
                        payload_bytes += bytes as u64;
                    }
                    Some(Event::Closed) if closed_links + 1 < expected_links => {
                        closed_links += 1;
                    }
                    Some(Event::Closed) | None => {
                        println!("RESULT delivered={delivered} payload_bytes={payload_bytes}");
                        return;
                    }
                    Some(_) => {}
                }
            }
        }
    }
}

async fn initiate_link(
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
    let establish = commands
        .issue(EngineCommand::EstablishLink(EstablishLink { destination }))
        .expect("reactor alive");
    let link_id = loop {
        match events.recv().await.expect("reactor alive") {
            Event::Settled(id, Settlement::EstablishLink(Ok(established))) if id == establish => {
                break established.link_id;
            }
            Event::Settled(id, Settlement::EstablishLink(Err(failure))) if id == establish => {
                panic!("link refused: {failure:?}");
            }
            _ => {}
        }
    };

    let scratch = incompressible_payload(profile.payload_max.max(profile.payload_len));
    let mut sizes = SizeSequence::new(
        profile.size_seed,
        profile.payload_min,
        profile.payload_max,
        profile.payload_len,
    );
    await_measurement_start();
    let started = tokio::time::Instant::now();
    let deadline = started + duration;
    let mut sent = 0u64;
    let mut delivered = 0u64;
    let mut timeouts = 0u64;
    let mut culled = 0u64;
    let mut in_flight = 0usize;
    let mut sent_sizes: std::collections::HashMap<u64, usize> = std::collections::HashMap::new();
    let mut sent_payload_bytes = 0u64;
    let mut rtts: Vec<u64> = Vec::new();
    let mut send_one = |in_flight: &mut usize,
                        sent: &mut u64,
                        sent_sizes: &mut std::collections::HashMap<u64, usize>,
                        sent_payload_bytes: &mut u64| {
        let len = sizes.next_len();
        if let Some(id) = commands.issue(EngineCommand::SendToLink(SendToLink {
            link_id,
            payload: SendToLinkPayload::from_slice(&scratch[..len]).expect("payload fits"),
        })) {
            sent_sizes.insert(id.0, len);
            *sent += 1;
            *sent_payload_bytes += len as u64;
            *in_flight += 1;
        }
    };

    for _ in 0..profile.window {
        send_one(
            &mut in_flight,
            &mut sent,
            &mut sent_sizes,
            &mut sent_payload_bytes,
        );
    }
    let drain_deadline = deadline + drain_grace(profile);
    while in_flight > 0 {
        let event = tokio::time::timeout_at(drain_deadline, events.recv()).await;
        let Ok(Some(event)) = event else { break };
        if let Event::Settled(id, Settlement::SendToLink(result)) = event {
            in_flight -= 1;
            let size = sent_sizes.remove(&id.0).unwrap_or(0) as u64;
            let replenish = match result {
                Ok(receipt) => {
                    delivered += 1;
                    rtts.push(receipt.rtt.millis());
                    true
                }
                Err(SendToLinkFailure::Culled) => {
                    // A local receipt-capacity cull is backpressure, not a wire timeout.
                    // Do not refill this slot: the window contracts until every outstanding
                    // send has a receipt the engine can actually track to settlement.
                    sent -= 1;
                    sent_payload_bytes = sent_payload_bytes.saturating_sub(size);
                    culled += 1;
                    false
                }
                Err(_) => {
                    timeouts += 1;
                    true
                }
            };
            if replenish && tokio::time::Instant::now() < deadline {
                send_one(
                    &mut in_flight,
                    &mut sent,
                    &mut sent_sizes,
                    &mut sent_payload_bytes,
                );
            }
        }
    }
    timeouts += in_flight as u64;
    let elapsed_ms = started.elapsed().as_millis() as u64;
    println!("MEASURE_DONE");

    assert!(commands.close_link(link_id), "reactor alive");
    let close_deadline = tokio::time::Instant::now() + drain_grace(profile);
    loop {
        match tokio::time::timeout_at(close_deadline, events.recv()).await {
            Ok(Some(Event::Settled(_, Settlement::CloseLink(_)))) | Ok(None) | Err(_) => break,
            Ok(Some(_)) => {}
        }
    }

    rtts.sort_unstable();
    let payload_bytes = sent_payload_bytes;
    let seconds = (elapsed_ms as f64 / 1000.0).max(f64::EPSILON);
    let attempted = sent + culled;
    println!(
        "RESULT attempted={attempted} sent={sent} delivered={sent} timeouts=0 \
         receipt_proved={delivered} receipt_unproved={timeouts} culled={culled} \
         payload_bytes={payload_bytes} elapsed_ms={elapsed_ms} \
         delivered_per_sec={:.1} goodput_bytes_per_sec={:.0} \
         rtt_p50_ms={:.0} rtt_p99_ms={:.0}{} build={BUILD_PROFILE}",
        sent as f64 / seconds,
        payload_bytes as f64 / seconds,
        percentile(&rtts, 0.50),
        percentile(&rtts, 0.99),
        died_marker(false),
    );
}
