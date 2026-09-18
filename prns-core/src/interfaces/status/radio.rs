//! Last-known radio indication for an interface or fleet member.
//!
//! Packet PHY stats stay on the packet that measured them. This type is the
//! status-layer answer: whether this wire is a radio, whether this node can
//! measure it, and the latest sample when one exists.

use crate::interfaces::{
    InterfaceKind, PacketPhyStats, RssiDbm, SignalQualityTenthsPercent, SnrQuarterDb,
};

const TAG_NOT_RADIO: u8 = 0;
const TAG_BLUETOOTH_PENDING: u8 = 1;
const TAG_BLUETOOTH_RSSI: u8 = 2;
const TAG_WIFI_UNAVAILABLE: u8 = 3;
const TAG_WIFI_PENDING: u8 = 4;
const TAG_WIFI_RSSI: u8 = 5;
const TAG_WIFI_LINK_LOCAL: u8 = 8;
const TAG_LORA_PENDING: u8 = 6;
const TAG_LORA_SAMPLE: u8 = 7;
const LORA_FLAG_SNR: u8 = 0x01;
const LORA_FLAG_QUALITY: u8 = 0x02;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RadioFamily {
    NotRadio,
    Bluetooth,
    Wifi,
    LoRa,
}

impl InterfaceKind {
    #[must_use]
    pub const fn radio_family(self) -> RadioFamily {
        match self {
            Self::BluetoothAuto | Self::BluetoothPeer => RadioFamily::Bluetooth,
            Self::LoRa | Self::Rnode => RadioFamily::LoRa,
            Self::AutoWifi
            | Self::WifiPeer
            | Self::WifiDirect
            | Self::WifiDirectPeer
            | Self::WifiAware
            | Self::WifiAwarePeer
            | Self::EspNow => RadioFamily::Wifi,
            Self::Loopback
            | Self::TcpClient
            | Self::TcpServer
            | Self::Udp
            | Self::Serial
            | Self::UsbAutoHost
            | Self::UsbAutoDevice
            | Self::LocalServer
            | Self::LocalClient
            | Self::TcpServerPeer
            | Self::Kiss
            | Self::Ax25Kiss
            | Self::Pipe
            | Self::BackboneServer
            | Self::BackboneServerPeer
            | Self::BackboneClient
            | Self::WebSocketClient
            | Self::WebSocketServer
            | Self::WebSocketServerPeer
            | Self::I2p
            | Self::I2pPeer
            | Self::Weave
            | Self::WeavePeer => RadioFamily::NotRadio,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RadioIndication {
    NotRadio,
    Bluetooth(BluetoothIndication),
    Wifi(WifiIndication),
    LoRa(LoRaIndication),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BluetoothIndication {
    Pending,
    Rssi(RssiDbm),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WifiIndication {
    /// The medium is RF, but this node has no per-peer measurement (UDP Auto Wi-Fi).
    Unavailable,
    Pending,
    Rssi(RssiDbm),
    /// Station MAC packed out of an `fe80::/64` EUI-64 address.
    ///
    /// `RadioIndication` is 10 bytes. An 8-byte interface identifier grows it,
    /// and Heltec T096 has no static RAM left for that. These boards publish
    /// `link_local_from_mac`, so the 6-byte MAC round-trips the address.
    LinkLocal([u8; 6]),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoRaIndication {
    Pending,
    Sample {
        rssi: RssiDbm,
        snr: Option<SnrQuarterDb>,
        quality: Option<SignalQualityTenthsPercent>,
    },
}

impl WifiIndication {
    /// Pack a unicast link-local only when it is `fe80::/64` plus an EUI-64
    /// interface identifier. Privacy IIDs do not fit this slot.
    #[must_use]
    pub fn from_link_local(address: core::net::Ipv6Addr) -> Option<Self> {
        let octets = address.octets();
        if octets[0] != 0xfe || octets[1] != 0x80 || octets[2..8].iter().any(|byte| *byte != 0) {
            return None;
        }
        if octets[11] != 0xff || octets[12] != 0xfe {
            return None;
        }
        Some(Self::LinkLocal([
            octets[8] ^ 0x02,
            octets[9],
            octets[10],
            octets[13],
            octets[14],
            octets[15],
        ]))
    }

    #[must_use]
    pub fn link_local(self) -> Option<core::net::Ipv6Addr> {
        let Self::LinkLocal(mac) = self else {
            return None;
        };
        Some(core::net::Ipv6Addr::new(
            0xfe80,
            0,
            0,
            0,
            (u16::from(mac[0] ^ 0x02) << 8) | u16::from(mac[1]),
            (u16::from(mac[2]) << 8) | 0x00ff,
            0xfe00 | u16::from(mac[3]),
            (u16::from(mac[4]) << 8) | u16::from(mac[5]),
        ))
    }
}

impl RadioIndication {
    pub const MAX_ENCODED_LEN: usize = 8;

    #[must_use]
    pub fn wifi_link_local(self) -> Option<core::net::Ipv6Addr> {
        match self {
            Self::Wifi(indication) => indication.link_local(),
            Self::NotRadio | Self::Bluetooth(_) | Self::LoRa(_) => None,
        }
    }

    #[must_use]
    pub const fn for_kind(kind: Option<InterfaceKind>) -> Self {
        match kind {
            Some(kind) => match kind.radio_family() {
                RadioFamily::NotRadio => Self::NotRadio,
                RadioFamily::Bluetooth => Self::Bluetooth(BluetoothIndication::Pending),
                RadioFamily::Wifi => Self::Wifi(WifiIndication::Unavailable),
                RadioFamily::LoRa => Self::LoRa(LoRaIndication::Pending),
            },
            None => Self::NotRadio,
        }
    }

    #[must_use]
    pub const fn from_bluetooth_rssi(rssi: Option<i8>) -> Self {
        match rssi {
            Some(dbm) => Self::Bluetooth(BluetoothIndication::Rssi(RssiDbm::new(dbm as i16))),
            None => Self::Bluetooth(BluetoothIndication::Pending),
        }
    }

    #[must_use]
    pub const fn from_lora_phy(phy: PacketPhyStats) -> Self {
        match phy.rssi {
            Some(rssi) => Self::LoRa(LoRaIndication::Sample {
                rssi,
                snr: phy.snr,
                quality: phy.quality,
            }),
            None => Self::LoRa(LoRaIndication::Pending),
        }
    }

    #[must_use]
    pub const fn encoded_len(self) -> usize {
        match self {
            Self::NotRadio
            | Self::Bluetooth(BluetoothIndication::Pending)
            | Self::Wifi(WifiIndication::Unavailable)
            | Self::Wifi(WifiIndication::Pending)
            | Self::LoRa(LoRaIndication::Pending) => 1,
            Self::Bluetooth(BluetoothIndication::Rssi(_)) | Self::Wifi(WifiIndication::Rssi(_)) => {
                3
            }
            Self::Wifi(WifiIndication::LinkLocal(_)) => 7,
            Self::LoRa(LoRaIndication::Sample { snr, quality, .. }) => {
                let mut len = 4usize;
                if snr.is_some() {
                    len = len.saturating_add(2);
                }
                if quality.is_some() {
                    len = len.saturating_add(2);
                }
                len
            }
        }
    }

    pub fn write_into(self, out: &mut [u8]) -> Option<&mut [u8]> {
        let (tag_out, rest) = out.split_first_mut()?;
        match self {
            Self::NotRadio => {
                *tag_out = TAG_NOT_RADIO;
                Some(rest)
            }
            Self::Bluetooth(BluetoothIndication::Pending) => {
                *tag_out = TAG_BLUETOOTH_PENDING;
                Some(rest)
            }
            Self::Bluetooth(BluetoothIndication::Rssi(rssi)) => {
                *tag_out = TAG_BLUETOOTH_RSSI;
                write_i16(rest, rssi.get())
            }
            Self::Wifi(WifiIndication::Unavailable) => {
                *tag_out = TAG_WIFI_UNAVAILABLE;
                Some(rest)
            }
            Self::Wifi(WifiIndication::Pending) => {
                *tag_out = TAG_WIFI_PENDING;
                Some(rest)
            }
            Self::Wifi(WifiIndication::Rssi(rssi)) => {
                *tag_out = TAG_WIFI_RSSI;
                write_i16(rest, rssi.get())
            }
            Self::Wifi(WifiIndication::LinkLocal(mac)) => {
                *tag_out = TAG_WIFI_LINK_LOCAL;
                let (slot, rest) = rest.split_at_mut_checked(6)?;
                slot.copy_from_slice(&mac);
                Some(rest)
            }
            Self::LoRa(LoRaIndication::Pending) => {
                *tag_out = TAG_LORA_PENDING;
                Some(rest)
            }
            Self::LoRa(LoRaIndication::Sample { rssi, snr, quality }) => {
                *tag_out = TAG_LORA_SAMPLE;
                let rest = write_i16(rest, rssi.get())?;
                let (flag_out, rest) = rest.split_first_mut()?;
                let mut flags = 0u8;
                if snr.is_some() {
                    flags |= LORA_FLAG_SNR;
                }
                if quality.is_some() {
                    flags |= LORA_FLAG_QUALITY;
                }
                *flag_out = flags;
                let rest = match snr {
                    Some(snr) => write_i16(rest, snr.quarters())?,
                    None => rest,
                };
                match quality {
                    Some(quality) => write_u16(rest, quality.tenths_percent()),
                    None => Some(rest),
                }
            }
        }
    }

    pub fn parse(input: &[u8]) -> Option<(Self, &[u8])> {
        let (tag, rest) = input.split_first()?;
        match *tag {
            TAG_NOT_RADIO => Some((Self::NotRadio, rest)),
            TAG_BLUETOOTH_PENDING => Some((Self::Bluetooth(BluetoothIndication::Pending), rest)),
            TAG_BLUETOOTH_RSSI => {
                let (rssi, rest) = parse_i16(rest)?;
                Some((
                    Self::Bluetooth(BluetoothIndication::Rssi(RssiDbm::new(rssi))),
                    rest,
                ))
            }
            TAG_WIFI_UNAVAILABLE => Some((Self::Wifi(WifiIndication::Unavailable), rest)),
            TAG_WIFI_PENDING => Some((Self::Wifi(WifiIndication::Pending), rest)),
            TAG_WIFI_RSSI => {
                let (rssi, rest) = parse_i16(rest)?;
                Some((Self::Wifi(WifiIndication::Rssi(RssiDbm::new(rssi))), rest))
            }
            TAG_WIFI_LINK_LOCAL => {
                let (mac, rest) = rest.split_at_checked(6)?;
                let mut octets = [0u8; 6];
                octets.copy_from_slice(mac);
                Some((Self::Wifi(WifiIndication::LinkLocal(octets)), rest))
            }
            TAG_LORA_PENDING => Some((Self::LoRa(LoRaIndication::Pending), rest)),
            TAG_LORA_SAMPLE => {
                let (rssi, rest) = parse_i16(rest)?;
                let (flags, rest) = rest.split_first()?;
                if *flags & !(LORA_FLAG_SNR | LORA_FLAG_QUALITY) != 0 {
                    return None;
                }
                let (snr, rest) = if *flags & LORA_FLAG_SNR != 0 {
                    let (quarters, rest) = parse_i16(rest)?;
                    (Some(SnrQuarterDb::new(quarters)), rest)
                } else {
                    (None, rest)
                };
                let (quality, rest) = if *flags & LORA_FLAG_QUALITY != 0 {
                    let (tenths, rest) = parse_u16(rest)?;
                    (Some(SignalQualityTenthsPercent::new(tenths)?), rest)
                } else {
                    (None, rest)
                };
                Some((
                    Self::LoRa(LoRaIndication::Sample {
                        rssi: RssiDbm::new(rssi),
                        snr,
                        quality,
                    }),
                    rest,
                ))
            }
            _ => None,
        }
    }
}

fn write_i16(out: &mut [u8], value: i16) -> Option<&mut [u8]> {
    let (slot, rest) = out.split_at_mut_checked(2)?;
    slot.copy_from_slice(&value.to_be_bytes());
    Some(rest)
}

fn write_u16(out: &mut [u8], value: u16) -> Option<&mut [u8]> {
    let (slot, rest) = out.split_at_mut_checked(2)?;
    slot.copy_from_slice(&value.to_be_bytes());
    Some(rest)
}

fn parse_i16(input: &[u8]) -> Option<(i16, &[u8])> {
    let (slot, rest) = input.split_at_checked(2)?;
    Some((i16::from_be_bytes(slot.try_into().ok()?), rest))
}

fn parse_u16(input: &[u8]) -> Option<(u16, &[u8])> {
    let (slot, rest) = input.split_at_checked(2)?;
    Some((u16::from_be_bytes(slot.try_into().ok()?), rest))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn radio_indication_layout() {
        assert_eq!(core::mem::size_of::<RadioIndication>(), 10);
    }

    #[test]
    fn kind_selects_the_family_without_inventing_a_sample() {
        assert_eq!(
            RadioIndication::for_kind(Some(InterfaceKind::TcpClient)),
            RadioIndication::NotRadio
        );
        assert_eq!(
            RadioIndication::for_kind(Some(InterfaceKind::BluetoothPeer)),
            RadioIndication::Bluetooth(BluetoothIndication::Pending)
        );
        assert_eq!(
            RadioIndication::for_kind(Some(InterfaceKind::WifiPeer)),
            RadioIndication::Wifi(WifiIndication::Unavailable)
        );
        assert_eq!(
            RadioIndication::for_kind(Some(InterfaceKind::LoRa)),
            RadioIndication::LoRa(LoRaIndication::Pending)
        );
    }

    #[test]
    fn lora_samples_reject_unknown_flag_bits() {
        let bytes = [TAG_LORA_SAMPLE, 0, 0, 0x80];
        assert_eq!(RadioIndication::parse(&bytes), None);
    }

    #[test]
    fn bluetooth_rssi_and_lora_samples_round_trip_on_the_wire() {
        for indication in [
            RadioIndication::NotRadio,
            RadioIndication::Bluetooth(BluetoothIndication::Pending),
            RadioIndication::from_bluetooth_rssi(Some(-67)),
            RadioIndication::Wifi(WifiIndication::Unavailable),
            RadioIndication::Wifi(WifiIndication::Rssi(RssiDbm::new(-51))),
            RadioIndication::from_lora_phy(PacketPhyStats {
                rssi: Some(RssiDbm::new(-103)),
                snr: Some(SnrQuarterDb::new(-11)),
                quality: SignalQualityTenthsPercent::new(812),
            }),
            RadioIndication::from_lora_phy(PacketPhyStats {
                rssi: Some(RssiDbm::new(-90)),
                snr: None,
                quality: None,
            }),
        ] {
            let mut bytes = [0u8; RadioIndication::MAX_ENCODED_LEN];
            let rest = indication.write_into(&mut bytes).expect("fits");
            let written = RadioIndication::MAX_ENCODED_LEN - rest.len();
            assert_eq!(written, indication.encoded_len());
            assert_eq!(
                RadioIndication::parse(&bytes[..written]),
                Some((indication, [].as_slice()))
            );
        }
    }

    #[test]
    fn eui64_link_local_round_trips_through_the_radio_slot() {
        let address = crate::interfaces::wifi_auto::link_local_from_mac(
            crate::interfaces::MacAddress::new([0x18, 0x62, 0xb4, 0x11, 0x22, 0x33]),
        );
        let indication =
            RadioIndication::Wifi(WifiIndication::from_link_local(address).expect("eui-64 packs"));
        let mut slot = [0u8; RadioIndication::MAX_ENCODED_LEN];
        indication.write_into(&mut slot).expect("fits");
        let (parsed, padding) = RadioIndication::parse(&slot).expect("slot parses");
        assert!(padding.iter().all(|byte| *byte == 0));
        assert_eq!(parsed.wifi_link_local(), Some(address));
    }

    #[test]
    fn privacy_iid_does_not_fit_the_radio_slot() {
        let address = core::net::Ipv6Addr::new(0xfe80, 0, 0, 0, 0x1111, 0x2222, 0x3333, 0x4444);
        assert_eq!(WifiIndication::from_link_local(address), None);
    }
}
