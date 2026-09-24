use super::*;

pub fn refused_response(triple: &mut Triple<'_, '_>) {
    let link = triple.app_links[0];
    let command = triple.issue(
        PRIMARY,
        PrnsCommand::SendRequest(SendRequest {
            link_id: link,
            path_hash: personal_rns::routing::request_handlers::RequestPathHash::of(
                traffic::RESOURCE_PATH,
            ),
            data: SendRequestData::from_slice(
                &(traffic::COMMON_RESPONSE_BYTES as u16).to_be_bytes(),
            )
            .expect("resource query"),
            response_timeout: RequestResponseTimeout::LinkDefault,
            maximum_response_bytes: ByteLimit::Maximum(0),
        }),
    );
    let events = triple.nodes[PRIMARY].events.clone();
    let observed = events.clone();
    triple.drive_until(move || {
        observed
            .snapshot()
            .iter()
            .any(|event| matches!(event, Event::Settled { id, .. } if *id == command))
    });
    let outcomes: Vec<_> = events
        .snapshot()
        .into_iter()
        .filter_map(|event| match event {
            Event::Settled {
                id,
                settlement: CommandOutcome::Request(result),
            } if id == command => Some(result),
            _ => None,
        })
        .collect();
    assert_eq!(
        outcomes,
        [Err(SendRequestFailure::ResponseTooLarge)],
        "original refusal survives transfer cancellation"
    );
    healthy(triple);
    let handle = triple.nodes[PRIMARY].handle.clone();
    assert_eq!(
        triple.complete(async move {
            fixture::app_request(handle, link, traffic::ECHO_PATH, b"reused".to_vec()).await
        }),
        Ok(b"reused".to_vec())
    );
}
