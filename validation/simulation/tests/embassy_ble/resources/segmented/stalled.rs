use super::*;
use crate::wire_gate::WireGate;

const ADVERTISEMENTS_BEFORE_LOSS: usize = 1;
const DROPPED_ADVERTISEMENT_BUDGET: usize = 16;

fn stall_until_timeout<T: 'static>(
    tasks: &mut EmbassyTasks<'_>,
    lab: &VirtualBleLab,
    gate: &WireGate,
    link: LinkId,
    request: impl Future<Output = T> + 'static,
) -> T {
    let header = buffered_interruption::advertisement_header(link);
    gate.lose_after(
        header,
        ADVERTISEMENTS_BEFORE_LOSS,
        NonZeroUsize::new(DROPPED_ADVERTISEMENT_BUDGET).unwrap(),
    );
    let sender = gate.clone();
    let result = tasks.complete_with_budget(
        CompletionBudget {
            deadline: tick(tasks.snapshot().tick.get() + INTERRUPTION_BUDGET_MS),
            polls_per_tick: NonZeroUsize::new(TRANSFER_POLLS_PER_TICK).unwrap(),
        },
        async move {
            let result = request.await;
            assert!(sender.stop_loss() > 0);
            result
        },
    );
    assert_eq!(lab.active_connection_count(), 1);
    assert!(gate.is_idle());
    result
}

fn exercise(
    tasks: &mut EmbassyTasks<'_>,
    lab: &VirtualBleLab,
    embedded: &ResourceNode,
    desktop: &tokio_node::TokioNode,
    links: [LinkId; 2],
    requester: Requester,
) {
    let spare = establish_pair(tasks, embedded, &desktop.handle);
    match requester {
        Requester::Tokio => {
            let handle = desktop.handle.clone();
            let result = stall_until_timeout(tasks, lab, &embedded.wire, links[1], async move {
                handle
                    .request(
                        links[1],
                        RequestPathHash::of(files::FILE_PATH),
                        &(TRANSFER_BYTES as u16).to_be_bytes(),
                    )
                    .await
            });
            assert_eq!(result, Err(SendError::Failed(SendRequestFailure::Timeout)));
            assert_eq!(
                response_settlements(embedded),
                [Settlement::Respond(Err(RespondFailure::Resource(
                    SendResourceFailure::Timeout
                )))]
            );
            journaled_request(
                tasks,
                &desktop.handle,
                desktop.responses.clone(),
                spare[1],
                &AdmissionCase::ThreeSegments,
            );
            assert_eq!(
                response_settlements(embedded),
                [Settlement::Respond(Ok(()))]
            );
        }
        Requester::Embassy => {
            let handle = embedded.handle;
            let result = stall_until_timeout(tasks, lab, &desktop.wire, links[0], async move {
                handle
                    .request(
                        links[0],
                        RequestPathHash::of(files::FILE_PATH),
                        &(TRANSFER_BYTES as u16).to_be_bytes(),
                    )
                    .await
            });
            assert_eq!(result, Err(SendError::Failed(SendRequestFailure::Timeout)));
            assert!(response_settlements(embedded).is_empty());
            let expected = journaled_request(
                tasks,
                &embedded.handle,
                embedded.responses.clone(),
                spare[0],
                &AdmissionCase::ThreeSegments,
            );
            assert_eq!(embedded.take_settled(), [expected]);
        }
    }
    assert!(embedded.take_closed().is_empty());
    assert!(desktop.take_closed().is_empty());
    reassemble(tasks, lab, embedded, desktop, links);
}

#[test]
fn apple_reclaims_timed_out_assembly_without_retiring_the_link() {
    scenario(
        Endpoint::Esp32(Esp32Host::Esp32),
        Endpoint::CoreBluetooth(AppleHost::MacOs),
        INTERRUPTION_TRACE_CAPACITY,
        |tasks, lab, embedded, desktop, links| {
            exercise(tasks, lab, embedded, desktop, links, Requester::Tokio)
        },
    );
}

#[test]
fn bluez_reclaims_timed_out_assembly_without_retiring_the_link() {
    scenario(
        Endpoint::Nrf52(Nrf52Host::Nrf52),
        Endpoint::BlueZ(BlueZHost::Linux),
        INTERRUPTION_TRACE_CAPACITY,
        |tasks, lab, embedded, desktop, links| {
            exercise(tasks, lab, embedded, desktop, links, Requester::Tokio)
        },
    );
}

#[test]
fn esp32_reclaims_timed_out_assembly_without_retiring_the_link() {
    scenario(
        Endpoint::Esp32(Esp32Host::Esp32),
        Endpoint::CoreBluetooth(AppleHost::MacOs),
        INTERRUPTION_TRACE_CAPACITY,
        |tasks, lab, embedded, desktop, links| {
            exercise(tasks, lab, embedded, desktop, links, Requester::Embassy)
        },
    );
}

#[test]
fn nrf52_reclaims_timed_out_assembly_without_retiring_the_link() {
    scenario(
        Endpoint::Nrf52(Nrf52Host::Nrf52),
        Endpoint::BlueZ(BlueZHost::Linux),
        INTERRUPTION_TRACE_CAPACITY,
        |tasks, lab, embedded, desktop, links| {
            exercise(tasks, lab, embedded, desktop, links, Requester::Embassy)
        },
    );
}
