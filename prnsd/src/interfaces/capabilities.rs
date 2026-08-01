use prns_config::InterfaceKind;

pub(super) fn available_kinds() -> impl Iterator<Item = InterfaceKind> {
    InterfaceKind::CANONICAL_NAMES
        .iter()
        .filter_map(|canonical| InterfaceKind::parse(canonical))
        .filter(|kind| available(*kind))
}

pub(super) const fn available(kind: InterfaceKind) -> bool {
    match kind {
        InterfaceKind::Auto
        | InterfaceKind::Serial
        | InterfaceKind::Kiss
        | InterfaceKind::Ax25Kiss
        | InterfaceKind::Rnode
        | InterfaceKind::RnodeMulti
        | InterfaceKind::Weave
        | InterfaceKind::PrnsUsbAuto
        | InterfaceKind::PrnsBluetoothAuto => cfg!(feature = "tokio-host"),
        InterfaceKind::TcpClient
        | InterfaceKind::TcpServer
        | InterfaceKind::Udp
        | InterfaceKind::Pipe
        | InterfaceKind::Backbone
        | InterfaceKind::BackboneClient
        | InterfaceKind::I2p
        | InterfaceKind::PrnsWebSocketClient
        | InterfaceKind::PrnsWebSocketServer => true,
    }
}

#[cfg(test)]
mod tests {
    use prns_config::InterfaceKind;

    use super::{available, available_kinds};

    #[test]
    fn availability_matches_the_binary_feature_set() {
        assert_eq!(available(InterfaceKind::Auto), cfg!(feature = "tokio-host"));
        assert_eq!(
            available(InterfaceKind::PrnsUsbAuto),
            cfg!(feature = "tokio-host")
        );
        assert!(available(InterfaceKind::Backbone));
        assert!(available(InterfaceKind::TcpServer));
    }

    #[test]
    fn the_prompt_catalog_contains_only_available_types() {
        let kinds = available_kinds().collect::<Vec<_>>();

        assert!(kinds.iter().all(|kind| available(*kind)));
        assert!(kinds.contains(&InterfaceKind::Backbone));
        assert_eq!(
            kinds.contains(&InterfaceKind::Auto),
            cfg!(feature = "tokio-host")
        );
    }
}
