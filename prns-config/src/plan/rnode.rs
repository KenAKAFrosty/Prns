use super::error::PlanErrorKind;
use crate::reference::keys::interface as interface_key;
use crate::reference::keys::rnode as rnode_key;

pub const RNODE_TCP_PORT: u16 = 7_633;

const RNODE_BLE_ADDRESS_LEN: usize = 17;

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
pub struct RNodeBleAddress([u8; 6]);

impl RNodeBleAddress {
    fn parse(value: &str) -> Result<Self, RNodeBleTargetError> {
        let mut octets = [0; 6];
        let mut parts = value.split(':');
        for octet in &mut octets {
            let Some(part) = parts.next() else {
                return Err(RNodeBleTargetError::InvalidAddress);
            };
            *octet =
                u8::from_str_radix(part, 16).map_err(|_| RNodeBleTargetError::InvalidAddress)?;
        }
        if parts.next().is_some() {
            return Err(RNodeBleTargetError::InvalidAddress);
        }
        Ok(Self(octets))
    }

    #[must_use]
    pub const fn octets(&self) -> [u8; 6] {
        self.0
    }
}

impl core::fmt::Display for RNodeBleAddress {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let [a, b, c, d, e, f] = self.0;
        write!(formatter, "{a:02X}:{b:02X}:{c:02X}:{d:02X}:{e:02X}:{f:02X}")
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RNodeBleName(String);

impl RNodeBleName {
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RNodeBleTarget {
    FirstBondedRnode,
    Address(RNodeBleAddress),
    Name(RNodeBleName),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RNodeBleTargetError {
    InvalidAddress,
}

impl RNodeBleTarget {
    pub(crate) fn from_uri_suffix(value: String) -> Result<Self, RNodeBleTargetError> {
        if value.is_empty() {
            return Ok(Self::FirstBondedRnode);
        }
        if looks_like_ble_address(&value) {
            return RNodeBleAddress::parse(&value).map(Self::Address);
        }
        Ok(Self::Name(RNodeBleName(value)))
    }
}

impl core::fmt::Display for RNodeBleTarget {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::FirstBondedRnode => formatter.write_str("the first bonded `RNode ` device"),
            Self::Address(address) => write!(formatter, "BLE address {address}"),
            Self::Name(name) => write!(formatter, "BLE device named {:?}", name.as_str()),
        }
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
    Ble(RNodeBleTarget),
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
        if port
            .as_bytes()
            .get(..rnode_key::BLE_SCHEME.len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(rnode_key::BLE_SCHEME.as_bytes()))
        {
            let target = port.split_off(rnode_key::BLE_SCHEME.len());
            return RNodeBleTarget::from_uri_suffix(target)
                .map(Self::Ble)
                .map_err(|_| invalid_rnode_port());
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
            Self::Ble(RNodeBleTarget::FirstBondedRnode) => {
                rnode_key::BLE_SCHEME.as_bytes().to_vec()
            }
            Self::Ble(RNodeBleTarget::Address(address)) => {
                format!("{}{address}", rnode_key::BLE_SCHEME).into_bytes()
            }
            Self::Ble(RNodeBleTarget::Name(name)) => {
                let mut tag = rnode_key::BLE_SCHEME.as_bytes().to_vec();
                tag.extend_from_slice(name.as_str().as_bytes());
                tag
            }
        }
    }
}

fn looks_like_ble_address(value: &str) -> bool {
    value.len() == RNODE_BLE_ADDRESS_LEN && value.split(':').count() == 6
}

const fn invalid_rnode_port() -> PlanErrorKind {
    PlanErrorKind::InvalidSetting {
        key: interface_key::PORT,
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

    #[test]
    fn ble_targets_preserve_the_three_stock_selection_modes() {
        let automatic = RNodeTransportPlan::from_configured_port("ble://".to_string())
            .expect("stock automatic BLE URI");
        assert_eq!(
            automatic,
            RNodeTransportPlan::Ble(RNodeBleTarget::FirstBondedRnode)
        );

        let named = RNodeTransportPlan::from_configured_port("BLE://RNode 1234".to_string())
            .expect("case-insensitive named BLE URI");
        assert_eq!(
            named,
            RNodeTransportPlan::Ble(RNodeBleTarget::Name(RNodeBleName("RNode 1234".to_string())))
        );

        let addressed =
            RNodeTransportPlan::from_configured_port("ble://aa:bb:cc:dd:ee:ff".to_string())
                .expect("addressed BLE URI");
        assert_eq!(
            addressed,
            RNodeTransportPlan::Ble(RNodeBleTarget::Address(RNodeBleAddress([
                0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF,
            ])))
        );
        assert_eq!(addressed.channel_tag(), b"ble://AA:BB:CC:DD:EE:FF");
    }

    #[test]
    fn address_shaped_ble_targets_must_be_hexadecimal() {
        let error = RNodeTransportPlan::from_configured_port("ble://GG:BB:CC:DD:EE:FF".to_string())
            .expect_err("malformed BLE address");
        assert_eq!(
            error,
            PlanErrorKind::InvalidSetting {
                key: interface_key::PORT
            }
        );
    }
}
