use super::*;

pub(super) struct CalibratedSegments {
    pub settlement: (CommandId, Settlement),
    parts: Vec<Vec<u8>>,
}

impl CalibratedSegments {
    pub(super) fn suffix(
        &self,
        link: LinkId,
        request: RequestId,
        after: DropAfter,
    ) -> Vec<ResponseEvent> {
        self.parts
            .iter()
            .enumerate()
            .skip(after.count())
            .map(|(index, bytes)| ResponseEvent::Segment {
                link,
                request,
                index: (index + 1) as u64,
                total: self.parts.len() as u64,
                bytes: bytes.clone(),
            })
            .collect()
    }
}

pub(super) async fn calibrate(
    handle: &impl PrnsNodeApi,
    trace: &ResponseTrace,
    gate: &WireGate,
    responder: &RespondProbe,
    link: LinkId,
    after: DropAfter,
) -> (CalibratedSegments, (CommandId, Settlement)) {
    after.block_continuation(gate, link);
    responder.arm();
    let started = tokio::time::Instant::now();
    let command = handle
        .issue(PrnsCommand::SendRequest(SendRequest {
            link_id: link,
            path_hash: RequestPathHash::of(files::FILE_PATH),
            data: SendRequestData::from_slice(&(TRANSFER_BYTES as u16).to_be_bytes()).unwrap(),
            response_timeout: RequestResponseTimeout::LinkDefault,
            maximum_response_bytes: ByteLimit::Maximum(TRANSFER_BYTES as u64),
        }))
        .unwrap();
    assert_eq!(
        gate.first_loss().await,
        buffered_interruption::advertisement_header(link)
    );
    let mut events = Vec::new();
    for index in 1..=after.count() {
        let event = trace.next().await;
        assert!(
            matches!(&event, ResponseEvent::Segment { index: observed, total: 3, .. } if *observed == index as u64)
        );
        events.push(event);
    }
    assert!(
        trace.is_empty(),
        "the blocked continuation cannot deliver another segment"
    );
    assert!(gate.stop_loss() > 0);
    let ((remaining, elapsed), responded) = tokio::join!(
        async {
            let remaining = trace.completed().await;
            let elapsed = RttMillis::new(started.elapsed().as_millis().try_into().unwrap());
            (remaining, elapsed)
        },
        responder.take(),
    );
    events.extend(remaining);
    let result = Ok(PacketReceiptDelivered {
        rtt: elapsed,
        evidence: DeliveryEvidence::Response,
    });
    assert_eq!(events.len(), 4);
    assert_eq!(
        events.last(),
        Some(&ResponseEvent::Settled { command, result })
    );
    let ResponseEvent::Segment { request, .. } = events[0] else {
        unreachable!("calibration begins with a verified file segment")
    };
    let mut parts = Vec::new();
    let mut offset = 0;
    for (index, event) in events[..3].iter().enumerate() {
        let ResponseEvent::Segment { bytes, .. } = event else {
            unreachable!("three segments precede settlement")
        };
        assert!(!bytes.is_empty() && bytes.len() <= TRANSFER_WINDOW_BYTES);
        let end = offset + bytes.len();
        assert_eq!(
            *event,
            ResponseEvent::Segment {
                link,
                request,
                index: (index + 1) as u64,
                total: 3,
                bytes: files::FILE_BYTES[offset..end].to_vec(),
            }
        );
        parts.push(bytes.clone());
        offset = end;
    }
    assert_eq!(offset, TRANSFER_BYTES);
    assert_eq!(responded.result, Ok(()));
    assert!(trace.is_empty());
    (
        CalibratedSegments {
            settlement: (command, Settlement::SendRequest(result)),
            parts,
        },
        (responded.command, Settlement::Respond(responded.result)),
    )
}
