use super::*;

pub fn with_triple<T>(
    profile: Profile,
    seed: u64,
    scenario: impl FnOnce(&mut Triple<'_, '_>) -> T,
) -> (T, Trace) {
    with_triple_layout(profile, seed, ResourceLayout::Common, scenario)
}
pub fn with_triple_layout<T>(
    profile: Profile,
    seed: u64,
    layout: ResourceLayout,
    scenario: impl FnOnce(&mut Triple<'_, '_>) -> T,
) -> (T, Trace) {
    let clock = ClockLease::acquire();
    let capture = BleWireCapture::new(NonZeroUsize::new(WIRE_CAPACITY).expect("wire budget"));
    let lab = VirtualBleLab::with_wire_capture(
        BleMediumConfig::new(
            TopologyConfig::Explicit {
                max_neighbors: NonZeroUsize::new(2).expect("two neighbors"),
            },
            3,
            32,
            32,
            MEDIUM_TRACE_CAPACITY,
        )
        .expect("bounded medium"),
        capture.clone(),
    );
    let mut driver =
        ManualTimeDriver::new(ManualMedium::Ble(lab.clone()), Duration::from_millis(1))
            .expect("shared clock");
    let mut tasks = EmbassyTasks::with_limits(
        &mut driver,
        clock,
        NonZeroUsize::new(ACTORS).expect("actors"),
        NonZeroUsize::new(POLLS_PER_TICK).expect("polls"),
        ManualTaskScheduling::Seeded {
            seed: SimulationSeed::new(seed),
        },
    );
    let storage = std::array::from_fn(|index| Storage::new(runtime(&profile, index).clone()));
    let messages = Messages(
        Rc::new(RefCell::new(Vec::new())),
        Rc::new(RefCell::new(Vec::new())),
    );
    let mut controls = Vec::new();
    let nodes = std::array::from_fn(|index| {
        let crypto = profile.crypto(runtime(&profile, index));
        retain_control(&mut controls, index, 0, &crypto);
        start(
            &mut tasks,
            &lab,
            index,
            0,
            storage[index].clone(),
            messages.clone(),
            (crypto, layout),
        )
    });
    for controller in [PRIMARY, HEALTHY] {
        reach(&lab, controller, Reachability::Reachable);
    }
    converge(&mut tasks, &lab, &nodes);
    for controller in [PRIMARY, HEALTHY] {
        pair(&mut tasks, &nodes, &messages, controller);
    }
    let target = nodes[TARGET].handle.clone();
    tasks.complete_ready(async move {
        target.announce().await;
        announce_app(target).await;
    });
    wait_app_routes(&mut tasks, &nodes, [0; 2]);
    let links = [PRIMARY, HEALTHY].map(|index| {
        let handle = nodes[index].handle.clone();
        tasks.complete_ready(async move { handle.connect().await })
    });
    let app_links = [PRIMARY, HEALTHY].map(|index| {
        let handle = nodes[index].handle.clone();
        tasks.complete_ready(async move { connect_app(handle).await })
    });
    assert_ne!(
        links[0], app_links[0],
        "distinct control and application links"
    );
    assert_ne!(
        links[1], app_links[1],
        "distinct control and application links"
    );
    let mut triple = Triple {
        tasks: &mut tasks,
        lab: &lab,
        nodes,
        storage,
        profile,
        layout,
        generations: [0; 3],
        controls,
        messages,
        links,
        app_links,
        retired_events: Vec::new(),
    };
    triple.advance_for(150);
    let result = scenario(&mut triple);
    let persistence = triple.storage.each_ref().map(Storage::trace);
    let events = triple.events();
    for node in triple.nodes {
        triple.tasks.cancel(node.task);
    }
    let crypto = triple
        .controls
        .iter()
        .map(|(index, generation, control)| {
            (
                *index,
                *generation,
                control.trace().expect("owned worker trace"),
            )
        })
        .collect();
    let retained_static = crate::static_storage::footprint();
    drop(tasks);
    assert_eq!(lab.active_connection_count(), 0);
    let wire = capture.snapshot();
    assert_eq!(wire.discarded_values, 0);
    assert_eq!(lab.trace().discarded_events, 0);
    let mut connections = Vec::new();
    let wire: Vec<_> = wire.values.into_iter().map(|value| {
        let connection = match connections.iter().position(|id| *id == value.connection) { Some(index) => index, None => { connections.push(value.connection); connections.len() - 1 } };
        serde_json::json!({ "connection": connection, "from": value.from.octets(), "to": value.to.octets(), "channel": format!("{:?}", value.channel), "bytes": value.bytes })
    }).collect();
    (
        result,
        Trace {
            wire: serde_json::Value::Array(wire),
            persistence,
            events,
            crypto,
            retained_static,
        },
    )
}

pub(super) fn route_counts(nodes: &[node::Node; 3]) -> [usize; 2] {
    let destination = crate::echo::destination((TARGET + 1) as u8)
        .destination_hash()
        .expect("app destination");
    [PRIMARY, HEALTHY].map(|index| {
        nodes[index]
            .events
            .snapshot()
            .iter()
            .filter(|event| **event == crate::node_events::Event::Announce { destination })
            .count()
    })
}
pub(super) fn wait_app_routes(
    tasks: &mut EmbassyTasks<'_>,
    nodes: &[node::Node; 3],
    baseline: [usize; 2],
) {
    let horizon = crate::tick(tasks.snapshot().tick.get() + OPERATION_BUDGET_MS);
    loop {
        tasks.settle();
        if route_counts(nodes)
            .into_iter()
            .zip(baseline)
            .all(|(count, before)| count > before)
        {
            return;
        }
        assert!(
            tasks.snapshot().tick < horizon,
            "application route convergence: {:?}",
            nodes.each_ref().map(|node| node.events.snapshot())
        );
        tasks
            .advance_to_next_wake(horizon)
            .expect("route convergence clock");
    }
}
