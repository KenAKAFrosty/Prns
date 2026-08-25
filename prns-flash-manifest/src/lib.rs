//! Shared release contract for the Personal Hopspot web and CLI flashers.

mod catalog;
mod domain;
mod manifest;
mod provisioning;
mod trust;
mod uf2;

pub use catalog::{
    board_catalog, BoardAvailability, BoardBuild, BoardCatalog, BoardCatalogEntry, CatalogError,
    EspBuild, NrfDfuApplicationVersion, NrfDfuBankLayout, NrfSerialDfuBuild,
    NrfSerialDfuCompatibility, NrfSerialDfuControlApplication, NrfSerialDfuRecoveryBootloader,
    NrfSerialDfuRecoveryBuild, NrfSerialDfuSerialTransport, NrfSerialDfuSerialTransportError,
    NrfSerialDfuTouchApplicationAndBootloader, ProvisioningDescriptor,
    TcpClientProvisioningDescriptor, Transport, Uf2ApplicationLink, Uf2ApplicationUsb,
    Uf2BoardIdentity, Uf2Build, Uf2BuildVariant, UsbVendorProductId,
};
pub use domain::{
    AfterResetStrategy, BeforeResetStrategy, BoardId, ChipFamily, DomainValueError, EspFlashPart,
    EspSerialTarget, FlashFrequency, FlashMode, ImmutableArtifactPath, KeyId, NrfSerialDfuArtifact,
    NrfSerialDfuRecovery, NrfSerialDfuTarget, PreparationProfile, ProvisioningFormat,
    ProvisioningSlot, ReleasePartRef, ReleaseTarget, ReleaseVersion, Sha256Digest,
    SoftdeviceFamily, SoftdeviceIdentity, SoftdeviceVersion, Uf2BoardIdMatch, Uf2BoardIdMatchKind,
    Uf2Compatibility, Uf2MountLabel, Uf2Part, Uf2Target, Uf2Variant, UsbVidPid,
    ValidatedChannelDescriptor, ValidatedFlashManifest, ValidatedNrfSerialDfuCompatibility,
    ValidatedNrfSerialDfuSerialTransport, ValidatedOfflineKeySigningInfo, ValidatedReleaseInfo,
};
pub use manifest::{
    ChannelDescriptor, FlashManifest, FlashPart, FlashPartKind, ManifestError,
    ManifestTargetSetPolicy, NrfSerialDfuManifest, NrfSerialDfuRecoveryManifest,
    OfflineKeySigningInfo, ReleaseChannel, ReleaseInfo, SourceArchiveIdentity, TargetManifest,
    Uf2VariantManifest,
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
pub use uf2::{
    validate_nrf_serial_dfu_recovery_artifact, validate_uf2_artifact, Uf2ArtifactError,
    Uf2BootloaderIdentity, Uf2IdentityError,
};

pub const FLASH_MANIFEST_SCHEMA: u32 = 3;

pub const ESP_FLASH_SECTOR_SIZE: u32 = 0x1000;
