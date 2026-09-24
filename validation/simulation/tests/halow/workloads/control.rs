use super::*;
use personal_rns::remote_control::*;
pub fn overlap(lab: &mut Lab<'_>) {
    lab.announce(TARGET);
    let primary = lab.link(PRIMARY);
    let healthy = lab.link(HEALTHY);
    let outsider = lab.link(OUTSIDER);
    let response_count = lab.response_count(TARGET);
    let unlisted = lab.request(
        OUTSIDER,
        outsider,
        RemoteControlRequest::AppMessage(
            RemoteControlAppMessage::from_slice(b"unlisted").expect("message"),
        ),
    );
    let Event::Response(reply) = lab.wait(unlisted, 1000) else {
        panic!("unlisted outcome");
    };
    assert_eq!(
        reply,
        Err(personal_rns::runtime::SendError::Failed(
            personal_rns::engine::SendRequestFailure::Timeout
        ))
    );
    assert_eq!(
        lab.response_count(TARGET),
        response_count,
        "no unauthorized response"
    );
    assert!(!lab
        .calls
        .borrow()
        .iter()
        .any(|call| call.controller == controller(OUTSIDER)));
    let payload = RemoteControlAppMessage::from_slice(&[0x31; 96]).expect("maximum message");
    let resource = lab.request_bytes(
        PRIMARY,
        primary,
        crate::traffic::RESOURCE_PATH,
        Vec::new(),
        10_000,
    );
    let app = lab.request(
        HEALTHY,
        healthy,
        RemoteControlRequest::AppMessage(payload.clone()),
    );
    let inventory = lab.request(
        HEALTHY,
        healthy,
        RemoteControlRequest::InventoryInterfaces {
            page: RemoteControlInterfacePage::First,
        },
    );
    for (task, event) in lab.wait_many(&[resource, app, inventory], 10_000) {
        let Event::Response(reply) = event else {
            panic!("overlap outcome");
        };
        let bytes = reply.expect("concurrent request");
        if task == resource {
            assert_eq!(bytes, crate::traffic::PAYLOAD);
            continue;
        }
        let response = RemoteControlResponse::parse(&bytes).expect("typed response");
        if task == app {
            assert_eq!(response, RemoteControlResponse::AppMessage(payload.clone()));
            continue;
        }
        let RemoteControlResponse::InventoryInterfaces(page) = response else {
            panic!("real interface page");
        };
        assert_eq!(page.entries().len(), 1);
        assert_eq!(page.entries()[0].id, lab.nodes[TARGET].adapter_id());
    }
    let id = lab.nodes[TARGET].adapter_id();
    assert!(matches!(
        lab.exchange(
            HEALTHY,
            healthy,
            RemoteControlRequest::InventoryInterfaceConfig { id }
        ),
        RemoteControlResponse::InventoryInterfaceConfig(RemoteControlInterfaceConfigOutcome::Card(
            _
        ))
    ));
    let RemoteControlResponse::InventoryInterfacePeers(RemoteControlInterfacePeersOutcome::Page(
        page,
    )) = lab.exchange(
        HEALTHY,
        healthy,
        RemoteControlRequest::InventoryInterfacePeers {
            id,
            page: RemoteControlPeerPage::First,
        },
    )
    else {
        panic!("peer page");
    };
    let mut expected = lab.peer_ids(TARGET);
    expected.push(personal_rns::interfaces::InterfaceId::from_channel_tag(
        personal_rns::interfaces::InterfaceKind::WifiHaLowBroadcast,
        &scope(TARGET).channel_tag(),
    ));
    expected.sort_by_key(|id| *id.as_bytes());
    assert_eq!(
        page.peers.iter().map(|peer| peer.id).collect::<Vec<_>>(),
        expected
    );
    for peer in page.peers {
        assert_eq!(
            peer.radio,
            personal_rns::interfaces::RadioIndication::HaLow(
                personal_rns::interfaces::WifiIndication::Unavailable
            )
        );
    }
    assert!(lab.calls.borrow().contains(&AppInvocation {
        controller: controller(HEALTHY),
        payload: vec![0x31; 96]
    }));
    wire_contract(lab);
}
#[test]
fn actual_halow_inventory_and_verified_app_messages_overlap_with_resources() {
    for seed in [0, 1, 42, 0x5eed] {
        let run = || {
            let medium = medium();
            with_lab(medium.clone(), seed, Topology::Shared, overlap);
            medium.snapshot()
        };
        assert_eq!(run(), run());
    }
}
