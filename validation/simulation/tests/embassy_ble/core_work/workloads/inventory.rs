use super::*;

pub fn inventory(triple: &mut Triple<'_, '_>) {
    let Ok(RemoteControlResponse::InventoryInterfaces(page)) = triple.exchange(
        PRIMARY,
        RemoteControlRequest::InventoryInterfaces {
            page: RemoteControlInterfacePage::First,
        },
    ) else {
        panic!("admitted interface inventory");
    };
    assert_eq!(page.entries().len(), 1);
    let id = page.entries()[0].id;
    assert!(matches!(
        triple.exchange(
            PRIMARY,
            RemoteControlRequest::InventoryInterfaceConfig { id }
        ),
        Ok(RemoteControlResponse::InventoryInterfaceConfig(
            RemoteControlInterfaceConfigOutcome::Card(_)
        ))
    ));
    let Ok(RemoteControlResponse::InventoryInterfacePeers(
        RemoteControlInterfacePeersOutcome::Page(page),
    )) = triple.exchange(
        PRIMARY,
        RemoteControlRequest::InventoryInterfacePeers {
            id,
            page: RemoteControlPeerPage::First,
        },
    )
    else {
        panic!("admitted peer inventory");
    };
    assert_eq!(
        page.peers.len(),
        2,
        "both independently paired controllers visible"
    );
    for peer in page.peers {
        assert_eq!(
            peer.connection,
            personal_rns::interfaces::ConnectionState::Connected
        );
        assert_eq!(
            peer.rate_bytes_per_sec, None,
            "unavailable measurement remains unavailable"
        );
    }
}

pub fn watch(triple: &mut Triple<'_, '_>) {
    let stream = StreamId::new(3).expect("watch stream");
    if let (Handle::Tokio(controller), Handle::Tokio(_)) =
        (&triple.nodes[PRIMARY].handle, &triple.nodes[TARGET].handle)
    {
        let controller = controller.clone();
        let link = triple.links[0];
        triple.complete(async move {
            let (mut watch, _) = controller
                .remote_control(link)
                .watch_interfaces(stream)
                .await
                .expect("watch admitted");
            assert_eq!(
                watch.next_event().await.expect("initial resync"),
                RemoteControlStreamEvent::ResyncRequired { sequence: 1 }
            );
        });
    } else {
        let target = triple.profile.runtimes[1].clone();
        let reply = triple.exchange(
            PRIMARY,
            RemoteControlRequest::WatchInterfaces { stream_id: stream },
        );
        match target {
            crate::remote_control::adapter::Runtime::Tokio => assert_eq!(
                reply,
                Ok(RemoteControlResponse::WatchInterfaces { stream_id: stream })
            ),
            crate::remote_control::adapter::Runtime::Embassy => {
                assert_eq!(reply, Err(SendError::Failed(SendRequestFailure::Timeout)))
            }
        }
    }
    triple.reconnect(PRIMARY);
    inventory(triple);
}
