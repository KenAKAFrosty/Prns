use crate::{FlashPart, FlashPartKind, ReleaseChannel, TargetManifest, Transport};

use super::values::{
    AfterResetStrategy, BeforeResetStrategy, BoardId, ChipFamily, FlashFrequency, FlashMode,
    ImmutableArtifactPath, KeyId, PreparationProfile, ProvisioningSlot, ReleaseVersion,
    Sha256Digest,
};

/// Immutable release identity after validation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatedReleaseInfo {
    pub(crate) version: ReleaseVersion,
    pub(crate) channel: ReleaseChannel,
    pub(crate) commit: String,
}

impl ValidatedReleaseInfo {
    /// Immutable release version.
    pub fn version(&self) -> &ReleaseVersion {
        &self.version
    }

    /// Signed release channel.
    pub const fn channel(&self) -> ReleaseChannel {
        self.channel
    }

    /// Full source commit.
    pub fn commit(&self) -> &str {
        &self.commit
    }
}

/// Signing identity after validation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatedSigningInfo {
    pub(crate) key_id: KeyId,
}

impl ValidatedSigningInfo {
    /// Canonical signing-key identifier.
    pub fn key_id(&self) -> &KeyId {
        &self.key_id
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TargetIdentity {
    pub(crate) board_id: BoardId,
    pub(crate) display_name: String,
    pub(crate) silicon: String,
    pub(crate) interfaces: Vec<String>,
    pub(crate) preparation_profile: PreparationProfile,
}

/// One validated sparse ESP firmware part. Its offset cannot be absent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EspFlashPart {
    pub(crate) kind: FlashPartKind,
    pub(crate) path: ImmutableArtifactPath,
    pub(crate) offset: u32,
    pub(crate) size: u64,
    pub(crate) sha256: Sha256Digest,
}

impl EspFlashPart {
    /// Semantic part kind.
    pub const fn kind(&self) -> FlashPartKind {
        self.kind
    }

    /// Immutable relative artifact path.
    pub fn path(&self) -> &ImmutableArtifactPath {
        &self.path
    }

    /// Absolute flash offset.
    pub const fn offset(&self) -> u32 {
        self.offset
    }

    /// Exact byte size.
    pub const fn size(&self) -> u64 {
        self.size
    }

    /// Signed SHA-256 digest.
    pub fn sha256(&self) -> &Sha256Digest {
        &self.sha256
    }

    pub(crate) fn to_wire(&self) -> FlashPart {
        FlashPart {
            kind: self.kind,
            path: self.path.as_str().to_string(),
            offset: Some(self.offset),
            size: self.size,
            sha256: self.sha256.as_str().to_string(),
        }
    }
}

/// One validated UF2 payload. An ESP offset cannot be represented.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Uf2Part {
    pub(crate) path: ImmutableArtifactPath,
    pub(crate) size: u64,
    pub(crate) sha256: Sha256Digest,
}

impl Uf2Part {
    /// Immutable relative artifact path.
    pub fn path(&self) -> &ImmutableArtifactPath {
        &self.path
    }

    /// Exact byte size.
    pub const fn size(&self) -> u64 {
        self.size
    }

    /// Signed SHA-256 digest.
    pub fn sha256(&self) -> &Sha256Digest {
        &self.sha256
    }

    pub(crate) fn to_wire(&self) -> FlashPart {
        FlashPart {
            kind: FlashPartKind::Uf2,
            path: self.path.as_str().to_string(),
            offset: None,
            size: self.size,
            sha256: self.sha256.as_str().to_string(),
        }
    }
}

/// Borrowed part shared by transport-neutral verification code.
#[derive(Clone, Copy, Debug)]
pub enum ReleasePartRef<'a> {
    /// Sparse ESP part.
    Esp(&'a EspFlashPart),
    /// UF2 payload.
    Uf2(&'a Uf2Part),
}

impl ReleasePartRef<'_> {
    /// Semantic payload kind.
    pub const fn kind(self) -> FlashPartKind {
        match self {
            Self::Esp(part) => part.kind(),
            Self::Uf2(_) => FlashPartKind::Uf2,
        }
    }

    /// Immutable relative path.
    pub fn path(&self) -> &ImmutableArtifactPath {
        match *self {
            Self::Esp(part) => part.path(),
            Self::Uf2(part) => part.path(),
        }
    }

    /// Optional ESP offset; UF2 is always offset-free.
    pub const fn offset(self) -> Option<u32> {
        match self {
            Self::Esp(part) => Some(part.offset()),
            Self::Uf2(_) => None,
        }
    }

    /// Exact byte size.
    pub const fn size(self) -> u64 {
        match self {
            Self::Esp(part) => part.size(),
            Self::Uf2(part) => part.size(),
        }
    }

    /// Signed SHA-256 digest.
    pub fn sha256(&self) -> &Sha256Digest {
        match *self {
            Self::Esp(part) => part.sha256(),
            Self::Uf2(part) => part.sha256(),
        }
    }

    /// Convert back to the stable schema-v2 wire DTO.
    pub fn to_wire(self) -> FlashPart {
        match self {
            Self::Esp(part) => part.to_wire(),
            Self::Uf2(part) => part.to_wire(),
        }
    }
}

/// Validated Espressif serial target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EspSerialTarget {
    pub(crate) identity: TargetIdentity,
    pub(crate) expected_chip: ChipFamily,
    pub(crate) flash_size: u32,
    pub(crate) flash_mode: FlashMode,
    pub(crate) flash_frequency: FlashFrequency,
    pub(crate) before_reset: BeforeResetStrategy,
    pub(crate) after_reset: AfterResetStrategy,
    pub(crate) parts: Vec<EspFlashPart>,
    pub(crate) provisioning: Option<ProvisioningSlot>,
}

impl EspSerialTarget {
    /// Expected ROM chip family.
    pub const fn expected_chip(&self) -> ChipFamily {
        self.expected_chip
    }

    /// Physical flash capacity.
    pub const fn flash_size(&self) -> u32 {
        self.flash_size
    }

    /// SPI flash mode.
    pub const fn flash_mode(&self) -> FlashMode {
        self.flash_mode
    }

    /// SPI flash frequency.
    pub const fn flash_frequency(&self) -> FlashFrequency {
        self.flash_frequency
    }

    /// Pre-connect reset strategy.
    pub const fn before_reset(&self) -> BeforeResetStrategy {
        self.before_reset
    }

    /// Post-verification reset strategy.
    pub const fn after_reset(&self) -> AfterResetStrategy {
        self.after_reset
    }

    /// Ordered sparse parts.
    pub fn parts(&self) -> &[EspFlashPart] {
        &self.parts
    }

    /// Optional local provisioning slot.
    pub fn provisioning(&self) -> Option<&ProvisioningSlot> {
        self.provisioning.as_ref()
    }
}

/// Validated UF2 mass-storage target.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Uf2Target {
    pub(crate) identity: TargetIdentity,
    pub(crate) part: Uf2Part,
}

impl Uf2Target {
    /// The single signed UF2 payload.
    pub fn part(&self) -> &Uf2Part {
        &self.part
    }
}

/// A validated target whose transport-specific impossible states are unrepresentable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReleaseTarget {
    /// Espressif serial target with required chip, flash, reset, and offsets.
    EspSerial(EspSerialTarget),
    /// UF2 mass-storage target with exactly one offset-free payload.
    Uf2(Uf2Target),
}

impl ReleaseTarget {
    fn identity(&self) -> &TargetIdentity {
        match self {
            Self::EspSerial(target) => &target.identity,
            Self::Uf2(target) => &target.identity,
        }
    }

    /// Stable board identity.
    pub fn board_id(&self) -> &BoardId {
        &self.identity().board_id
    }

    /// User-facing board name.
    pub fn display_name(&self) -> &str {
        &self.identity().display_name
    }

    /// Captured silicon summary.
    pub fn silicon(&self) -> &str {
        &self.identity().silicon
    }

    /// Captured supported interfaces.
    pub fn interfaces(&self) -> &[String] {
        &self.identity().interfaces
    }

    /// Localized preparation profile.
    pub const fn preparation_profile(&self) -> PreparationProfile {
        match self {
            Self::EspSerial(target) => target.identity.preparation_profile,
            Self::Uf2(target) => target.identity.preparation_profile,
        }
    }

    /// Public transport.
    pub const fn transport(&self) -> Transport {
        match self {
            Self::EspSerial(_) => Transport::EspSerial,
            Self::Uf2(_) => Transport::Uf2MassStorage,
        }
    }

    /// Borrow every signed artifact in canonical order.
    pub fn parts(&self) -> Vec<ReleasePartRef<'_>> {
        match self {
            Self::EspSerial(target) => target.parts.iter().map(ReleasePartRef::Esp).collect(),
            Self::Uf2(target) => vec![ReleasePartRef::Uf2(&target.part)],
        }
    }

    /// Provisioning exists only for compatible ESP targets.
    pub fn provisioning(&self) -> Option<&ProvisioningSlot> {
        match self {
            Self::EspSerial(target) => target.provisioning(),
            Self::Uf2(_) => None,
        }
    }

    /// Convert back to the stable schema-v2 wire DTO.
    pub fn to_wire(&self) -> TargetManifest {
        let identity = self.identity();
        match self {
            Self::EspSerial(target) => TargetManifest {
                board_slug: identity.board_id.as_str().to_string(),
                display_name: identity.display_name.clone(),
                silicon: identity.silicon.clone(),
                interfaces: identity.interfaces.clone(),
                transport: Transport::EspSerial,
                expected_chip: Some(target.expected_chip.as_str().to_string()),
                flash_size: Some(target.flash_size),
                flash_mode: Some(target.flash_mode.as_str().to_string()),
                flash_frequency: Some(target.flash_frequency.as_str().to_string()),
                before_reset: Some(target.before_reset.as_str().to_string()),
                after_reset: Some(target.after_reset.as_str().to_string()),
                preparation_profile: identity.preparation_profile.as_str().to_string(),
                parts: target.parts.iter().map(EspFlashPart::to_wire).collect(),
                provisioning: target.provisioning.as_ref().map(ProvisioningSlot::to_wire),
            },
            Self::Uf2(target) => TargetManifest {
                board_slug: identity.board_id.as_str().to_string(),
                display_name: identity.display_name.clone(),
                silicon: identity.silicon.clone(),
                interfaces: identity.interfaces.clone(),
                transport: Transport::Uf2MassStorage,
                expected_chip: None,
                flash_size: None,
                flash_mode: None,
                flash_frequency: None,
                before_reset: None,
                after_reset: None,
                preparation_profile: identity.preparation_profile.as_str().to_string(),
                parts: vec![target.part.to_wire()],
                provisioning: None,
            },
        }
    }
}

/// Signed schema-v2 manifest after wire parsing and semantic conversion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatedFlashManifest {
    pub(crate) schema: u32,
    pub(crate) release: ValidatedReleaseInfo,
    pub(crate) signing: ValidatedSigningInfo,
    pub(crate) targets: Vec<ReleaseTarget>,
}

impl ValidatedFlashManifest {
    /// Manifest schema.
    pub const fn schema(&self) -> u32 {
        self.schema
    }

    /// Immutable release identity.
    pub fn release(&self) -> &ValidatedReleaseInfo {
        &self.release
    }

    /// Signing identity.
    pub fn signing(&self) -> &ValidatedSigningInfo {
        &self.signing
    }

    /// Shipping targets.
    pub fn targets(&self) -> &[ReleaseTarget] {
        &self.targets
    }

    /// Consume the manifest into its typed targets.
    pub fn into_targets(self) -> Vec<ReleaseTarget> {
        self.targets
    }
}

/// Signed channel descriptor after conversion to typed release identity and digest values.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatedChannelDescriptor {
    pub(crate) schema: u32,
    pub(crate) channel: ReleaseChannel,
    pub(crate) version: ReleaseVersion,
    pub(crate) manifest_url: String,
    pub(crate) manifest_sha256: Sha256Digest,
}

impl ValidatedChannelDescriptor {
    /// Channel descriptor schema version.
    pub const fn schema(&self) -> u32 {
        self.schema
    }

    /// Descriptor channel.
    pub const fn channel(&self) -> ReleaseChannel {
        self.channel
    }

    /// Immutable release version.
    pub fn version(&self) -> &ReleaseVersion {
        &self.version
    }

    /// Exact immutable manifest URL.
    pub fn manifest_url(&self) -> &str {
        &self.manifest_url
    }

    /// Digest of the exact signed manifest bytes.
    pub fn manifest_sha256(&self) -> &Sha256Digest {
        &self.manifest_sha256
    }
}
