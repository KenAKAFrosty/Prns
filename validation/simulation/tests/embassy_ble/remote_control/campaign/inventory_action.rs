use super::super::tests::Pair;
use super::*;
use personal_rns::{
    engine::SendRequestFailure,
    interfaces::{ConnectionState, InterfaceKind},
    runtime::SendError,
};

pub(super) fn fetch(
    pair: &mut Pair<'_, '_>,
) -> Result<RemoteControlResponse, SendError<SendRequestFailure>> {
    let reply = pair.exchange(RemoteControlRequest::InventoryInterfaces {
        page: RemoteControlInterfacePage::First,
    });
    if let Ok(RemoteControlResponse::InventoryInterfaces(snapshot)) = &reply {
        assert_eq!(snapshot.entries().len(), 1);
        let entry = &snapshot.entries()[0];
        assert_eq!(entry.kind, InterfaceKind::BluetoothAuto);
        assert_eq!(entry.connection, ConnectionState::Connected);
        let id = entry.id;
        assert!(matches!(
            pair.exchange(RemoteControlRequest::InventoryInterfaceConfig { id }),
            Ok(RemoteControlResponse::InventoryInterfaceConfig(
                RemoteControlInterfaceConfigOutcome::Card(_)
            ))
        ));
        let Ok(RemoteControlResponse::InventoryInterfacePeers(
            RemoteControlInterfacePeersOutcome::Page(peers),
        )) = pair.exchange(RemoteControlRequest::InventoryInterfacePeers {
            id,
            page: RemoteControlPeerPage::First,
        })
        else {
            unreachable!("peer inventory")
        };
        assert_eq!(peers.peers.len(), 1);
        assert_eq!(peers.peers[0].connection, ConnectionState::Connected);
        assert_eq!(peers.peers[0].rate_bytes_per_sec, None);
    }
    reply
}
