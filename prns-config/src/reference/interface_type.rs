#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum InterfaceType {
    Auto,
    TcpClient,
    TcpServer,
    Udp,
    Serial,
    Kiss,
    Ax25Kiss,
    Rnode,
    RnodeMulti,
    Pipe,
    Backbone,
    BackboneClient,
    I2p,
    Weave,
    PrnsUsbAuto,
    PrnsBluetoothAuto,
    PrnsWebSocketClient,
    PrnsWebSocketServer,
}

impl InterfaceType {
    pub(super) const CANONICAL_NAMES: &[&str] = &[
        "AutoInterface",
        "TCPClientInterface",
        "TCPServerInterface",
        "UDPInterface",
        "SerialInterface",
        "KISSInterface",
        "AX25KISSInterface",
        "RNodeInterface",
        "RNodeMultiInterface",
        "PipeInterface",
        "BackboneInterface",
        "BackboneClientInterface",
        "I2PInterface",
        "WeaveInterface",
        "PrnsUsbAuto",
        "PrnsBluetoothAuto",
        "PrnsWebSocketClient",
        "PrnsWebSocketServer",
    ];

    pub(super) fn parse(value: &str) -> Option<Self> {
        match value {
            "AutoInterface" => Some(Self::Auto),
            "TCPClientInterface" => Some(Self::TcpClient),
            "TCPServerInterface" => Some(Self::TcpServer),
            "UDPInterface" => Some(Self::Udp),
            "SerialInterface" => Some(Self::Serial),
            "KISSInterface" => Some(Self::Kiss),
            "AX25KISSInterface" => Some(Self::Ax25Kiss),
            "RNodeInterface" => Some(Self::Rnode),
            "RNodeMultiInterface" => Some(Self::RnodeMulti),
            "PipeInterface" => Some(Self::Pipe),
            "BackboneInterface" => Some(Self::Backbone),
            "BackboneClientInterface" => Some(Self::BackboneClient),
            "I2PInterface" => Some(Self::I2p),
            "WeaveInterface" => Some(Self::Weave),
            _ => Self::parse_prns(value),
        }
    }

    pub(super) const fn canonical_name(self) -> &'static str {
        match self {
            Self::Auto => "AutoInterface",
            Self::TcpClient => "TCPClientInterface",
            Self::TcpServer => "TCPServerInterface",
            Self::Udp => "UDPInterface",
            Self::Serial => "SerialInterface",
            Self::Kiss => "KISSInterface",
            Self::Ax25Kiss => "AX25KISSInterface",
            Self::Rnode => "RNodeInterface",
            Self::RnodeMulti => "RNodeMultiInterface",
            Self::Pipe => "PipeInterface",
            Self::Backbone => "BackboneInterface",
            Self::BackboneClient => "BackboneClientInterface",
            Self::I2p => "I2PInterface",
            Self::Weave => "WeaveInterface",
            Self::PrnsUsbAuto => "PrnsUsbAuto",
            Self::PrnsBluetoothAuto => "PrnsBluetoothAuto",
            Self::PrnsWebSocketClient => "PrnsWebSocketClient",
            Self::PrnsWebSocketServer => "PrnsWebSocketServer",
        }
    }

    fn parse_prns(value: &str) -> Option<Self> {
        if ["PrnsUsbAuto", "PrnsUsbAutoInterface"]
            .iter()
            .any(|candidate| value.eq_ignore_ascii_case(candidate))
        {
            return Some(Self::PrnsUsbAuto);
        }
        if [
            "PrnsBluetoothAuto",
            "PrnsBluetoothAutoInterface",
            "PrnsBleAuto",
            "PrnsBleAutoInterface",
        ]
        .iter()
        .any(|candidate| value.eq_ignore_ascii_case(candidate))
        {
            return Some(Self::PrnsBluetoothAuto);
        }
        if ["PrnsWebSocketClient", "PrnsWebSocketClientInterface"]
            .iter()
            .any(|candidate| value.eq_ignore_ascii_case(candidate))
        {
            return Some(Self::PrnsWebSocketClient);
        }
        if ["PrnsWebSocketServer", "PrnsWebSocketServerInterface"]
            .iter()
            .any(|candidate| value.eq_ignore_ascii_case(candidate))
        {
            return Some(Self::PrnsWebSocketServer);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::InterfaceType;

    #[test]
    fn prns_names_normalize_without_relaxing_stock_names() {
        for alias in [
            "PrnsUsbAuto",
            "prnsusbauto",
            "PRNSUSBAUTOINTERFACE",
            "PrnsBluetoothAuto",
            "prnsbleauto",
            "PRNSBLEAUTOINTERFACE",
            "prnswebsocketclient",
            "PRNSWEBSOCKETCLIENTINTERFACE",
            "prnswebsocketserver",
            "PRNSWEBSOCKETSERVERINTERFACE",
        ] {
            assert!(InterfaceType::parse(alias).is_some(), "{alias}");
        }
        assert_eq!(InterfaceType::parse("autointerface"), None);
    }
}
