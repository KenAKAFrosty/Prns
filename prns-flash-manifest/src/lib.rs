//! Shared release contract for the Personal Hopspot web and CLI flashers.

mod catalog;
mod manifest;
mod provisioning;
mod trust;

pub use catalog::{
    board_catalog, BoardBuild, BoardCatalog, BoardCatalogEntry, CatalogError, EspBuild,
    ProvisioningDescriptor, Transport, Uf2Build,
};
pub use manifest::{
    ChannelDescriptor, FlashManifest, FlashPart, FlashPartKind, ManifestError, ReleaseChannel,
    ReleaseInfo, SigningInfo, TargetManifest,
};
pub use provisioning::{
    provisioning_image, ProvisioningAction, ProvisioningError, WifiCredentials, CONFIG_MAGIC,
    CONFIG_OFFSET, CONFIG_PASSWORD_MAX_BYTES, CONFIG_SIZE, CONFIG_SSID_MAX_BYTES, CONFIG_VERSION,
};
pub use trust::{
    minisign_public_key_id, pinned_key_id, pinned_key_is_configured, sha256_hex, verify_minisign,
    TrustError, PINNED_MINISIGN_PUBLIC_KEY,
};

/// Schema version for the signed public flash manifest.
pub const FLASH_MANIFEST_SCHEMA: u32 = 2;
