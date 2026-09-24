use super::*;

pub(super) fn runtime(profile: &Profile, index: usize) -> &Runtime {
    &profile.runtimes[usize::from(index == TARGET)]
}
pub(super) fn retain_control(
    controls: &mut Vec<(usize, u64, ControlledCrypto)>,
    index: usize,
    generation: u64,
    crypto: &CryptoPoolConfig,
) {
    if let CryptoPoolConfig::Controlled(control) = crypto {
        controls.push((index, generation, control.clone()));
    }
}
pub(super) fn start(
    tasks: &mut EmbassyTasks<'_>,
    lab: &VirtualBleLab,
    index: usize,
    generation: u64,
    storage: Storage,
    messages: Messages,
    execution: (CryptoPoolConfig, ResourceLayout),
) -> node::Node {
    match execution.1 {
        ResourceLayout::Common => start_storage::<
            { super::super::storage::COMMON_WINDOW_BYTES },
            {
                personal_rns::routing::links::resources::max_part_count(
                    super::super::storage::COMMON_WINDOW_BYTES,
                )
            },
        >(
            tasks,
            lab,
            index,
            generation,
            storage,
            messages,
            execution.0,
        ),
        ResourceLayout::WorkerSized => start_storage::<
            { super::super::storage::LARGE_WINDOW_BYTES },
            {
                personal_rns::routing::links::resources::max_part_count(
                    super::super::storage::LARGE_WINDOW_BYTES,
                )
            },
        >(
            tasks,
            lab,
            index,
            generation,
            storage,
            messages,
            execution.0,
        ),
    }
}
fn start_storage<const WINDOW: usize, const PARTS: usize>(
    tasks: &mut EmbassyTasks<'_>,
    lab: &VirtualBleLab,
    index: usize,
    generation: u64,
    storage: Storage,
    messages: Messages,
    crypto: CryptoPoolConfig,
) -> node::Node {
    node::start_with_settings(
        tasks,
        lab,
        index,
        generation,
        storage,
        messages,
        NodeSettings::<_, _, 1, 4> {
            crypto,
            storage: super::super::storage::WorkStorage::<WINDOW, PARTS>,
            destinations: [crate::echo::destination_with_limit(
                (index + 1) as u8,
                ByteLimit::Maximum(256),
            )],
            endpoints: personal_rns::request_endpoints![traffic::Echo, traffic::ResourceReply],
        },
    )
}
pub(super) fn reach(lab: &VirtualBleLab, controller: usize, reachability: Reachability) {
    lab.set_reachability(
        BleAddress::new([(controller + 1) as u8; 6]),
        BleAddress::new([(TARGET + 1) as u8; 6]),
        reachability,
    )
    .expect("explicit star topology");
}
pub(super) fn converge(tasks: &mut EmbassyTasks<'_>, lab: &VirtualBleLab, nodes: &[node::Node; 3]) {
    let horizon =
        crate::tick(tasks.snapshot().tick.get() + crate::fixture::ADVERTISING_INTERVAL_MS * 3);
    for _ in 0..4096 {
        tasks.settle();
        if lab.active_connection_count() == 2
            && nodes.iter().enumerate().all(|(index, node)| {
                node.inspection
                    .snapshots()
                    .iter()
                    .filter(|snapshot| {
                        matches!(snapshot.membership, Membership::FleetMember { .. })
                            && snapshot.connection == ConnectionState::Connected
                    })
                    .count()
                    == if index == TARGET { 2 } else { 1 }
            })
        {
            return;
        }
        assert!(
            tasks.snapshot().tick < horizon,
            "three-node discovery horizon"
        );
        tasks
            .advance_to_next_wake(horizon)
            .expect("discovery clock");
    }
    panic!("three-node BLE convergence exceeded its work budget");
}
pub(super) async fn announce_app(handle: Handle) {
    let announce = AnnounceNow {
        destination: crate::echo::destination((TARGET + 1) as u8)
            .destination_hash()
            .expect("app destination"),
        target: AnnounceTarget::AllInterfaces,
        app_data: AnnounceAppData::Registered,
    };
    match handle {
        Handle::Tokio(handle) => handle.announce_now(announce).await,
        Handle::Embassy(handle) => handle.announce_now(announce).await,
    }
    .expect("explicit application discovery");
}
pub(super) async fn connect_app(handle: Handle) -> LinkId {
    let destination = crate::echo::destination((TARGET + 1) as u8)
        .destination_hash()
        .expect("app destination");
    match handle {
        Handle::Tokio(handle) => handle.establish_link(destination).await,
        Handle::Embassy(handle) => handle.establish_link(destination).await,
    }
    .expect("application link")
}
pub(super) fn pair(
    tasks: &mut EmbassyTasks<'_>,
    nodes: &[node::Node; 3],
    messages: &Messages,
    controller: usize,
) {
    messages.1.borrow_mut().clear();
    let target = nodes[TARGET].handle.clone();
    let opened = tasks.complete_ready(async move { target.open_pairing().await });
    let opened = pairing::observed_offer_for(&messages.1, controller, opened);
    let handle = nodes[controller].handle.clone();
    tasks.complete_with_budget(
        CompletionBudget {
            deadline: crate::tick(tasks.snapshot().tick.get() + 1000),
            polls_per_tick: NonZeroUsize::new(POLLS_PER_TICK).expect("pairing budget"),
        },
        async move { handle.initiate(opened).await },
    );
    let controller_confirmation = messages
        .1
        .borrow_mut()
        .iter()
        .position(|event| matches!(event, Observation::Controller(_)))
        .expect("controller confirmation");
    let Observation::Controller(controller_confirmation) =
        messages.1.borrow_mut().remove(controller_confirmation)
    else {
        unreachable!("controller confirmation")
    };
    let target_confirmation = messages
        .1
        .borrow_mut()
        .iter()
        .position(|event| matches!(event, Observation::Target(_)))
        .expect("target confirmation");
    let Observation::Target(target_confirmation) =
        messages.1.borrow_mut().remove(target_confirmation)
    else {
        unreachable!("target confirmation")
    };
    assert_eq!(
        controller_confirmation.confirmation(),
        target_confirmation.confirmation()
    );
    let target = nodes[TARGET].handle.clone();
    tasks.complete_ready(async move { target.approve_target(target_confirmation).await });
    let handle = nodes[controller].handle.clone();
    tasks.complete_ready(async move { handle.approve_controller(controller_confirmation).await });
    assert!(messages.1.borrow().contains(&Observation::TargetPersisted));
    assert!(messages
        .1
        .borrow()
        .contains(&Observation::ControllerPersisted));
    let target = nodes[TARGET].handle.clone();
    tasks.complete_ready(async move { target.close_pairing().await });
}
