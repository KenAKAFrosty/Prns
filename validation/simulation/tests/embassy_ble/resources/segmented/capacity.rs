use super::*;

const SEGMENTS: u64 = 5;

fn calibrate(
    tasks: &mut EmbassyTasks<'_>,
    handle: &impl PrnsNodeApi,
    trace: ResponseTrace,
    link: LinkId,
    requested: usize,
) -> (CommandId, Settlement) {
    assert!(trace.is_empty());
    let started = tasks.snapshot().tick;
    let command = handle
        .issue(PrnsCommand::SendRequest(SendRequest {
            link_id: link,
            path_hash: RequestPathHash::of(files::FILE_PATH),
            data: SendRequestData::from_slice(&(requested as u16).to_be_bytes()).unwrap(),
            response_timeout: RequestResponseTimeout::LinkDefault,
            maximum_response_bytes: ByteLimit::Maximum(RESPONSE_BYTES as u64),
        }))
        .unwrap();
    let events = complete(tasks, async move { trace.completed().await });
    let result = if requested > RESPONSE_BYTES {
        Err(SendRequestFailure::ResponseTooLarge)
    } else {
        Ok(PacketReceiptDelivered {
            rtt: RttMillis::new(tasks.snapshot().tick.get() - started.get()),
            evidence: DeliveryEvidence::Response,
        })
    };
    let expected_parts = if requested > RESPONSE_BYTES {
        SEGMENTS - 1
    } else {
        SEGMENTS
    };
    assert_eq!(events.len(), expected_parts as usize + 1);
    let Some(ResponseEvent::Segment { request, .. }) = events.first() else {
        unreachable!("the boundary response must arrive as verified segments")
    };
    let mut expected = Vec::new();
    let mut offset = 0;
    for (position, event) in events[..expected_parts as usize].iter().enumerate() {
        let ResponseEvent::Segment { bytes, .. } = event else {
            unreachable!("no alternate delivery or early terminal outcome")
        };
        assert!(!bytes.is_empty() && bytes.len() <= TRANSFER_WINDOW_BYTES);
        let end = offset + bytes.len();
        expected.push(ResponseEvent::Segment {
            link,
            request: *request,
            index: position as u64 + 1,
            total: SEGMENTS,
            bytes: files::FILE_BYTES[offset..end].to_vec(),
        });
        offset = end;
    }
    if requested > RESPONSE_BYTES {
        assert!(offset > 3 * TRANSFER_WINDOW_BYTES && offset < RESPONSE_BYTES);
    } else {
        assert_eq!(offset, requested);
    }
    expected.push(ResponseEvent::Settled { command, result });
    assert_eq!(events, expected);
    (command, Settlement::SendRequest(result))
}

fn responder_outcome(requested: usize) -> Settlement {
    if requested > RESPONSE_BYTES {
        Settlement::Respond(Err(RespondFailure::Resource(
            SendResourceFailure::RejectedByPeer,
        )))
    } else {
        Settlement::Respond(Ok(()))
    }
}

fn exercise(
    tasks: &mut EmbassyTasks<'_>,
    lab: &VirtualBleLab,
    embedded: &ResourceNode,
    desktop: &tokio_node::TokioNode,
    links: [LinkId; 2],
) {
    // End with an exact-capacity success to prove reuse after late refusal.
    for requested in [
        RESPONSE_BYTES - 1,
        RESPONSE_BYTES,
        RESPONSE_BYTES + 1,
        RESPONSE_BYTES,
    ] {
        calibrate(
            tasks,
            &desktop.handle,
            desktop.responses.clone(),
            links[1],
            requested,
        );
        assert_eq!(
            response_settlements(embedded),
            [responder_outcome(requested)]
        );
        let settled = calibrate(
            tasks,
            &embedded.handle,
            embedded.responses.clone(),
            links[0],
            requested,
        );
        assert_eq!(embedded.take_settled(), [settled]);

        let handle = desktop.handle.clone();
        let (result, elapsed) = complete(tasks, async move {
            measured(handle.request_with_options(
                links[1],
                RequestPathHash::of(files::FILE_PATH),
                &(requested as u16).to_be_bytes(),
                RequestOptions {
                    response_timeout: RequestResponseTimeout::LinkDefault,
                    maximum_response_bytes: ByteLimit::Maximum(RESPONSE_BYTES as u64),
                },
            ))
            .await
        });
        assert_eq!(
            result,
            if requested > RESPONSE_BYTES {
                Err(SendError::Failed(SendRequestFailure::ResponseTooLarge))
            } else {
                Ok((files::FILE_BYTES[..requested].to_vec(), elapsed))
            }
        );
        assert_eq!(
            response_settlements(embedded),
            [responder_outcome(requested)]
        );

        let handle = embedded.handle;
        let (result, elapsed) = complete(tasks, async move {
            measured(handle.request(
                links[0],
                RequestPathHash::of(files::FILE_PATH),
                &(requested as u16).to_be_bytes(),
            ))
            .await
        });
        assert_eq!(
            result.map(|(bytes, rtt)| (bytes.as_slice().to_vec(), rtt)),
            if requested > RESPONSE_BYTES {
                Err(SendError::Failed(SendRequestFailure::ResponseTooLarge))
            } else {
                Ok((files::FILE_BYTES[..requested].to_vec(), elapsed))
            }
        );
        assert!(embedded.take_settled().is_empty());
        assert!(embedded.responses.is_empty());
        assert!(desktop.responses.is_empty());
        assert_eq!(lab.active_connection_count(), 1);
    }
    reuse_both_embassy_request_slots(tasks, embedded, links[0]);
}

#[test]
fn esp32_and_apple_segmented_responses_honor_the_completion_capacity() {
    scenario(
        Endpoint::Esp32(Esp32Host::Esp32),
        Endpoint::CoreBluetooth(AppleHost::MacOs),
        TRACE_CAPACITY,
        exercise,
    );
}

#[test]
fn nrf52_and_bluez_segmented_responses_honor_the_completion_capacity() {
    scenario(
        Endpoint::Nrf52(Nrf52Host::Nrf52),
        Endpoint::BlueZ(BlueZHost::Linux),
        TRACE_CAPACITY,
        exercise,
    );
}
