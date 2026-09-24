use super::*;

pub fn persistence_gate(triple: &mut Triple<'_, '_>) {
    let Storage::Tokio(storage) = &triple.storage[TARGET] else {
        overlap(triple, 0x70);
        return;
    };
    let storage = storage.clone();
    let gate = storage.io.gate(PersistenceIoOperation::AuthorizationStore(
        personal_rns::persistence::SnapshotRegion::RemoteControlControllerGrants,
    ));
    let before = storage.io.events().len();
    let target = triple.nodes[TARGET].handle.clone();
    let requests = crate::remote_control::requests()
        .iter()
        .filter(|request| *request != RemoteControlRequestKind::WatchInterfaces)
        .fold(RemoteControlRequestSet::empty(), |mut requests, request| {
            requests.insert(request);
            requests
        });
    let grant = RemoteControlControllerGrant::new(
        *crate::remote_control::secrets(PRIMARY)
            .identities()
            .controller(),
        RemoteControlControllerAuthority::Administrator,
        requests,
    )
    .expect("explicit controller grant");
    let mutation = Operation::start(triple, async move { target.grant(grant).await });
    let io = storage.io.clone();
    triple.drive_until(move || {
        io.events()[before..].iter().any(|event| {
            *event
                == crate::remote_control::node::file::IoEvent::Started(
                    PersistenceIoOperation::AuthorizationStore(
                        personal_rns::persistence::SnapshotRegion::RemoteControlControllerGrants,
                    ),
                )
        })
    });
    let primary = triple.nodes[PRIMARY].handle.clone();
    let link = triple.app_links[0];
    let request = Operation::start(triple, async move {
        fixture::app_request(
            primary,
            link,
            traffic::ECHO_PATH,
            b"during-storage".to_vec(),
        )
        .await
    });
    let channel = triple.issue(
        HEALTHY,
        PrnsCommand::SendToChannel(SendToChannel {
            link_id: triple.app_links[1],
            message_type: CHANNEL_KIND,
            body: SendToChannelBody::from_slice(b"independent-engine-work")
                .expect("channel during storage"),
        }),
    );
    let events = triple.nodes[HEALTHY].events.clone();
    triple.drive_until(move || events.snapshot().iter().any(|event| matches!(event, Event::Settled { id, settlement: CommandOutcome::Channel(Ok(_)) } if *id == channel)));
    assert!(
        matches!(*request.state(), State::Pending),
        "authority transaction pauses fresh endpoint admission, while engine work continues"
    );
    gate.send(()).expect("release real FileStore write");
    mutation.finish(triple).expect("grant committed");
    assert_eq!(request.finish(triple), Ok(b"during-storage".to_vec()));
    let target = triple.nodes[TARGET].handle.clone();
    let full = RemoteControlControllerGrant::new(
        *crate::remote_control::secrets(PRIMARY)
            .identities()
            .controller(),
        RemoteControlControllerAuthority::Administrator,
        crate::remote_control::requests(),
    )
    .expect("full grant");
    triple.complete(async move {
        target.grant(full).await.expect("restore inspection grant");
    });
    healthy(triple);
}
