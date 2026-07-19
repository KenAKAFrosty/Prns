use super::shared_instance::{build_bus_client_node, join_bus};
use super::*;

pub(super) async fn run_runtime_endpoint(
    manifest: &Manifest,
    role: &str,
    addr: &str,
    duration: Duration,
) {
    let mechanism = manifest.profile.mechanism.as_str();
    let announce_every = Duration::from_millis(manifest.profile.announce_every_ms);
    let initiators = manifest.profile.initiator_count;

    // The recipe borrows its destination names for the node's whole life, and the node lives as
    // long as its `run` loop is driven, so the manifest-derived aspect is promoted to 'static.
    let aspect: &'static str = Box::leak(manifest.name.clone().into_boxed_str());
    let aspects: &'static [&'static str] = Box::leak(Box::new([aspect]));
    let identity_secret = generate_identity_secret();
    let transport_identity =
        (manifest.profile.tunnel && role == "responder").then(|| identity_secret.clone());
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
            PrnsEvent::Message(Message::ChannelMessage { data, .. }) => {
                Some(Event::Delivered(data.len()))
            }
            _ => None,
        };
        if let Some(event) = mapped {
            let _ = event_tx.send(event);
        }
    };

    let shared_port = shared_instance_port();
    if role == "responder" {
        let (node, bound) = match shared_port {
            Some(_) => (
                build_bus_client_node(single, on_event),
                "shared".to_string(),
            ),
            None => {
                build_responder_node(
                    single,
                    (),
                    routes![],
                    on_event,
                    manifest,
                    addr,
                    transport_identity,
                )
                .await
            }
        };
        let commands = node.handle();
        if let Some(port) = shared_port {
            join_bus(&commands, port).await;
        }
        println!("READY role=responder addr={bound}");
        let expected_links = if mechanism == "links-breadth" {
            manifest.profile.link_count
        } else {
            initiators
        };
        let firehose = async {
            if matches!(mechanism, "link" | "channel" | "links-breadth") {
                respond_link(
                    destination,
                    announce_every,
                    expected_links,
                    &commands,
                    event_rx,
                )
                .await;
            } else {
                respond(destination, announce_every, duration, &commands, event_rx).await;
            }
        };
        tokio::select! {
            () = node.run() => unreachable!("the responder's run loop returned"),
            () = firehose => {}
        }
    } else if role == "initiator" {
        let node = match shared_port {
            Some(_) => build_bus_client_node(single, on_event),
            None => build_initiator_node(single, on_event, manifest, addr).await,
        };
        let commands = node.handle();
        if let Some(port) = shared_port {
            join_bus(&commands, port).await;
        }
        println!("READY role=initiator");
        let firehose = async {
            if mechanism == "link" {
                initiate_link(&manifest.profile, duration, &commands, event_rx).await;
            } else if mechanism == "links-breadth" {
                initiate_links_breadth(&manifest.profile, duration, &commands, event_rx).await;
            } else if mechanism == "link-storm" {
                initiate_link_storm(&manifest.profile, duration, &commands, event_rx).await;
            } else if mechanism == "channel" {
                initiate_channel(&manifest.profile, duration, &commands, event_rx).await;
            } else {
                initiate(&manifest.profile, duration, &commands, event_rx).await;
            }
            // Close settlement is engine-state, not wire-state: give the egress lane a beat to
            // flush the close frame, or the responder only learns via its 10s stale reaper.
            tokio::time::sleep(Duration::from_millis(200)).await;
        };
        tokio::select! {
            () = node.run() => unreachable!("the initiator's run loop returned"),
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
    let report_at = tokio::time::Instant::now() + duration + DRAIN_GRACE;
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
            _ = tokio::time::sleep_until(report_at) => {
                println!("RESULT delivered={delivered} payload_bytes={payload_bytes}");
                return;
            }
            event = events.recv() => {
                match event {
                    Some(Event::Delivered(bytes)) => {
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
    let elapsed_ms = started.elapsed().as_millis() as u64;

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
            if let Some(id) = commands.issue(EngineCommand::SendToLink(SendToLink {
                link_id,
                payload: SendToLinkPayload::from_slice(&scratch[..len]).expect("payload fits"),
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
        if let Event::Settled(id, Settlement::SendToLink(result)) = event {
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
                eprintln!("DIED mechanism=link failure_streak={failure_streak}");
            }
            if !died && tokio::time::Instant::now() < deadline {
                send_one(&mut in_flight, &mut sent, &mut sent_sizes);
            }
        }
    }
    let elapsed_ms = started.elapsed().as_millis() as u64;

    assert!(commands.close_link(link_id), "reactor alive");
    let close_deadline = tokio::time::Instant::now() + DRAIN_GRACE;
    loop {
        match tokio::time::timeout_at(close_deadline, events.recv()).await {
            Ok(Some(Event::Settled(_, Settlement::CloseLink(_)))) | Ok(None) | Err(_) => break,
            Ok(Some(_)) => {}
        }
    }

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

async fn initiate_links_breadth(
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

    let link_count = profile.link_count;
    for _ in 0..link_count {
        commands
            .issue(EngineCommand::EstablishLink(EstablishLink { destination }))
            .expect("reactor alive");
    }
    let mut links = Vec::with_capacity(link_count);
    let establish_deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    while links.len() < link_count {
        match tokio::time::timeout_at(establish_deadline, events.recv()).await {
            Ok(Some(Event::Settled(_, Settlement::EstablishLink(Ok(established))))) => {
                links.push(established.link_id);
            }
            Ok(Some(Event::Settled(_, Settlement::EstablishLink(Err(failure))))) => {
                panic!(
                    "link {} of {link_count} refused: {failure:?}",
                    links.len() + 1
                );
            }
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => panic!(
                "only {} of {link_count} links established before the deadline",
                links.len()
            ),
        }
    }

    let scratch = incompressible_payload(profile.payload_max.max(profile.payload_len));
    let mut sizes = SizeSequence::new(
        profile.size_seed,
        profile.payload_min,
        profile.payload_max,
        profile.payload_len,
    );
    let started = tokio::time::Instant::now();
    let deadline = started + duration;
    let mut sent = 0u64;
    let mut delivered = 0u64;
    let mut timeouts = 0u64;
    let mut delivered_bytes = 0u64;
    let mut in_flight = 0usize;
    let mut rtts: Vec<u64> = Vec::new();
    let mut per_link_delivered = vec![0u64; link_count];
    let mut id_to_link: std::collections::HashMap<u64, usize> = std::collections::HashMap::new();
    let mut id_to_size: std::collections::HashMap<u64, usize> = std::collections::HashMap::new();
    let mut send_on =
        |link_index: usize,
         in_flight: &mut usize,
         sent: &mut u64,
         id_to_link: &mut std::collections::HashMap<u64, usize>,
         id_to_size: &mut std::collections::HashMap<u64, usize>| {
            let len = sizes.next_len();
            if let Some(id) = commands.issue(EngineCommand::SendToLink(SendToLink {
                link_id: links[link_index],
                payload: SendToLinkPayload::from_slice(&scratch[..len]).expect("payload fits"),
            })) {
                id_to_link.insert(id.0, link_index);
                id_to_size.insert(id.0, len);
                *sent += 1;
                *in_flight += 1;
            }
        };

    for link_index in 0..link_count {
        send_on(
            link_index,
            &mut in_flight,
            &mut sent,
            &mut id_to_link,
            &mut id_to_size,
        );
    }

    let drain_deadline = deadline + DRAIN_GRACE;
    while in_flight > 0 {
        let event = tokio::time::timeout_at(drain_deadline, events.recv()).await;
        let Ok(Some(event)) = event else { break };
        if let Event::Settled(id, Settlement::SendToLink(result)) = event {
            in_flight -= 1;
            let link_index = id_to_link.remove(&id.0).expect("a tracked send");
            let size = id_to_size.remove(&id.0).unwrap_or(0) as u64;
            match result {
                Ok(receipt) => {
                    delivered += 1;
                    delivered_bytes += size;
                    per_link_delivered[link_index] += 1;
                    rtts.push(receipt.rtt.millis());
                }
                Err(_) => timeouts += 1,
            }
            if tokio::time::Instant::now() < deadline {
                send_on(
                    link_index,
                    &mut in_flight,
                    &mut sent,
                    &mut id_to_link,
                    &mut id_to_size,
                );
            }
        }
    }
    let elapsed_ms = started.elapsed().as_millis() as u64;

    for &link_id in &links {
        commands.close_link(link_id);
    }
    let mut closed = 0usize;
    let close_deadline = tokio::time::Instant::now() + DRAIN_GRACE;
    while closed < link_count {
        match tokio::time::timeout_at(close_deadline, events.recv()).await {
            Ok(Some(Event::Settled(_, Settlement::CloseLink(_)))) => closed += 1,
            Ok(Some(_)) => {}
            Ok(None) | Err(_) => break,
        }
    }

    let silent_links = per_link_delivered.iter().filter(|&&d| d == 0).count();
    assert!(
        silent_links == 0,
        "{silent_links} of {link_count} relayed links delivered nothing — a carried link was dropped",
    );

    rtts.sort_unstable();
    let payload_bytes = delivered_bytes;
    let seconds = (elapsed_ms as f64 / 1000.0).max(f64::EPSILON);
    println!(
        "RESULT sent={sent} delivered={delivered} timeouts={timeouts} \
         links_established={link_count} payload_bytes={payload_bytes} elapsed_ms={elapsed_ms} \
         delivered_per_sec={:.1} rtt_p50_ms={:.0} rtt_p99_ms={:.0} build={BUILD_PROFILE}",
        delivered as f64 / seconds,
        percentile(&rtts, 0.50),
        percentile(&rtts, 0.99),
    );
}

async fn initiate_link_storm(
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

    let window = profile.window.max(1);
    let started = tokio::time::Instant::now();
    let deadline = started + duration;
    let mut established = 0u64;
    let mut closed = 0u64;
    let mut failures = 0u64;
    let mut outstanding = 0usize;
    let mut establish_ms: Vec<u64> = Vec::new();
    let mut pending: std::collections::HashMap<u64, tokio::time::Instant> =
        std::collections::HashMap::new();

    let start_one =
        |outstanding: &mut usize,
         pending: &mut std::collections::HashMap<u64, tokio::time::Instant>| {
            if let Some(id) =
                commands.issue(EngineCommand::EstablishLink(EstablishLink { destination }))
            {
                pending.insert(id.0, tokio::time::Instant::now());
                *outstanding += 1;
            }
        };

    for _ in 0..window {
        start_one(&mut outstanding, &mut pending);
    }

    let drain_deadline = deadline + DRAIN_GRACE;
    while outstanding > 0 {
        let event = tokio::time::timeout_at(drain_deadline, events.recv()).await;
        let Ok(Some(event)) = event else { break };
        match event {
            Event::Settled(id, Settlement::EstablishLink(Ok(est))) => {
                established += 1;
                if let Some(at) = pending.remove(&id.0) {
                    establish_ms.push(at.elapsed().as_millis() as u64);
                }
                commands.close_link(est.link_id);
            }
            Event::Settled(id, Settlement::EstablishLink(Err(_))) => {
                failures += 1;
                pending.remove(&id.0);
                outstanding -= 1;
                if tokio::time::Instant::now() < deadline {
                    start_one(&mut outstanding, &mut pending);
                }
            }
            Event::Settled(_, Settlement::CloseLink(_)) => {
                closed += 1;
                outstanding -= 1;
                if tokio::time::Instant::now() < deadline {
                    start_one(&mut outstanding, &mut pending);
                }
            }
            _ => {}
        }
    }

    let elapsed_ms = started.elapsed().as_millis() as u64;
    establish_ms.sort_unstable();
    let seconds = (duration.as_millis() as f64 / 1000.0).max(f64::EPSILON);
    println!(
        "RESULT established={established} closed={closed} failures={failures} window={window} \
         elapsed_ms={elapsed_ms} establish_per_sec={:.1} establish_p50_ms={:.0} \
         establish_p99_ms={:.0} build={BUILD_PROFILE}",
        established as f64 / seconds,
        percentile(&establish_ms, 0.50),
        percentile(&establish_ms, 0.99),
    );
}

async fn initiate_channel(
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
    let started = tokio::time::Instant::now();
    let deadline = started + duration;
    let mut sent = 0u64;
    let mut delivered = 0u64;
    let mut timeouts = 0u64;
    let mut in_flight = 0usize;
    let mut sent_sizes: std::collections::HashMap<u64, usize> = std::collections::HashMap::new();
    let mut delivered_bytes = 0u64;
    let mut rtts: Vec<u64> = Vec::new();
    let mut emit_one =
        |in_flight: &mut usize, sent_sizes: &mut std::collections::HashMap<u64, usize>| {
            let len = sizes.next_len();
            if let Some(id) = commands.issue(EngineCommand::SendToChannel(SendToChannel {
                link_id,
                message_type: BENCH_CHANNEL_MSGTYPE,
                body: SendToChannelBody::from_slice(&scratch[..len]).expect("payload fits"),
            })) {
                sent_sizes.insert(id.0, len);
                *in_flight += 1;
            }
        };

    for _ in 0..profile.window {
        emit_one(&mut in_flight, &mut sent_sizes);
    }
    let drain_deadline = deadline + DRAIN_GRACE;
    let failure_streak_limit = failure_streak_limit(profile.window);
    let mut failure_streak = 0u64;
    let mut died = false;
    while in_flight > 0 {
        let event = tokio::time::timeout_at(drain_deadline, events.recv()).await;
        let Ok(Some(event)) = event else { break };
        if let Event::Settled(id, Settlement::SendToChannel(result)) = event {
            in_flight -= 1;
            let size = sent_sizes.remove(&id.0).unwrap_or(0) as u64;
            match result {
                Ok(receipt) => {
                    failure_streak = 0;
                    sent += 1;
                    delivered += 1;
                    delivered_bytes += size;
                    rtts.push(receipt.rtt.millis());
                }
                Err(SendToChannelFailure::WindowFull) => {}
                Err(_) => {
                    sent += 1;
                    timeouts += 1;
                    failure_streak += 1;
                }
            }
            if !died && failure_streak >= failure_streak_limit {
                died = true;
                eprintln!("DIED mechanism=channel failure_streak={failure_streak}");
            }
            if !died && tokio::time::Instant::now() < deadline {
                emit_one(&mut in_flight, &mut sent_sizes);
            }
        }
    }
    let elapsed_ms = started.elapsed().as_millis() as u64;

    assert!(commands.close_link(link_id), "reactor alive");
    let close_deadline = tokio::time::Instant::now() + DRAIN_GRACE;
    loop {
        match tokio::time::timeout_at(close_deadline, events.recv()).await {
            Ok(Some(Event::Settled(_, Settlement::CloseLink(_)))) | Ok(None) | Err(_) => break,
            Ok(Some(_)) => {}
        }
    }

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

pub(super) async fn initiate_single(
    profile: &Profile,
    duration: Duration,
    commands: mpsc::UnboundedSender<HostCommand>,
    mut events: mpsc::UnboundedReceiver<Event>,
) {
    let destination = loop {
        match events.recv().await.expect("reactor alive") {
            Event::Heard(destination) => break destination,
            _ => {}
        }
    };

    let scratch = incompressible_payload(profile.payload_max.max(profile.payload_len).max(1));
    let mut sizes = SizeSequence::new(
        profile.size_seed,
        profile.payload_min,
        profile.payload_max,
        profile.payload_len,
    );
    let started = tokio::time::Instant::now();
    let deadline = started + duration;
    let drain_until = deadline + DRAIN_GRACE;
    let reconnect_after_ms = profile.reconnect_at_ms.saturating_add(1000);
    // Time-paced, not window-on-settle: a burst loss at the reconnect would otherwise fill the
    // window with stuck sends and stall the firehose before it can prove post-repoint delivery. A
    // short local timeout reaps the lost sends so fresh ones keep flowing.
    const LOCAL_TIMEOUT_MS: u64 = 5_000;
    const OUTSTANDING_CAP: usize = 256;
    let mut send_tick = tokio::time::interval(Duration::from_millis(25));
    let mut sweep = tokio::time::interval(Duration::from_millis(200));
    let mut next_id = 1u64;
    let mut sent = 0u64;
    let mut delivered = 0u64;
    let mut delivered_after_reconnect = 0u64;
    let mut timeouts = 0u64;
    let mut rtts: Vec<u64> = Vec::new();
    let mut outstanding: std::collections::HashMap<u64, u64> = std::collections::HashMap::new();
    loop {
        tokio::select! {
            _ = send_tick.tick() => {
                if tokio::time::Instant::now() < deadline && outstanding.len() < OUTSTANDING_CAP {
                    let len = sizes.next_len();
                    let id = next_id;
                    next_id += 1;
                    let command = IssuedCommand {
                        id: CommandId(id),
                        command: EngineCommand::SendSinglePacket(SendSinglePacket {
                            destination,
                            payload: SendSinglePacketPayload::from_slice(&scratch[..len])
                                .expect("payload fits"),
                        }),
                    };
                    if commands.send(HostCommand::Engine(command)).is_ok() {
                        outstanding.insert(id, started.elapsed().as_millis() as u64);
                        sent += 1;
                    }
                }
            }
            _ = sweep.tick() => {
                let now_ms = started.elapsed().as_millis() as u64;
                outstanding.retain(|_, send_ms| {
                    if now_ms.saturating_sub(*send_ms) > LOCAL_TIMEOUT_MS {
                        timeouts += 1;
                        false
                    } else {
                        true
                    }
                });
            }
            _ = tokio::time::sleep_until(drain_until) => {
                break;
            }
            event = events.recv() => {
                match event {
                    Some(Event::Settled(CommandId(id), Settlement::SendSinglePacket(result))) => {
                        if let Some(send_ms) = outstanding.remove(&id) {
                            match result {
                                Ok(receipt) => {
                                    delivered += 1;
                                    if profile.reconnect_at_ms > 0 && send_ms > reconnect_after_ms {
                                        delivered_after_reconnect += 1;
                                    }
                                    rtts.push(receipt.rtt.millis());
                                }
                                Err(_) => timeouts += 1,
                            }
                        }
                    }
                    None => break,
                    Some(_) => {}
                }
            }
        }
    }
    timeouts += outstanding.len() as u64;
    let elapsed_ms = started.elapsed().as_millis() as u64;

    rtts.sort_unstable();
    let seconds = (elapsed_ms as f64 / 1000.0).max(f64::EPSILON);
    let reconnect_field = if profile.reconnect_at_ms > 0 {
        format!(" delivered_after_reconnect={delivered_after_reconnect}")
    } else {
        String::new()
    };
    println!(
        "RESULT sent={sent} delivered={delivered} timeouts={timeouts} \
         elapsed_ms={elapsed_ms} delivered_per_sec={:.1} \
         rtt_p50_ms={:.0} rtt_p99_ms={:.0}{reconnect_field} build={BUILD_PROFILE}",
        delivered as f64 / seconds,
        percentile(&rtts, 0.50),
        percentile(&rtts, 0.99),
    );
}
