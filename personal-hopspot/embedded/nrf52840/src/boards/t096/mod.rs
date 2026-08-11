#![allow(dead_code, unused_imports)]

mod profile;

pub(crate) use profile::{HardwareProfile, HARDWARE};

/// Work that must land before this module can export the same selected-board surface as T-Echo.
pub(crate) const BRING_UP_BOUNDARY: &[&str] = &[
    "confirm the installed SoftDevice/bootloader and assign application, identity, and journal flash regions",
    "extend the SX1262 seam with KCT8103L PA/LNA power and TX/RX sequencing",
    "implement the ST7735 display face and UC6580 GNSS lifecycle",
    "wire battery sampling, controls, persistence, USB descriptors, and hardware-seeded identities",
    "add the release-catalog entry only after a hardware-validated image exists",
];
