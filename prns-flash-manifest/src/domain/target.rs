use crate::{
    FlashPart, FlashPartKind, NrfDfuApplicationVersion, NrfDfuBankLayout, ReleaseChannel,
    SourceArchiveIdentity, TargetManifest, Transport,
};

use super::values::{
    AfterResetStrategy, BeforeResetStrategy, BoardId, ChipFamily, FlashFrequency, FlashMode,
    ImmutableArtifactPath, KeyId, PreparationProfile, ProvisioningSlot, ReleaseVersion,
    Sha256Digest, Uf2BoardIdMatch, Uf2MountLabel,
};

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub enum SoftdeviceFamily {
    S140,
}

impl SoftdeviceFamily {
    pub fn parse(value: &str) -> Result<Self, super::values::DomainValueError> {
        match value.to_ascii_lowercase().as_str() {
            "s140" => Ok(Self::S140),
            _ => Err(super::values::DomainValueError::SoftdeviceFamily(
                value.to_string(),
            )),
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::S140 => "s140",
        }
    }
}

impl std::fmt::Display for SoftdeviceFamily {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct SoftdeviceVersion(String);

impl SoftdeviceVersion {
    pub fn parse(value: impl Into<String>) -> Result<Self, super::values::DomainValueError> {
        let value = value.into();
        let components = value.split('.').collect::<Vec<_>>();
        let valid = components.len() == 3
            && components.iter().all(|component| {
                !component.is_empty()
                    && component.bytes().all(|byte| byte.is_ascii_digit())
                    && (component == &"0" || !component.starts_with('0'))
                    && component.parse::<u16>().is_ok()
            });
        valid
            .then_some(Self(value.clone()))
            .ok_or(super::values::DomainValueError::SoftdeviceVersion(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SoftdeviceVersion {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct SoftdeviceIdentity {
    family: SoftdeviceFamily,
    version: SoftdeviceVersion,
}

impl SoftdeviceIdentity {
    pub fn parse(
        family: &str,
        version: impl Into<String>,
    ) -> Result<Self, super::values::DomainValueError> {
        Ok(Self {
            family: SoftdeviceFamily::parse(family)?,
            version: SoftdeviceVersion::parse(version)?,
        })
    }

    pub const fn family(&self) -> SoftdeviceFamily {
        self.family
    }

    pub fn version(&self) -> &SoftdeviceVersion {
        &self.version
    }
}

impl std::fmt::Display for SoftdeviceIdentity {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "{} {}",
            self.family.to_string().to_ascii_uppercase(),
            self.version
        )
    }
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct Uf2Compatibility {
    softdevice: SoftdeviceIdentity,
    fwid: u16,
    application_base: u32,
    application_end_exclusive: u32,
    family_id: u32,
}

impl Uf2Compatibility {
    pub(crate) fn new(
        softdevice: SoftdeviceIdentity,
        fwid: u16,
        application_base: u32,
        application_end_exclusive: u32,
        family_id: u32,
    ) -> Self {
        Self {
            softdevice,
            fwid,
            application_base,
            application_end_exclusive,
            family_id,
        }
    }

    pub fn softdevice(&self) -> &SoftdeviceIdentity {
        &self.softdevice
    }

    pub const fn fwid(&self) -> u16 {
        self.fwid
    }

    pub const fn application_base(&self) -> u32 {
        self.application_base
    }

    pub const fn application_end_exclusive(&self) -> u32 {
        self.application_end_exclusive
    }

    pub const fn family_id(&self) -> u32 {
        self.family_id
    }

    pub fn label(&self) -> String {
        format!(
            "{}-{}-fwid-0x{:04x}",
            self.softdevice.family().as_str(),
            self.softdevice.version(),
            self.fwid
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatedReleaseInfo {
    pub(crate) version: ReleaseVersion,
    pub(crate) channel: ReleaseChannel,
    pub(crate) commit: String,
}

impl ValidatedReleaseInfo {
    pub fn version(&self) -> &ReleaseVersion {
        &self.version
    }

    pub const fn channel(&self) -> ReleaseChannel {
        self.channel
    }

    pub fn commit(&self) -> &str {
        &self.commit
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatedOfflineKeySigningInfo {
    pub(crate) key_id: KeyId,
}

impl ValidatedOfflineKeySigningInfo {
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
    pub(crate) source: Option<SourceArchiveIdentity>,
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
    pub const fn kind(&self) -> FlashPartKind {
        self.kind
    }

    pub fn path(&self) -> &ImmutableArtifactPath {
        &self.path
    }

    pub const fn offset(&self) -> u32 {
        self.offset
    }

    pub const fn size(&self) -> u64 {
        self.size
    }

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Uf2Variant {
    pub(crate) compatibility: Uf2Compatibility,
    pub(crate) part: Uf2Part,
}

impl Uf2Variant {
    pub fn compatibility(&self) -> &Uf2Compatibility {
        &self.compatibility
    }

    pub fn part(&self) -> &Uf2Part {
        &self.part
    }
}

impl Uf2Part {
    pub fn path(&self) -> &ImmutableArtifactPath {
        &self.path
    }

    pub const fn size(&self) -> u64 {
        self.size
    }

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatedNrfSerialDfuCompatibility {
    pub(crate) softdevice: SoftdeviceIdentity,
    pub(crate) fwid: u16,
    pub(crate) device_type: u16,
    pub(crate) device_revision: u16,
    pub(crate) application_version: NrfDfuApplicationVersion,
    pub(crate) application_base: u32,
    pub(crate) application_end_exclusive: u32,
    pub(crate) bank_layout: NrfDfuBankLayout,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UsbVidPid {
    pub(crate) vendor_id: u16,
    pub(crate) product_id: u16,
}

impl UsbVidPid {
    pub const fn vendor_id(self) -> u16 {
        self.vendor_id
    }

    pub const fn product_id(self) -> u16 {
        self.product_id
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatedNrfSerialDfuSerialTransport {
    pub(crate) touch_application_and_bootloader_usb: UsbVidPid,
    pub(crate) recovery_bootloader_usb: UsbVidPid,
    pub(crate) recovery_bootloader_manufacturer: String,
    pub(crate) recovery_bootloader_product: String,
    pub(crate) touch_baud_rate: u32,
    pub(crate) managed_application_usb: UsbVidPid,
    pub(crate) managed_application_manufacturer: String,
    pub(crate) managed_application_product: String,
    pub(crate) managed_application_serial_number: String,
    pub(crate) managed_application_interface_number: u8,
    pub(crate) managed_application_request: u8,
    pub(crate) managed_application_value: u16,
    pub(crate) managed_application_index: u16,
    pub(crate) transfer_baud_rate: u32,
}

impl ValidatedNrfSerialDfuSerialTransport {
    pub const fn touch_application_and_bootloader_usb(&self) -> UsbVidPid {
        self.touch_application_and_bootloader_usb
    }

    pub const fn recovery_bootloader_usb(&self) -> UsbVidPid {
        self.recovery_bootloader_usb
    }

    pub fn recovery_bootloader_manufacturer(&self) -> &str {
        &self.recovery_bootloader_manufacturer
    }

    pub fn recovery_bootloader_product(&self) -> &str {
        &self.recovery_bootloader_product
    }

    pub const fn touch_baud_rate(&self) -> u32 {
        self.touch_baud_rate
    }

    pub const fn managed_application_usb(&self) -> UsbVidPid {
        self.managed_application_usb
    }

    pub fn managed_application_manufacturer(&self) -> &str {
        &self.managed_application_manufacturer
    }

    pub fn managed_application_product(&self) -> &str {
        &self.managed_application_product
    }

    pub fn managed_application_serial_number(&self) -> &str {
        &self.managed_application_serial_number
    }

    pub const fn managed_application_interface_number(&self) -> u8 {
        self.managed_application_interface_number
    }

    pub const fn managed_application_request(&self) -> u8 {
        self.managed_application_request
    }

    pub const fn managed_application_value(&self) -> u16 {
        self.managed_application_value
    }

    pub const fn managed_application_index(&self) -> u16 {
        self.managed_application_index
    }

    pub const fn transfer_baud_rate(&self) -> u32 {
        self.transfer_baud_rate
    }
}

impl ValidatedNrfSerialDfuCompatibility {
    pub fn softdevice(&self) -> &SoftdeviceIdentity {
        &self.softdevice
    }

    pub const fn fwid(&self) -> u16 {
        self.fwid
    }

    pub const fn device_type(&self) -> u16 {
        self.device_type
    }

    pub const fn device_revision(&self) -> u16 {
        self.device_revision
    }

    pub const fn application_version(&self) -> NrfDfuApplicationVersion {
        self.application_version
    }

    pub const fn application_base(&self) -> u32 {
        self.application_base
    }

    pub const fn application_end_exclusive(&self) -> u32 {
        self.application_end_exclusive
    }

    pub const fn bank_layout(&self) -> NrfDfuBankLayout {
        self.bank_layout
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NrfSerialDfuArtifact {
    pub(crate) kind: FlashPartKind,
    pub(crate) path: ImmutableArtifactPath,
    pub(crate) size: u64,
    pub(crate) sha256: Sha256Digest,
}

impl NrfSerialDfuArtifact {
    pub const fn kind(&self) -> FlashPartKind {
        self.kind
    }

    pub fn path(&self) -> &ImmutableArtifactPath {
        &self.path
    }

    pub const fn size(&self) -> u64 {
        self.size
    }

    pub fn sha256(&self) -> &Sha256Digest {
        &self.sha256
    }

    pub(crate) fn to_wire(&self) -> FlashPart {
        FlashPart {
            kind: self.kind,
            path: self.path.as_str().to_string(),
            offset: None,
            size: self.size,
            sha256: self.sha256.as_str().to_string(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NrfSerialDfuRecovery {
    pub(crate) mount_label: Uf2MountLabel,
    pub(crate) board_id_match: Uf2BoardIdMatch,
    pub(crate) family_id: u32,
    pub(crate) artifact: NrfSerialDfuArtifact,
}

impl NrfSerialDfuRecovery {
    pub fn mount_label(&self) -> &Uf2MountLabel {
        &self.mount_label
    }

    pub fn board_id_match(&self) -> &Uf2BoardIdMatch {
        &self.board_id_match
    }

    pub const fn family_id(&self) -> u32 {
        self.family_id
    }

    pub fn artifact(&self) -> &NrfSerialDfuArtifact {
        &self.artifact
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NrfSerialDfuTarget {
    pub(crate) identity: TargetIdentity,
    pub(crate) serial_transport: ValidatedNrfSerialDfuSerialTransport,
    pub(crate) compatibility: ValidatedNrfSerialDfuCompatibility,
    pub(crate) application: NrfSerialDfuArtifact,
    pub(crate) init_packet: NrfSerialDfuArtifact,
    pub(crate) recovery: NrfSerialDfuRecovery,
}

impl NrfSerialDfuTarget {
    pub fn serial_transport(&self) -> &ValidatedNrfSerialDfuSerialTransport {
        &self.serial_transport
    }

    pub fn compatibility(&self) -> &ValidatedNrfSerialDfuCompatibility {
        &self.compatibility
    }

    pub fn application(&self) -> &NrfSerialDfuArtifact {
        &self.application
    }

    pub fn init_packet(&self) -> &NrfSerialDfuArtifact {
        &self.init_packet
    }

    pub fn recovery(&self) -> &NrfSerialDfuRecovery {
        &self.recovery
    }
}

/// Borrowed part shared by transport-neutral verification code.
#[derive(Clone, Copy, Debug)]
pub enum ReleasePartRef<'a> {
    Esp(&'a EspFlashPart),
    Uf2(&'a Uf2Part),
    NrfSerialDfu(&'a NrfSerialDfuArtifact),
}

impl ReleasePartRef<'_> {
    pub const fn kind(self) -> FlashPartKind {
        match self {
            Self::Esp(part) => part.kind(),
            Self::Uf2(_) => FlashPartKind::Uf2,
            Self::NrfSerialDfu(part) => part.kind(),
        }
    }

    pub fn path(&self) -> &ImmutableArtifactPath {
        match *self {
            Self::Esp(part) => part.path(),
            Self::Uf2(part) => part.path(),
            Self::NrfSerialDfu(part) => part.path(),
        }
    }

    pub const fn offset(self) -> Option<u32> {
        match self {
            Self::Esp(part) => Some(part.offset()),
            Self::Uf2(_) => None,
            Self::NrfSerialDfu(_) => None,
        }
    }

    pub const fn size(self) -> u64 {
        match self {
            Self::Esp(part) => part.size(),
            Self::Uf2(part) => part.size(),
            Self::NrfSerialDfu(part) => part.size(),
        }
    }

    pub fn sha256(&self) -> &Sha256Digest {
        match *self {
            Self::Esp(part) => part.sha256(),
            Self::Uf2(part) => part.sha256(),
            Self::NrfSerialDfu(part) => part.sha256(),
        }
    }

    pub fn to_wire(self) -> FlashPart {
        match self {
            Self::Esp(part) => part.to_wire(),
            Self::Uf2(part) => part.to_wire(),
            Self::NrfSerialDfu(part) => part.to_wire(),
        }
    }
}

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
    pub const fn expected_chip(&self) -> ChipFamily {
        self.expected_chip
    }

    pub const fn flash_size(&self) -> u32 {
        self.flash_size
    }

    pub const fn flash_mode(&self) -> FlashMode {
        self.flash_mode
    }

    pub const fn flash_frequency(&self) -> FlashFrequency {
        self.flash_frequency
    }

    pub const fn before_reset(&self) -> BeforeResetStrategy {
        self.before_reset
    }

    pub const fn after_reset(&self) -> AfterResetStrategy {
        self.after_reset
    }

    pub fn parts(&self) -> &[EspFlashPart] {
        &self.parts
    }

    pub fn provisioning(&self) -> Option<&ProvisioningSlot> {
        self.provisioning.as_ref()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Uf2Target {
    pub(crate) identity: TargetIdentity,
    pub(crate) variants: Vec<Uf2Variant>,
}

impl Uf2Target {
    pub fn variants(&self) -> &[Uf2Variant] {
        &self.variants
    }

    pub fn variant_for(&self, identity: &SoftdeviceIdentity) -> Option<&Uf2Variant> {
        self.variants
            .iter()
            .find(|variant| variant.compatibility.softdevice() == identity)
    }
}

/// A validated target whose transport-specific impossible states are unrepresentable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReleaseTarget {
    EspSerial(EspSerialTarget),
    Uf2(Uf2Target),
    NrfSerialDfu(Box<NrfSerialDfuTarget>),
}

impl ReleaseTarget {
    fn identity(&self) -> &TargetIdentity {
        match self {
            Self::EspSerial(target) => &target.identity,
            Self::Uf2(target) => &target.identity,
            Self::NrfSerialDfu(target) => &target.identity,
        }
    }

    pub fn board_id(&self) -> &BoardId {
        &self.identity().board_id
    }

    pub fn display_name(&self) -> &str {
        &self.identity().display_name
    }

    pub fn silicon(&self) -> &str {
        &self.identity().silicon
    }

    pub fn interfaces(&self) -> &[String] {
        &self.identity().interfaces
    }

    pub const fn preparation_profile(&self) -> PreparationProfile {
        match self {
            Self::EspSerial(target) => target.identity.preparation_profile,
            Self::Uf2(target) => target.identity.preparation_profile,
            Self::NrfSerialDfu(target) => target.identity.preparation_profile,
        }
    }

    pub const fn transport(&self) -> Transport {
        match self {
            Self::EspSerial(_) => Transport::EspSerial,
            Self::Uf2(_) => Transport::Uf2MassStorage,
            Self::NrfSerialDfu(_) => Transport::NrfSerialDfu,
        }
    }

    pub fn parts(&self) -> Vec<ReleasePartRef<'_>> {
        match self {
            Self::EspSerial(target) => target.parts.iter().map(ReleasePartRef::Esp).collect(),
            Self::Uf2(target) => target
                .variants
                .iter()
                .map(|variant| ReleasePartRef::Uf2(&variant.part))
                .collect(),
            Self::NrfSerialDfu(target) => vec![
                ReleasePartRef::NrfSerialDfu(&target.application),
                ReleasePartRef::NrfSerialDfu(&target.init_packet),
                ReleasePartRef::NrfSerialDfu(&target.recovery.artifact),
            ],
        }
    }

    pub fn provisioning(&self) -> Option<&ProvisioningSlot> {
        match self {
            Self::EspSerial(target) => target.provisioning(),
            Self::Uf2(_) => None,
            Self::NrfSerialDfu(_) => None,
        }
    }

    pub fn source(&self) -> Option<&SourceArchiveIdentity> {
        self.identity().source.as_ref()
    }

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
                variants: Vec::new(),
                nrf_serial_dfu: None,
                provisioning: target.provisioning.as_ref().map(ProvisioningSlot::to_wire),
                source: identity.source.clone(),
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
                parts: Vec::new(),
                variants: target
                    .variants
                    .iter()
                    .map(|variant| crate::Uf2VariantManifest {
                        softdevice_family: variant
                            .compatibility
                            .softdevice()
                            .family()
                            .as_str()
                            .to_string(),
                        softdevice_version: variant
                            .compatibility
                            .softdevice()
                            .version()
                            .as_str()
                            .to_string(),
                        fwid: format!("0x{:04x}", variant.compatibility.fwid()),
                        application_base: format!(
                            "0x{:08x}",
                            variant.compatibility.application_base()
                        ),
                        family_id: format!("0x{:08x}", variant.compatibility.family_id()),
                        path: variant.part.path.as_str().to_string(),
                        size: variant.part.size,
                        sha256: variant.part.sha256.as_str().to_string(),
                    })
                    .collect(),
                nrf_serial_dfu: None,
                provisioning: None,
                source: identity.source.clone(),
            },
            Self::NrfSerialDfu(target) => TargetManifest {
                board_slug: identity.board_id.as_str().to_string(),
                display_name: identity.display_name.clone(),
                silicon: identity.silicon.clone(),
                interfaces: identity.interfaces.clone(),
                transport: Transport::NrfSerialDfu,
                expected_chip: None,
                flash_size: None,
                flash_mode: None,
                flash_frequency: None,
                before_reset: None,
                after_reset: None,
                preparation_profile: identity.preparation_profile.as_str().to_string(),
                parts: Vec::new(),
                variants: Vec::new(),
                nrf_serial_dfu: Some(crate::NrfSerialDfuManifest {
                    serial: crate::NrfSerialDfuSerialTransport {
                        touch_application_and_bootloader:
                            crate::NrfSerialDfuTouchApplicationAndBootloader {
                                usb: crate::UsbVendorProductId {
                                    vendor_id: format!(
                                        "0x{:04x}",
                                        target
                                            .serial_transport
                                            .touch_application_and_bootloader_usb
                                            .vendor_id
                                    ),
                                    product_id: format!(
                                        "0x{:04x}",
                                        target
                                            .serial_transport
                                            .touch_application_and_bootloader_usb
                                            .product_id
                                    ),
                                },
                                touch_baud_rate: target.serial_transport.touch_baud_rate,
                                transfer_baud_rate: target.serial_transport.transfer_baud_rate,
                            },
                        recovery_bootloader: crate::NrfSerialDfuRecoveryBootloader {
                            usb: crate::UsbVendorProductId {
                                vendor_id: format!(
                                    "0x{:04x}",
                                    target.serial_transport.recovery_bootloader_usb.vendor_id
                                ),
                                product_id: format!(
                                    "0x{:04x}",
                                    target.serial_transport.recovery_bootloader_usb.product_id
                                ),
                            },
                            manufacturer: target
                                .serial_transport
                                .recovery_bootloader_manufacturer
                                .clone(),
                            product: target.serial_transport.recovery_bootloader_product.clone(),
                        },
                        managed_application: crate::NrfSerialDfuControlApplication {
                            usb: crate::UsbVendorProductId {
                                vendor_id: format!(
                                    "0x{:04x}",
                                    target.serial_transport.managed_application_usb.vendor_id
                                ),
                                product_id: format!(
                                    "0x{:04x}",
                                    target.serial_transport.managed_application_usb.product_id
                                ),
                            },
                            manufacturer: target
                                .serial_transport
                                .managed_application_manufacturer
                                .clone(),
                            product: target.serial_transport.managed_application_product.clone(),
                            serial_number: target
                                .serial_transport
                                .managed_application_serial_number
                                .clone(),
                            interface_number: target
                                .serial_transport
                                .managed_application_interface_number,
                            request: format!(
                                "0x{:02x}",
                                target.serial_transport.managed_application_request
                            ),
                            value: format!(
                                "0x{:04x}",
                                target.serial_transport.managed_application_value
                            ),
                            index: format!(
                                "0x{:04x}",
                                target.serial_transport.managed_application_index
                            ),
                        },
                    },
                    compatibility: crate::NrfSerialDfuCompatibility {
                        softdevice_family: target
                            .compatibility
                            .softdevice
                            .family()
                            .as_str()
                            .to_string(),
                        softdevice_version: target
                            .compatibility
                            .softdevice
                            .version()
                            .as_str()
                            .to_string(),
                        fwid: format!("0x{:04x}", target.compatibility.fwid),
                        device_type: format!("0x{:04x}", target.compatibility.device_type),
                        device_revision: target.compatibility.device_revision,
                        application_version: target.compatibility.application_version,
                        application_base: format!(
                            "0x{:08x}",
                            target.compatibility.application_base
                        ),
                        application_end_exclusive: format!(
                            "0x{:08x}",
                            target.compatibility.application_end_exclusive
                        ),
                        bank_layout: target.compatibility.bank_layout,
                    },
                    application: target.application.to_wire(),
                    init_packet: target.init_packet.to_wire(),
                    recovery: crate::NrfSerialDfuRecoveryManifest {
                        mount_label: target.recovery.mount_label.as_str().to_string(),
                        board_id_prefix: target.recovery.board_id_match.as_str().to_string(),
                        family_id: format!("0x{:08x}", target.recovery.family_id),
                        artifact: target.recovery.artifact.to_wire(),
                    },
                }),
                provisioning: None,
                source: identity.source.clone(),
            },
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatedFlashManifest {
    pub(crate) schema_version: u32,
    pub(crate) release: ValidatedReleaseInfo,
    pub(crate) signing: ValidatedOfflineKeySigningInfo,
    pub(crate) targets: Vec<ReleaseTarget>,
}

impl ValidatedFlashManifest {
    pub const fn schema_version(&self) -> u32 {
        self.schema_version
    }

    pub fn release(&self) -> &ValidatedReleaseInfo {
        &self.release
    }

    pub fn signing(&self) -> &ValidatedOfflineKeySigningInfo {
        &self.signing
    }

    pub fn targets(&self) -> &[ReleaseTarget] {
        &self.targets
    }

    pub fn into_targets(self) -> Vec<ReleaseTarget> {
        self.targets
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatedChannelDescriptor {
    pub(crate) schema_version: u32,
    pub(crate) channel: ReleaseChannel,
    pub(crate) version: ReleaseVersion,
    pub(crate) manifest_url: String,
    pub(crate) manifest_sha256: Sha256Digest,
}

impl ValidatedChannelDescriptor {
    pub const fn schema_version(&self) -> u32 {
        self.schema_version
    }

    pub const fn channel(&self) -> ReleaseChannel {
        self.channel
    }

    pub fn version(&self) -> &ReleaseVersion {
        &self.version
    }

    pub fn manifest_url(&self) -> &str {
        &self.manifest_url
    }

    pub fn manifest_sha256(&self) -> &Sha256Digest {
        &self.manifest_sha256
    }
}
