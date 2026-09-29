use super::keys::interface as interface_key;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterfaceKind {
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

impl InterfaceKind {
    pub const CANONICAL_NAMES: &[&str] = &[
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

    pub fn parse(value: &str) -> Option<Self> {
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

    pub const fn canonical_name(self) -> &'static str {
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

    pub const fn cli_name(self) -> &'static str {
        match self {
            Self::Auto => "auto-wifi",
            Self::TcpClient => "tcp-client",
            Self::TcpServer => "tcp-server",
            Self::Udp => "udp",
            Self::Serial => "serial",
            Self::Kiss => "kiss",
            Self::Ax25Kiss => "ax25-kiss",
            Self::Rnode => "rnode",
            Self::RnodeMulti => "rnode-multi",
            Self::Pipe => "pipe",
            Self::Backbone => "backbone-server",
            Self::BackboneClient => "backbone-client",
            Self::I2p => "i2p",
            Self::Weave => "weave",
            Self::PrnsUsbAuto => "usb-auto",
            Self::PrnsBluetoothAuto => "bluetooth-auto",
            Self::PrnsWebSocketClient => "websocket-client",
            Self::PrnsWebSocketServer => "websocket-server",
        }
    }

    pub fn parse_cli(value: &str) -> Option<Self> {
        let normalized = value.trim().to_ascii_lowercase();
        match normalized.as_str() {
            "auto" | "auto-wifi" | "wifi-auto" => Some(Self::Auto),
            "tcp-client" => Some(Self::TcpClient),
            "tcp-server" => Some(Self::TcpServer),
            "udp" => Some(Self::Udp),
            "serial" => Some(Self::Serial),
            "kiss" => Some(Self::Kiss),
            "ax25-kiss" | "ax25" => Some(Self::Ax25Kiss),
            "rnode" => Some(Self::Rnode),
            "rnode-multi" => Some(Self::RnodeMulti),
            "pipe" => Some(Self::Pipe),
            "backbone" | "backbone-server" => Some(Self::Backbone),
            "backbone-client" => Some(Self::BackboneClient),
            "i2p" => Some(Self::I2p),
            "weave" => Some(Self::Weave),
            "usb" | "usb-auto" => Some(Self::PrnsUsbAuto),
            "bluetooth" | "bluetooth-auto" | "ble" | "ble-auto" => Some(Self::PrnsBluetoothAuto),
            "websocket-client" | "ws-client" => Some(Self::PrnsWebSocketClient),
            "websocket-server" | "ws-server" => Some(Self::PrnsWebSocketServer),
            _ => Self::parse(value),
        }
    }

    pub fn accepts_setting(self, key: &str) -> bool {
        if interface_key::COMMON.contains(&key) {
            return true;
        }
        let medium = match self {
            Self::Auto => interface_key::AUTO,
            Self::TcpClient => interface_key::TCP_CLIENT,
            Self::TcpServer => interface_key::TCP_SERVER,
            Self::Udp => interface_key::UDP,
            Self::Serial => interface_key::SERIAL,
            Self::Kiss => interface_key::KISS,
            Self::Ax25Kiss => interface_key::AX25_KISS,
            Self::Rnode => interface_key::RNODE,
            Self::RnodeMulti => interface_key::RNODE_MULTI,
            Self::Pipe => interface_key::PIPE,
            Self::Backbone | Self::BackboneClient => interface_key::BACKBONE,
            Self::I2p => interface_key::I2P,
            Self::Weave => interface_key::WEAVE,
            Self::PrnsUsbAuto => &[],
            Self::PrnsBluetoothAuto => interface_key::PRNS_BLUETOOTH_AUTO,
            Self::PrnsWebSocketClient => interface_key::PRNS_WEBSOCKET_CLIENT,
            Self::PrnsWebSocketServer => interface_key::PRNS_WEBSOCKET_SERVER,
        };
        medium.contains(&key)
    }
}

pub(super) use InterfaceKind as InterfaceType;

#[cfg(test)]
mod tests {
    use super::InterfaceKind;

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
            assert!(InterfaceKind::parse(alias).is_some(), "{alias}");
        }
        assert_eq!(InterfaceKind::parse("autointerface"), None);
    }
}
