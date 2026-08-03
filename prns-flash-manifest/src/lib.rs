//! Shared release contract for the Personal Hopspot web and CLI flashers.

mod catalog;
mod domain;
mod manifest;
mod provisioning;
mod trust;

pub use catalog::{
    board_catalog, BoardBuild, BoardCatalog, BoardCatalogEntry, CatalogError, EspBuild,
    ProvisioningDescriptor, TcpClientProvisioningDescriptor, Transport, Uf2Build,
};
pub use domain::{
    AfterResetStrategy, BeforeResetStrategy, BoardId, ChipFamily, DomainValueError, EspFlashPart,
    EspSerialTarget, FlashFrequency, FlashMode, ImmutableArtifactPath, KeyId, PreparationProfile,
    ProvisioningFormat, ProvisioningSlot, ReleasePartRef, ReleaseTarget, ReleaseVersion,
    Sha256Digest, Uf2BoardIdPrefix, Uf2Part, Uf2Target, ValidatedChannelDescriptor,
    ValidatedFlashManifest, ValidatedReleaseInfo, ValidatedSigningInfo,
};
pub use manifest::{
    ChannelDescriptor, FlashManifest, FlashPart, FlashPartKind, ManifestError,
    ManifestTargetSetPolicy, ReleaseChannel, ReleaseInfo, SigningInfo, SourceArchiveIdentity,
    TargetManifest,
};
pub use provisioning::{
    provisioning_image, ProvisioningAction, ProvisioningError, TcpClientEndpoint, TcpClientHost,
    WifiCredentials, CONFIG_MAGIC, CONFIG_OFFSET, CONFIG_PASSWORD_MAX_BYTES, CONFIG_SIZE,
    CONFIG_SSID_MAX_BYTES, CONFIG_TCP_CLIENT_HOSTNAME_MAX_BYTES,
    CONFIG_TCP_CLIENT_HOST_LENGTH_OFFSET, CONFIG_TCP_CLIENT_KIND_OFFSET,
    CONFIG_TCP_CLIENT_PORT_OFFSET, CONFIG_TCP_CLIENT_TARGET_OFFSET, CONFIG_VERSION,
    DEFAULT_TCP_CLIENT_PORT,
};
pub use trust::{
    minisign_public_key_id, pinned_key_id, pinned_key_is_configured, sha256_hex, verify_minisign,
    TrustError, PINNED_MINISIGN_PUBLIC_KEY,
};

/// Schema version for the signed public flash manifest.
pub const FLASH_MANIFEST_SCHEMA: u32 = 2;

/// Smallest erase unit used by supported Espressif flash targets.
pub const ESP_FLASH_SECTOR_SIZE: u32 = 0x1000;
