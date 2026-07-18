use super::error::PlanErrorKind;
use crate::reference::keys::interface as interface_key;
use crate::reference::keys::rnode as rnode_key;

pub const RNODE_TCP_PORT: u16 = 7_633;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RNodeSerialDevice(String);

impl RNodeSerialDevice {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RNodeTcpHost(String);

impl RNodeTcpHost {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RNodeTcpTarget {
    Loopback,
    Host(RNodeTcpHost),
}

impl RNodeTcpTarget {
    #[must_use]
    pub fn socket_target(&self) -> String {
        let host = match self {
            Self::Loopback => "localhost",
            Self::Host(host) => host.as_str(),
        };
        if host.contains(':') && !host.starts_with('[') {
            format!("[{host}]:{RNODE_TCP_PORT}")
        } else {
            format!("{host}:{RNODE_TCP_PORT}")
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RNodeTransportPlan {
    Serial(RNodeSerialDevice),
    Tcp(RNodeTcpTarget),
}

impl RNodeTransportPlan {
    pub(super) fn from_configured_port(mut port: String) -> Result<Self, PlanErrorKind> {
        if port.is_empty() {
            return Err(PlanErrorKind::InvalidSetting {
                key: interface_key::PORT,
            });
        }
        if port
            .as_bytes()
            .get(..rnode_key::TCP_SCHEME.len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(rnode_key::TCP_SCHEME.as_bytes()))
        {
            let host = port.split_off(rnode_key::TCP_SCHEME.len());
            return Ok(Self::Tcp(if host.is_empty() {
                RNodeTcpTarget::Loopback
            } else {
                RNodeTcpTarget::Host(RNodeTcpHost(host))
            }));
        }
        Ok(Self::Serial(RNodeSerialDevice(port)))
    }

    #[must_use]
    pub fn channel_tag(&self) -> Vec<u8> {
        match self {
            Self::Serial(device) => device.as_str().as_bytes().to_vec(),
            Self::Tcp(RNodeTcpTarget::Loopback) => rnode_key::TCP_SCHEME.as_bytes().to_vec(),
            Self::Tcp(RNodeTcpTarget::Host(host)) => {
                let mut tag = rnode_key::TCP_SCHEME.as_bytes().to_vec();
                tag.extend_from_slice(host.as_str().as_bytes());
                tag
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tcp_targets_have_a_fixed_stock_port_and_typed_loopback() {
        let loopback = RNodeTransportPlan::from_configured_port("tcp://".to_string())
            .expect("stock loopback URI");
        assert_eq!(loopback, RNodeTransportPlan::Tcp(RNodeTcpTarget::Loopback));
        assert_eq!(RNodeTcpTarget::Loopback.socket_target(), "localhost:7633");

        let ipv6 = RNodeTransportPlan::from_configured_port("TCP://::1".to_string())
            .expect("case-insensitive stock URI");
        let RNodeTransportPlan::Tcp(target) = ipv6 else {
            panic!("TCP transport expected")
        };
        assert_eq!(target.socket_target(), "[::1]:7633");
        assert_eq!(RNodeTransportPlan::Tcp(target).channel_tag(), b"tcp://::1");
    }

    #[test]
    fn a_serial_device_cannot_be_confused_with_a_tcp_target() {
        let transport = RNodeTransportPlan::from_configured_port("/dev/ttyUSB0".to_string())
            .expect("serial device");
        assert_eq!(
            transport,
            RNodeTransportPlan::Serial(RNodeSerialDevice("/dev/ttyUSB0".to_string()))
        );
    }
}
