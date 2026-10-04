use super::DeviceLabel;
use prns_core::identity::IdentityPublicKeys;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RememberedDevice {
    pub label: DeviceLabel,
    pub pairing: RememberedPairing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RememberedPairing {
    Unpaired,
    Paired { target: IdentityPublicKeys },
}
