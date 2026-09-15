//! Short family-specific medium/path fact for a peer or interface.
//!
//! Orthogonal to connection Status, RNS Health, and `RadioIndication` (RSSI).
//! Operator UI labels this row "Details". BLE values name the data plane
//! (GATT floor vs CoC), not an RF advertising channel.

const TAG_NOT_APPLICABLE: u8 = 0;
const TAG_UNKNOWN: u8 = 1;
const TAG_BLE_GATT: u8 = 2;
const TAG_BLE_COC: u8 = 3;
const TAG_WIFI_RF_CHANNEL: u8 = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerDetails {
    /// No Details row for this interface kind.
    NotApplicable,
    /// Kind can report Details, but the value is not settled yet.
    Unknown,
    /// BLE ACL data plane is the GATT floor.
    BleGatt,
    /// BLE ACL data plane is L2CAP Connection-Oriented Channels.
    BleCoc,
    /// 802.11 RF channel number when known.
    WifiRfChannel(u8),
}

/// Host-side callback that publishes peer Details as the BLE data plane settles.
#[cfg(feature = "tokio-host")]
#[derive(Clone)]
pub struct PeerDetailsNotify {
    inner: std::sync::Arc<dyn Fn(PeerDetails) + Send + Sync>,
}

#[cfg(feature = "tokio-host")]
impl PeerDetailsNotify {
    #[must_use]
    pub fn new(notify: impl Fn(PeerDetails) + Send + Sync + 'static) -> Self {
        Self {
            inner: std::sync::Arc::new(notify),
        }
    }

    pub fn publish(&self, details: PeerDetails) {
        (self.inner)(details);
    }
}

impl PeerDetails {
    pub const ENCODED_LEN: usize = 2;

    /// Fixed labels for non-Wi-Fi variants. Prefer [`Self::fmt_label`] when Wi-Fi may appear.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::NotApplicable => "",
            Self::Unknown => "Unknown",
            Self::BleGatt => "GATT",
            Self::BleCoc => "CoC",
            Self::WifiRfChannel(_) => "ch",
        }
    }

    pub fn fmt_label(self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::WifiRfChannel(channel) => write!(f, "ch {channel}"),
            other => f.write_str(other.label()),
        }
    }

    #[must_use]
    pub const fn is_applicable(self) -> bool {
        !matches!(self, Self::NotApplicable)
    }

    #[must_use]
    pub const fn wire_tag(self) -> u8 {
        match self {
            Self::NotApplicable => TAG_NOT_APPLICABLE,
            Self::Unknown => TAG_UNKNOWN,
            Self::BleGatt => TAG_BLE_GATT,
            Self::BleCoc => TAG_BLE_COC,
            Self::WifiRfChannel(_) => TAG_WIFI_RF_CHANNEL,
        }
    }

    #[must_use]
    pub const fn wire_payload(self) -> u8 {
        match self {
            Self::WifiRfChannel(channel) => channel,
            Self::NotApplicable | Self::Unknown | Self::BleGatt | Self::BleCoc => 0,
        }
    }

    #[must_use]
    pub const fn from_wire(tag: u8, payload: u8) -> Option<Self> {
        match tag {
            TAG_NOT_APPLICABLE => Some(Self::NotApplicable),
            TAG_UNKNOWN => Some(Self::Unknown),
            TAG_BLE_GATT => Some(Self::BleGatt),
            TAG_BLE_COC => Some(Self::BleCoc),
            TAG_WIFI_RF_CHANNEL => Some(Self::WifiRfChannel(payload)),
            _ => None,
        }
    }

    pub fn write_into(self, out: &mut [u8]) -> Option<&mut [u8]> {
        let (tag_out, rest) = out.split_first_mut()?;
        let (payload_out, rest) = rest.split_first_mut()?;
        *tag_out = self.wire_tag();
        *payload_out = self.wire_payload();
        Some(rest)
    }

    #[must_use]
    pub fn parse(input: &[u8]) -> Option<(Self, &[u8])> {
        let (tag, rest) = input.split_first()?;
        let (payload, rest) = rest.split_first()?;
        Some((Self::from_wire(*tag, *payload)?, rest))
    }
}

impl core::fmt::Display for PeerDetails {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        self.fmt_label(f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ble_and_wifi_round_trip_on_the_wire() {
        for details in [
            PeerDetails::NotApplicable,
            PeerDetails::Unknown,
            PeerDetails::BleGatt,
            PeerDetails::BleCoc,
            PeerDetails::WifiRfChannel(6),
            PeerDetails::WifiRfChannel(149),
        ] {
            let mut buf = [0u8; PeerDetails::ENCODED_LEN];
            let _ = details.write_into(&mut buf).expect("encode");
            let (parsed, rest) = PeerDetails::parse(&buf).expect("parse");
            assert_eq!(parsed, details);
            assert!(rest.is_empty());
        }
    }

    #[test]
    fn labels_name_ble_data_planes() {
        assert_eq!(PeerDetails::BleGatt.label(), "GATT");
        assert_eq!(PeerDetails::BleCoc.label(), "CoC");
        let mut buf = heapless::String::<8>::new();
        let _ = core::fmt::Write::write_fmt(
            &mut buf,
            format_args!("{}", PeerDetails::WifiRfChannel(100)),
        );
        assert_eq!(buf.as_str(), "ch 100");
    }
}
