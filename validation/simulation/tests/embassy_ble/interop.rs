use std::time::Duration;

use personal_rns::engine::{
    AnnounceAppData, AnnounceNow, AnnounceTarget, LinkClosedReason, SendRequestFailure,
    SendRequestRejection, SendResourceFailure, Settlement, MAX_SEND_REQUEST_DATA_LEN,
};
use personal_rns::interfaces::bluetooth_auto::{
    AppleHost, BleAddress, BlueZHost, Endpoint, Esp32Host, Nrf52Host,
};
use personal_rns::interfaces::{
    ConnectionState, InterfaceId, InterfaceKind, InterfaceStatus, Membership,
};
use personal_rns::routing::links::LinkId;
use personal_rns::routing::request_handlers::RequestPathHash;
use personal_rns::runtime::{PrnsNodeHandle, SendError};
use personal_rns::units::RttMillis;
use prns_simulation::ble::{BleSimulationEvent, VirtualBleDisconnectReport, VirtualBleLab};
use prns_simulation::{ManualMedium, ManualTimeDriver};

use super::clock::{ClockLease, EmbassyTasks};
use super::echo::{destination, QUERY_PATH};
use super::fixture::lab;
use super::node::{Node, PAYLOAD_BYTES};
use super::{tick, tokio_node};

const EMBASSY_ADDRESS: u8 = 1;
const TOKIO_ADDRESS: u8 = 2;
const DISCOVERY_BUDGET_MS: u64 = 60_000;

fn peer(address: u8) -> InterfaceId {
    InterfaceId::from_channel_tag(InterfaceKind::BluetoothPeer, &[address; 16])
}

fn members(embassy: &Node, tokio: &PrnsNodeHandle) -> [Vec<(InterfaceId, ConnectionState)>; 2] {
    [
        embassy
            .status
            .members()
            .map(|member| (member.id(), member.connection()))
            .collect(),
        tokio
            .interfaces()
            .into_iter()
            .filter_map(|interface| match interface.membership {
                Membership::Independent => None,
                Membership::FleetMember { .. } => Some((interface.id, interface.connection)),
            })
            .collect(),
    ]
}

fn converge(
    tasks: &mut EmbassyTasks<'_>,
    lab: &VirtualBleLab,
    embassy: &Node,
    tokio: &PrnsNodeHandle,
) {
    let expected = [
        vec![(peer(TOKIO_ADDRESS), ConnectionState::Connected)],
        vec![(peer(EMBASSY_ADDRESS), ConnectionState::Connected)],
    ];
    let deadline = tasks.snapshot().tick.get() + DISCOVERY_BUDGET_MS;
    for _ in 0..=DISCOVERY_BUDGET_MS * 2 {
        let _ = tasks.settle();
        if lab.active_connection_count() == 1 && members(embassy, tokio) == expected {
            return;
        }
        let now = tasks.snapshot().tick.get();
        assert!(now < deadline, "mixed-runtime discovery must converge");
        let _ = tasks.advance(tick(now + 1)).unwrap();
    }
    unreachable!("bounded discovery step count")
}

fn establish_pair(
    tasks: &mut EmbassyTasks<'_>,
    embassy: &Node,
    desktop: &PrnsNodeHandle,
) -> [LinkId; 2] {
    let embedded = embassy.handle;
    let tokio = desktop.clone();
    assert_eq!(
        tasks.complete_ready(async move {
            let announce = |address| AnnounceNow {
                destination: destination(address).destination_hash().unwrap(),
                target: AnnounceTarget::AllInterfaces,
                app_data: AnnounceAppData::Registered,
            };
            tokio::join!(
                embedded.announce_now(announce(EMBASSY_ADDRESS)),
                tokio.announce_now(announce(TOKIO_ADDRESS))
            )
        }),
        (Ok(()), Ok(()))
    );
    let embedded = embassy.handle;
    let tokio = desktop.clone();
    let (from_embassy, from_tokio) = tasks.complete_ready(async move {
        tokio::join!(
            embedded.establish_link(destination(TOKIO_ADDRESS).destination_hash().unwrap()),
            tokio.establish_link(destination(EMBASSY_ADDRESS).destination_hash().unwrap()),
        )
    });
    let links = [from_embassy.unwrap(), from_tokio.unwrap()];
    assert_ne!(links[0], links[1]);
    links
}

fn exchange(
    tasks: &mut EmbassyTasks<'_>,
    embassy: &Node,
    tokio: &PrnsNodeHandle,
    links: [LinkId; 2],
    round: u8,
) {
    let embedded = embassy.handle;
    let desktop = tokio.clone();
    let mut payloads = [[0xA1; PAYLOAD_BYTES], [0xB2; PAYLOAD_BYTES]];
    for payload in &mut payloads {
        payload[0] = round;
    }
    let results = tasks.complete_ready(async move {
        tokio::join!(
            embedded.request(links[0], RequestPathHash::of(QUERY_PATH), &payloads[0]),
            desktop.request(links[1], RequestPathHash::of(QUERY_PATH), &payloads[1]),
        )
    });
    assert_eq!(
        results
            .0
            .map(|(bytes, rtt)| (bytes.as_slice().to_vec(), rtt)),
        Ok((payloads[0].to_vec(), RttMillis::new(0)))
    );
    assert_eq!(results.1, Ok((payloads[1].to_vec(), RttMillis::new(0))));
    assert!(embassy.take_received().is_empty());
    assert_eq!(
        embassy
            .take_settled()
            .into_iter()
            .map(|(_, settlement)| settlement)
            .collect::<Vec<_>>(),
        vec![Settlement::Respond(Ok(()))]
    );
}

fn refuse_expired_requests(
    tasks: &mut EmbassyTasks<'_>,
    embassy: &Node,
    tokio: &PrnsNodeHandle,
    links: [LinkId; 2],
) {
    let rejected = Err(SendError::Failed(SendRequestFailure::Rejected(
        SendRequestRejection::NoSuchLink,
    )));
    for size in [0, PAYLOAD_BYTES] {
        let embedded = embassy.handle;
        let desktop = tokio.clone();
        let results = tasks.complete_ready(async move {
            let data = vec![0xC3; size];
            let (embedded, desktop) = tokio::join!(
                embedded.request(links[0], RequestPathHash::of(QUERY_PATH), &data),
                desktop.request(links[1], RequestPathHash::of(QUERY_PATH), &data),
            );
            [embedded.map(|_| ()), desktop.map(|_| ())]
        });
        assert_eq!(results, [rejected; 2]);
    }
    let desktop = tokio.clone();
    assert_eq!(
        tasks.complete_ready(async move {
            desktop
                .request(
                    links[1],
                    RequestPathHash::of(QUERY_PATH),
                    &[0xD4; MAX_SEND_REQUEST_DATA_LEN + 1],
                )
                .await
                .map(|_| ())
        }),
        rejected
    );
    assert!(embassy.take_settled().is_empty());
}

fn refuse_oversized_transfer(tasks: &mut EmbassyTasks<'_>, tokio: &PrnsNodeHandle, link: LinkId) {
    let desktop = tokio.clone();
    assert_eq!(
        tasks.complete_ready(async move {
            desktop
                .request(
                    link,
                    RequestPathHash::of(QUERY_PATH),
                    &[0xD4; MAX_SEND_REQUEST_DATA_LEN + 1],
                )
                .await
        }),
        Err(SendError::Failed(
            SendRequestFailure::RequestTransferFailed(SendResourceFailure::RejectedByPeer)
        ))
    );
}

fn scenario(embedded_endpoint: Endpoint, desktop_endpoint: Endpoint) {
    let clock = ClockLease::acquire();
    let lab = lab();
    let mut driver =
        ManualTimeDriver::new(ManualMedium::Ble(lab.clone()), Duration::from_millis(1)).unwrap();
    let mut tasks = EmbassyTasks::new(&mut driver, clock);
    let embassy = Node::start_echo(&mut tasks, &lab, EMBASSY_ADDRESS, embedded_endpoint);
    let mut desktop = tokio_node::start(&mut tasks, &lab, TOKIO_ADDRESS, desktop_endpoint);
    let mut attached: Vec<_> = lab
        .trace()
        .events
        .into_iter()
        .filter_map(|event| match event {
            BleSimulationEvent::RadioAttached { radio } => Some(radio),
            _ => None,
        })
        .collect();
    attached.sort();
    assert_eq!(attached.len(), 2);
    assert!(tasks.settle() > 0);
    let desktop = desktop.try_recv().unwrap();
    converge(&mut tasks, &lab, &embassy, &desktop.handle);
    let links = establish_pair(&mut tasks, &embassy, &desktop.handle);
    refuse_oversized_transfer(&mut tasks, &desktop.handle, links[1]);
    exchange(&mut tasks, &embassy, &desktop.handle, links, 0);

    assert_eq!(
        lab.disconnect_between(
            BleAddress::new([EMBASSY_ADDRESS; 6]),
            BleAddress::new([TOKIO_ADDRESS; 6])
        ),
        VirtualBleDisconnectReport {
            connections_closed: 1
        }
    );
    assert!(tasks.settle() > 0);
    assert_eq!(lab.active_connection_count(), 0);
    assert_eq!(members(&embassy, &desktop.handle), [vec![], vec![]]);
    converge(&mut tasks, &lab, &embassy, &desktop.handle);
    let mut expired: Vec<_> = links
        .into_iter()
        .map(|link| (link, LinkClosedReason::Timeout))
        .collect();
    expired.sort_by_key(|(link, _)| *link.as_bytes());
    assert_eq!(embassy.take_closed(), expired);
    assert_eq!(desktop.take_closed(), expired);
    refuse_expired_requests(&mut tasks, &embassy, &desktop.handle, links);
    let fresh = establish_pair(&mut tasks, &embassy, &desktop.handle);
    for (old, new) in links.into_iter().zip(fresh) {
        assert_ne!(old, new);
    }
    exchange(&mut tasks, &embassy, &desktop.handle, fresh, 1);
    assert_eq!(embassy.status.setup_failure_events(), 0);
    assert!(embassy.take_closed().is_empty());
    assert!(desktop.take_closed().is_empty());
    drop(tasks);
    assert_eq!(lab.active_connection_count(), 0);
    let mut detached: Vec<_> = lab
        .trace()
        .events
        .into_iter()
        .filter_map(|event| match event {
            BleSimulationEvent::RadioDetached { radio } => Some(radio),
            _ => None,
        })
        .collect();
    detached.sort();
    assert_eq!(detached, attached);
}

#[test]
fn esp32_and_apple_nodes_exchange_encrypted_requests_and_recover() {
    scenario(
        Endpoint::Esp32(Esp32Host::Esp32),
        Endpoint::CoreBluetooth(AppleHost::MacOs),
    );
}

#[test]
fn nrf52_and_bluez_nodes_exchange_encrypted_requests_and_recover() {
    scenario(
        Endpoint::Nrf52(Nrf52Host::Nrf52),
        Endpoint::BlueZ(BlueZHost::Linux),
    );
}
