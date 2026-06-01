//! Link-layer addressing shared across the L2 interface impls (the WiFi
//! auto-interface, ESP-NOW).

/// A 48-bit IEEE 802 MAC address (EUI-48), as a host reads it off its network
/// hardware.
#[derive(Clone, Copy)]
pub struct MacAddress([u8; 6]);

impl MacAddress {
    pub const fn new(octets: [u8; 6]) -> Self {
        Self(octets)
    }

    pub const fn octets(&self) -> [u8; 6] {
        self.0
    }
}
