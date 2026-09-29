use super::*;

#[test]
fn real_nodes_survive_a_day_of_deadline_driven_idle_then_exchange_and_shutdown() {
    const DAY_MILLIS: u64 = 86_400_000;
    const MAX_STEPS: usize = 100_000;
    let medium = VirtualMedium::new(
        VirtualMediumConfig::new(
            TopologyConfig::Explicit {
                max_neighbors: nonzero(1),
            },
            2,
            16,
            32,
            4096,
            FaultPlan::none(),
        )
        .unwrap_or_else(|e| unreachable!("{e}")),
    );
    let mut driver = ManualTimeDriver::new(
        ManualMedium::Frames(medium.clone()),
        Duration::from_millis(1),
    )
    .unwrap_or_else(|e| unreachable!("{e}"));
    let mut runner = ManualTaskRunner::new(&mut driver, nonzero(3));
    let mut nodes = Vec::new();
    let mut endpoints = Vec::new();
    for index in 0..2 {
        let interface = medium
            .attach(&[index as u8])
            .unwrap_or_else(|e| unreachable!("{e}"));
        endpoints.push(interface.endpoint_id());
        let (task, mut ready) = add_node(
            &mut runner,
            NodeSpec {
                index,
                role: NodeRole::Endpoint,
                attach_interfaces: move |handle: &personal_rns::runtime::PrnsNodeHandle| {
                    let _attached = handle.add_interface(interface);
                },
                heard: Rc::new(RefCell::new(Vec::new())),
                heard_capacity: nonzero(1),
            },
        );
        assert!(settle(&mut runner).is_empty());
        nodes.push((
            task,
            ready.try_recv().unwrap_or_else(|e| unreachable!("{e}")),
        ));
    }
    assert_eq!(
        medium.set_reachability(endpoints[0], endpoints[1], Reachability::Reachable),
        Ok(TopologyMutation::Applied)
    );
    let mut steps = 0;
    while medium.now() < tick(DAY_MILLIS) {
        assert!(steps < MAX_STEPS, "bounded long-duration work");
        runner
            .advance_to_next_wake(tick(DAY_MILLIS))
            .unwrap_or_else(|e| unreachable!("{e}"));
        assert!(settle(&mut runner).is_empty());
        steps += 1;
    }
    let remote = destination(1)
        .destination_hash()
        .unwrap_or_else(|e| unreachable!("{e:?}"));
    announce(&nodes[1].1, remote);
    assert!(settle(&mut runner).is_empty());
    let handle = nodes[0].1.handle.clone();
    let linked = runner
        .insert(async move {
            let link = handle
                .establish_link(remote)
                .await
                .unwrap_or_else(|e| unreachable!("{e:?}"));
            Completion::Linked { node: 0, link }
        })
        .unwrap_or_else(|e| unreachable!("{e}"));
    let completed = settle(&mut runner);
    let [(task, Completion::Linked { node: 0, link })] = completed.as_slice() else {
        unreachable!("link settles after a day")
    };
    assert_eq!(*task, linked);
    let bytes = vec![42; 256];
    let request_task = request(
        &mut runner,
        0,
        nodes[0].1.handle.clone(),
        *link,
        bytes.clone(),
    );
    assert_eq!(
        settle(&mut runner),
        vec![(request_task, Completion::Response { node: 0, bytes })]
    );
    assert_eq!(
        medium.set_reachability(endpoints[0], endpoints[1], Reachability::Isolated),
        Ok(TopologyMutation::Applied)
    );
    let handle = nodes[0].1.handle.clone();
    let link = *link;
    let timeout = runner
        .insert(async move {
            assert_eq!(
                handle
                    .request_with_response_timeout(
                        link,
                        RequestPathHash::of(QUERY_PATH),
                        b"lost after a day",
                        RequestResponseTimeout::Exact(DurationMillis(50)),
                    )
                    .await,
                Err(SendError::Failed(SendRequestFailure::Timeout))
            );
            Completion::TimedOut { node: 0 }
        })
        .unwrap_or_else(|e| unreachable!("{e}"));
    assert!(settle(&mut runner).is_empty());
    runner
        .advance_to_next_wake(tick(2 * DAY_MILLIS))
        .unwrap_or_else(|e| unreachable!("{e}"));
    assert_eq!(medium.now(), tick(DAY_MILLIS + 50));
    assert_eq!(
        settle(&mut runner),
        vec![(timeout, Completion::TimedOut { node: 0 })]
    );
    let expected: BTreeMap<_, _> = nodes
        .into_iter()
        .enumerate()
        .map(|(node, (task, control))| {
            assert_eq!(control.shutdown.send(()), Ok(()));
            (
                task,
                Completion::Stopped {
                    node,
                    result: Ok(()),
                },
            )
        })
        .collect();
    assert_eq!(
        settle(&mut runner).into_iter().collect::<BTreeMap<_, _>>(),
        expected
    );
    assert_eq!(runner.task_count(), 0);
    assert_eq!(medium.trace().discarded_events, 0);
    println!("simulated_hours=24 coordinated_steps={steps}");
}
