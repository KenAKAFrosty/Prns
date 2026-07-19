use super::*;

pub(super) async fn run_churn_endpoint(
    manifest: &Manifest,
    role: &str,
    addr: &str,
    duration: Duration,
) {
    let aspect: &'static str = Box::leak(manifest.name.clone().into_boxed_str());
    let aspects: &'static [&'static str] = Box::leak(Box::new([aspect]));
    let announce_every = Duration::from_millis(manifest.profile.announce_every_ms);
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
            PrnsEvent::Diagnostic(Diagnostic::CommandSettled { id, settlement }) => {
                Some(Event::Settled(id, settlement))
            }
            PrnsEvent::Diagnostic(Diagnostic::LinkEstablished(_)) => Some(Event::LinkUp),
            PrnsEvent::Message(Message::Delivered(Delivery::Single(delivery))) => {
                Some(Event::Delivered(delivery.plaintext.len()))
            }
            PrnsEvent::Message(Message::Delivered(Delivery::Link(delivery))) => {
                Some(Event::Delivered(delivery.plaintext.len()))
            }
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
        let firehose =
            respond_churn_runtime(destination, announce_every, duration, &commands, event_rx);
        tokio::select! {
            () = node.run() => unreachable!("the responder's run loop returned"),
            () = firehose => {}
        }
    } else if role == "initiator" {
        let node = build_initiator_node(single, on_event, manifest, addr).await;
        let commands = node.handle();
        println!("READY role=initiator");
        let firehose = async {
            initiate_churn_runtime(&manifest.profile, duration, &commands, event_rx).await;
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

async fn respond_churn_runtime(
    destination: DestinationHash,
    announce_every: Duration,
    duration: Duration,
    commands: &PrnsNodeHandle,
    mut events: mpsc::UnboundedReceiver<Event>,
) {
    let mut announce = tokio::time::interval(announce_every);
    let mut announcing = true;
    let mut idle = tokio::time::interval(Duration::from_millis(200));
    let report_at = tokio::time::Instant::now() + duration + DRAIN_GRACE;
    let mut received = 0u64;
    let mut payload_bytes = 0u64;
    let mut last_delivery: Option<tokio::time::Instant> = None;
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
            _ = idle.tick() => {
                if last_delivery.is_some_and(|at| at.elapsed() > QUIET_AFTER_TRAFFIC) {
                    println!("RESULT received={received} payload_bytes={payload_bytes}");
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
                        announcing = false;
                    }
                    Some(Event::Delivered(bytes)) | Some(Event::ResourceIn(bytes)) => {
                        received += 1;
                        payload_bytes += bytes as u64;
                        last_delivery = Some(tokio::time::Instant::now());
                    }
                    None => return,
                    Some(_) => {}
                }
            }
        }
    }
}

async fn initiate_churn_runtime(
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

    let scratch = scenario_payload(profile, profile.file_max.max(profile.page_max));
    let compression = segment_compression(profile);
    let mut sizes = SizeSequence::new(profile.size_seed, 0, 0, 1);
    let started = tokio::time::Instant::now();
    let deadline = started + duration;
    let mut cycles = 0u64;
    let mut failures = 0u64;
    let mut commands_moved = 0u64;
    let mut pages_moved = 0u64;
    let mut files_moved = 0u64;
    let mut payload_bytes = 0u64;
    let mut establish_ms: Vec<u64> = Vec::new();
    let mut cycle_ms: Vec<u64> = Vec::new();
    let mut close_ms: Vec<u64> = Vec::new();
    let mut transfer_ms_by_band: [Vec<u64>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    let mut failure_streak = 0u64;
    let mut died = false;

    'churn: while tokio::time::Instant::now() < deadline {
        let cycle_started = tokio::time::Instant::now();
        let Some(establish_id) =
            commands.issue(EngineCommand::EstablishLink(EstablishLink { destination }))
        else {
            break;
        };
        let link_id = loop {
            match events.recv().await {
                Some(Event::Settled(id, Settlement::EstablishLink(result)))
                    if id == establish_id =>
                {
                    match result {
                        Ok(established) => break established.link_id,
                        Err(_) => {
                            failures += 1;
                            failure_streak += 1;
                            if failure_streak >= CHURN_FAILURE_STREAK_LIMIT {
                                died = true;
                                eprintln!("DIED mechanism=churn failure_streak={failure_streak}");
                                break 'churn;
                            }
                            continue 'churn;
                        }
                    }
                }
                Some(_) => {}
                None => break 'churn,
            }
        };
        establish_ms.push(cycle_started.elapsed().as_millis() as u64);

        let (band, len) = roll_band(&mut sizes, profile);
        let transfer_started = tokio::time::Instant::now();
        let moved = match band {
            Band::Command => {
                let Some(transfer_id) = commands.issue(EngineCommand::SendToLink(SendToLink {
                    link_id,
                    payload: SendToLinkPayload::from_slice(&scratch[..len]).expect("command fits"),
                })) else {
                    break;
                };
                loop {
                    match events.recv().await {
                        Some(Event::Settled(id, Settlement::SendToLink(result)))
                            if id == transfer_id =>
                        {
                            break result.is_ok();
                        }
                        Some(_) => {}
                        None => break 'churn,
                    }
                }
            }
            Band::Page | Band::File => commands
                .send_resource_with_compression(link_id, len as u64, &scratch[..len], compression)
                .await
                .is_ok(),
        };
        let transfer_elapsed = transfer_started.elapsed().as_millis() as u64;
        if moved {
            failure_streak = 0;
            payload_bytes += len as u64;
            let band_index = match band {
                Band::Command => {
                    commands_moved += 1;
                    0
                }
                Band::Page => {
                    pages_moved += 1;
                    1
                }
                Band::File => {
                    files_moved += 1;
                    2
                }
            };
            transfer_ms_by_band[band_index].push(transfer_elapsed);
        } else {
            failures += 1;
            failure_streak += 1;
        }

        let close_started = tokio::time::Instant::now();
        commands.close_link(link_id);
        loop {
            match events.recv().await {
                Some(Event::Settled(_, Settlement::CloseLink(_))) => break,
                Some(_) => {}
                None => break 'churn,
            }
        }
        close_ms.push(close_started.elapsed().as_millis() as u64);
        if moved {
            cycles += 1;
            cycle_ms.push(cycle_started.elapsed().as_millis() as u64);
        }
        if !died && failure_streak >= CHURN_FAILURE_STREAK_LIMIT {
            died = true;
            eprintln!("DIED mechanism=churn failure_streak={failure_streak}");
            break;
        }
    }
    let elapsed_ms = started.elapsed().as_millis() as u64;

    establish_ms.sort_unstable();
    cycle_ms.sort_unstable();
    let seconds = (elapsed_ms as f64 / 1000.0).max(f64::EPSILON);
    println!(
        "RESULT cycles={cycles} failures={failures} commands={commands_moved} \
         pages={pages_moved} files={files_moved} payload_bytes={payload_bytes} \
         elapsed_ms={elapsed_ms} cycles_per_sec={:.1} \
         establish_p50_ms={:.0} establish_p99_ms={:.0} \
         cycle_p50_ms={:.0} cycle_p99_ms={:.0}{}",
        cycles as f64 / seconds,
        percentile(&establish_ms, 0.50),
        percentile(&establish_ms, 0.99),
        percentile(&cycle_ms, 0.50),
        percentile(&cycle_ms, 0.99),
        died_marker(died),
    );

    let [mut command_ms, mut page_ms, mut file_ms] = transfer_ms_by_band;
    let establish_line = phase_line("establish", &mut establish_ms);
    let close_line = phase_line("close", &mut close_ms);
    let command_line = phase_line("transfer_command", &mut command_ms);
    let page_line = phase_line("transfer_page", &mut page_ms);
    let file_line = phase_line("transfer_file", &mut file_ms);
    eprintln!(
        "PHASES {establish_line} | {close_line} | {command_line} | {page_line} | {file_line}"
    );
}
