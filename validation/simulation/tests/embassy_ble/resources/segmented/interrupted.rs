use super::*;

fn interrupt(
    tasks: &mut EmbassyTasks<'_>,
    lab: &VirtualBleLab,
    handle: &impl PrnsNodeApi,
    trace: ResponseTrace,
    link: LinkId,
) -> (CommandId, Settlement) {
    assert!(trace.is_empty());
    let command = handle
        .issue(PrnsCommand::SendRequest(SendRequest {
            link_id: link,
            path_hash: RequestPathHash::of(files::FILE_PATH),
            data: SendRequestData::from_slice(&(TRANSFER_BYTES as u16).to_be_bytes()).unwrap(),
            response_timeout: RequestResponseTimeout::LinkDefault,
            maximum_response_bytes: ByteLimit::Unlimited,
        }))
        .unwrap();
    let medium = lab.clone();
    let terminal = tasks.complete_with_budget(
        CompletionBudget {
            deadline: tick(tasks.snapshot().tick.get() + INTERRUPTION_BUDGET_MS),
            polls_per_tick: NonZeroUsize::new(TRANSFER_POLLS_PER_TICK).unwrap(),
        },
        async move {
            let first = trace.next().await;
            let ResponseEvent::Segment { request, bytes, .. } = &first else {
                unreachable!("the first observed event must be a response segment")
            };
            assert!(!bytes.is_empty() && bytes.len() <= TRANSFER_WINDOW_BYTES);
            assert_eq!(
                first,
                ResponseEvent::Segment {
                    link,
                    request: *request,
                    index: 1,
                    total: 3,
                    bytes: files::FILE_BYTES[..bytes.len()].to_vec(),
                }
            );
            assert!(
                trace.is_empty(),
                "interrupt before the second segment arrives"
            );
            assert_eq!(medium.active_connection_count(), 1);
            assert_eq!(
                medium.set_reachability(EMBASSY_RADIO, TOKIO_RADIO, Reachability::Isolated),
                Ok(TopologyMutation::Applied)
            );
            assert_eq!(medium.active_connection_count(), 0);
            trace.completed().await
        },
    );
    let result = Err(SendRequestFailure::Timeout);
    assert_eq!(terminal, [ResponseEvent::Settled { command, result }]);
    assert_eq!(lab.active_connection_count(), 0);
    (command, Settlement::SendRequest(result))
}

fn recover_after_each_direction(
    tasks: &mut EmbassyTasks<'_>,
    lab: &VirtualBleLab,
    embedded: &ResourceNode,
    desktop: &tokio_node::TokioNode,
    mut links: [LinkId; 2],
) {
    for requester in [Requester::Tokio, Requester::Embassy] {
        match requester {
            Requester::Tokio => {
                interrupt(
                    tasks,
                    lab,
                    &desktop.handle,
                    desktop.responses.clone(),
                    links[1],
                );
                assert!(response_settlements(embedded).is_empty());
            }
            Requester::Embassy => {
                let expected = interrupt(
                    tasks,
                    lab,
                    &embedded.handle,
                    embedded.responses.clone(),
                    links[0],
                );
                assert_eq!(embedded.take_settled(), [expected]);
            }
        }
        links = reconnect(tasks, lab, embedded, desktop, links);
        match requester {
            Requester::Tokio => assert_eq!(
                response_settlements(embedded),
                [Settlement::Respond(Err(RespondFailure::Resource(
                    SendResourceFailure::LinkClosed
                )))]
            ),
            Requester::Embassy => assert!(response_settlements(embedded).is_empty()),
        }
        reassemble(tasks, lab, embedded, desktop, links);
    }
}

#[test]
fn esp32_and_apple_nodes_settle_interrupted_segmented_responses_and_recover() {
    scenario(
        Endpoint::Esp32(Esp32Host::Esp32),
        Endpoint::CoreBluetooth(AppleHost::MacOs),
        INTERRUPTION_TRACE_CAPACITY,
        recover_after_each_direction,
    );
}

#[test]
fn nrf52_and_bluez_nodes_settle_interrupted_segmented_responses_and_recover() {
    scenario(
        Endpoint::Nrf52(Nrf52Host::Nrf52),
        Endpoint::BlueZ(BlueZHost::Linux),
        INTERRUPTION_TRACE_CAPACITY,
        recover_after_each_direction,
    );
}
