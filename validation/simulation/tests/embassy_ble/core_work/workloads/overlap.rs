use super::*;

pub const CHANNEL_KIND: MessageType = MessageType(0x3344);

pub fn healthy(triple: &mut Triple<'_, '_>) {
    assert_eq!(
        triple.exchange(HEALTHY, RemoteControlRequest::DescribeBuild),
        Ok(RemoteControlResponse::DescribeBuild(
            RemoteControlBuildVersion::from_text("simulation-v1").expect("build label")
        ))
    );
}

pub fn overlap(triple: &mut Triple<'_, '_>, marker: u8) {
    let primary = triple.nodes[PRIMARY].handle.clone();
    let healthy = triple.nodes[HEALTHY].handle.clone();
    let app_link = triple.app_links[0];
    let control_link = triple.links[1];
    let channel = [marker; 16];
    let command = triple.issue(
        PRIMARY,
        PrnsCommand::SendToChannel(SendToChannel {
            link_id: app_link,
            message_type: CHANNEL_KIND,
            body: SendToChannelBody::from_slice(&channel).expect("bounded channel"),
        }),
    );
    let payload = RemoteControlAppMessage::from_slice(&[marker; 48]).expect("bounded app message");
    let expected = payload.clone();
    let (resource, ordinary, control) = triple.complete(async move {
        tokio::join!(
            fixture::app_request(
                primary.clone(),
                app_link,
                traffic::RESOURCE_PATH,
                (traffic::COMMON_RESPONSE_BYTES as u16)
                    .to_be_bytes()
                    .to_vec()
            ),
            fixture::app_request(primary, app_link, traffic::ECHO_PATH, vec![marker; 24]),
            fixture::exchange(
                healthy,
                control_link,
                RemoteControlRequest::AppMessage(payload)
            ),
        )
    });
    assert_eq!(
        resource,
        Ok(traffic::PAYLOAD[..traffic::COMMON_RESPONSE_BYTES].to_vec())
    );
    assert_eq!(ordinary, Ok(vec![marker; 24]));
    assert_eq!(control, Ok(RemoteControlResponse::AppMessage(expected)));
    let settlement = triple.nodes[PRIMARY]
        .events
        .snapshot()
        .into_iter()
        .filter_map(|event| match event {
            Event::Settled {
                id,
                settlement: CommandOutcome::Channel(outcome),
            } if id == command => Some(outcome),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(
        matches!(settlement.as_slice(), [Ok(_)]),
        "one delivered command: {settlement:?}"
    );
    assert!(triple.nodes[TARGET].events.snapshot().iter().any(|event| matches!(event, Event::Channel { link, kind, bytes } if *link == app_link && *kind == CHANNEL_KIND && bytes == &channel)));
    assert!(!triple.nodes[HEALTHY]
        .events
        .snapshot()
        .iter()
        .any(|event| matches!(event, Event::Channel { kind, .. } if *kind == CHANNEL_KIND)));
    assert_eq!(
        triple
            .messages
            .0
            .borrow()
            .last()
            .expect("verified handler")
            .identity,
        crate::remote_control::secrets(HEALTHY)
            .identities()
            .controller()
            .identity_hash()
    );
}
