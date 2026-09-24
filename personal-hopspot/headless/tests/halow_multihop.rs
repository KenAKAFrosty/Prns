#![cfg(feature = "wifi-halow")]

use std::io;
use std::num::{NonZeroU32, NonZeroU8};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use personal_hopspot_core::{node_pages, HopspotDestinationSet};
use personal_rns::interfaces::wifi_halow::{self as wire, InstanceTag, PeerMac};
use personal_rns::interfaces::{BitrateBps, InterfaceKind, MacAddress};
use personal_rns::prelude::*;
use personal_rns::routing::links::request::{
    write_packed_binary_header, MAX_PACKED_BINARY_HEADER_LEN,
};
use personal_rns::routing::request_handlers::RequestPathHash;
use personal_rns::runtime::PrnsNodeHandle;
use personal_rns::storage::GrowableHeap;
use personal_rns::wifi_halow::{Destination, HaLow, HaLowDatagrams, HaLowLimits, ReceivedDatagram};
use personal_rns::wire::{PacketType, WireContext, WirePacketHeader};
use tokio::sync::mpsc;

const LEFT: usize = 0;
const RELAY: usize = 1;
const RIGHT: usize = 2;
const NODE_COUNT: usize = 3;

fn mac(station: usize) -> PeerMac {
    PeerMac::new(MacAddress::new([2, 0, 0, 0, 0, station as u8 + 1])).unwrap()
}

struct Transmission {
    source: usize,
    destination: Destination,
    datagram: Vec<u8>,
}

#[derive(Clone, Copy)]
enum RelayConnection {
    Connected,
    Disconnected,
}

struct Medium {
    receivers: [mpsc::Sender<(PeerMac, Vec<u8>)>; NODE_COUNT],
    relay: Mutex<RelayConnection>,
    transmissions: Mutex<Vec<Transmission>>,
}

struct Radio {
    station: usize,
    medium: Arc<Medium>,
    incoming: tokio::sync::Mutex<mpsc::Receiver<(PeerMac, Vec<u8>)>>,
}

impl HaLowDatagrams for Radio {
    async fn send(&self, destination: Destination, payload: &[u8]) -> io::Result<()> {
        self.medium
            .transmissions
            .lock()
            .unwrap()
            .push(Transmission {
                source: self.station,
                destination,
                datagram: payload.to_vec(),
            });
        if matches!(
            *self.medium.relay.lock().unwrap(),
            RelayConnection::Disconnected
        ) {
            return Ok(());
        }
        for station in 0..NODE_COUNT {
            // The medium has only A–B and B–C edges, even for group frames.
            if station == self.station || (station != RELAY && self.station != RELAY) {
                continue;
            }
            if matches!(destination, Destination::Peer(peer) if peer != mac(station)) {
                continue;
            }
            self.medium.receivers[station]
                .try_send((mac(self.station), payload.to_vec()))
                .expect("test medium must not silently drop or hide queue saturation");
        }
        Ok(())
    }

    async fn receive(&self, buffer: &mut [u8]) -> io::Result<ReceivedDatagram> {
        let (source, bytes) = self
            .incoming
            .lock()
            .await
            .recv()
            .await
            .ok_or(io::ErrorKind::UnexpectedEof)?;
        assert!(bytes.len() <= buffer.len());
        buffer[..bytes.len()].copy_from_slice(&bytes);
        Ok(ReceivedDatagram {
            source,
            length: bytes.len(),
        })
    }
}

async fn announce(handle: &PrnsNodeHandle, destination: DestinationHash) {
    handle
        .announce_now(AnnounceNow {
            destination,
            target: AnnounceTarget::AllInterfaces,
            app_data: AnnounceAppData::Registered,
        })
        .await
        .unwrap();
}

async fn page(handle: &PrnsNodeHandle, destination: DestinationHash) {
    handle.request_path(destination).await.unwrap();
    let link = handle.establish_link(destination).await.unwrap();
    let (page, _) = handle
        .request(link, RequestPathHash::of(node_pages::INDEX_PATH), &[])
        .await
        .unwrap();
    let mut header = [0; MAX_PACKED_BINARY_HEADER_LEN];
    let length =
        write_packed_binary_header(node_pages::HOPSPOT_INDEX_PAGE.len(), &mut header).unwrap();
    let expected: Vec<_> = header[..length]
        .iter()
        .chain(node_pages::HOPSPOT_INDEX_PAGE)
        .copied()
        .collect();
    assert_eq!(
        page, expected,
        "complete Resource response must survive both hops"
    );
}

#[tokio::test]
async fn hopspot_page_crosses_two_halow_hops_and_requires_the_relay() {
    let channels = [(); NODE_COUNT].map(|_| mpsc::channel(256));
    let medium = Arc::new(Medium {
        receivers: channels.each_ref().map(|(sender, _)| sender.clone()),
        relay: Mutex::new(RelayConnection::Disconnected),
        transmissions: Mutex::new(Vec::new()),
    });
    let nodes = channels
        .into_iter()
        .enumerate()
        .map(|(station, (_, incoming))| {
            let identity = Zeroizing::new([station as u8 + 11; 64]);
            let destinations = HopspotDestinationSet::new(identity.clone(), b"test", b"test");
            let address = destinations.destination_hashes().unwrap().node_page;
            let node = PrnsNode::new(PrnsNodeRecipe {
                transport_identity: Some(identity),
                pre_configured_destinations: destinations.into_preconfigured_destinations(),
                app_state: personal_rns::runtime::NoRemoteControlHostControls,
                storage: GrowableHeap,
                request_endpoints: node_pages::NodePageRoutes,
                remote_control: personal_rns::remote_control::RemoteControlService::Unavailable
                    .into(),
                interfaces: ManuallyAttached,
                persistence: NoPersistence,
                on_event: |_, _: &personal_rns::runtime::NoRemoteControlHostControls| {},
            });
            let handle = node.handle();
            let policy = wire::policy_for_bitrate(BitrateBps::guess(7_300_000));
            handle.supervise(HaLow::new(
                Radio {
                    station,
                    medium: medium.clone(),
                    incoming: tokio::sync::Mutex::new(incoming),
                },
                InstanceTag::new(b"qualification").unwrap(),
                policy,
                wire::policy_for_bitrate(BitrateBps::guess(4_000_000)),
                HaLowLimits {
                    peers: NonZeroU8::new(4).unwrap(),
                    idle_seconds: NonZeroU32::new(300).unwrap(),
                },
            ));
            (node, handle, address)
        })
        .collect::<Vec<_>>();
    let mut nodes = nodes.into_iter();
    let (left, left_handle, left_address) = nodes.next().unwrap();
    let (relay, relay_handle, _) = nodes.next().unwrap();
    let (right, right_handle, right_address) = nodes.next().unwrap();
    let exercise = async {
        for handle in [&left_handle, &relay_handle, &right_handle] {
            while !handle
                .interfaces()
                .iter()
                .any(|row| row.id.kind() == Some(InterfaceKind::WifiHaLowBroadcast))
            {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }
        announce(&right_handle, right_address).await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(left_handle
            .interfaces()
            .iter()
            .all(|row| row.id.kind() != Some(InterfaceKind::WifiHaLowPeer)));
        assert!(
            tokio::time::timeout(
                Duration::from_millis(300),
                left_handle.request_path(right_address)
            )
            .await
            .is_err(),
            "isolated endpoints must not discover one another"
        );

        *medium.relay.lock().unwrap() = RelayConnection::Connected;
        announce(&right_handle, right_address).await;
        announce(&left_handle, left_address).await;
        tokio::join!(
            page(&left_handle, right_address),
            page(&right_handle, left_address)
        );
        let scope = InstanceTag::new(b"qualification").unwrap();
        for handle in [&left_handle, &right_handle] {
            let peers: Vec<_> = handle
                .interfaces()
                .into_iter()
                .filter(|row| row.id.kind() == Some(InterfaceKind::WifiHaLowPeer))
                .map(|row| row.id)
                .collect();
            assert_eq!(
                peers,
                vec![scope.peer_id(mac(RELAY))],
                "identity is the immediate sender, not the remote destination"
            );
        }
        {
            let transmissions = medium.transmissions.lock().unwrap();
            let mut relayed_announces = 0;
            let mut relay_unicast = [0; NODE_COUNT];
            for sent in transmissions.iter() {
                let frame = wire::decode(&sent.datagram).unwrap();
                let (header, _) = WirePacketHeader::parse(frame).unwrap();
                if sent.source == RELAY
                    && header.packet_type == PacketType::Announce
                    && header.hops > 0
                    && header.context == WireContext::None
                {
                    assert_eq!(sent.destination, Destination::Broadcast);
                    relayed_announces += 1;
                }
                if header.packet_type != PacketType::Announce {
                    assert!(
                        matches!(sent.destination, Destination::Peer(_)),
                        "non-announce traffic must stay unicast"
                    );
                    if sent.source == RELAY {
                        for endpoint in [LEFT, RIGHT] {
                            if sent.destination == Destination::Peer(mac(endpoint)) {
                                relay_unicast[endpoint] += 1;
                            }
                        }
                    }
                }
            }
            assert!(relayed_announces > 0);
            assert!(relay_unicast[LEFT] > 0 && relay_unicast[RIGHT] > 0);
        }
        *medium.relay.lock().unwrap() = RelayConnection::Disconnected;
        assert!(
            tokio::time::timeout(Duration::from_secs(1), page(&left_handle, right_address))
                .await
                .is_err(),
            "learned routes cannot bypass a disconnected relay"
        );
    };
    tokio::select! {
        result = tokio::time::timeout(Duration::from_secs(20), exercise) => result.expect("two-hop qualification timed out"),
        result = left.run() => panic!("left stopped: {result:?}"),
        result = relay.run() => panic!("relay stopped: {result:?}"),
        result = right.run() => panic!("right stopped: {result:?}"),
    }
}
