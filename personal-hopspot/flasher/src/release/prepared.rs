use prns_flash_manifest::{
    sha256_hex, validate_uf2_artifact, BoardId, EspFlashPart, ImmutableArtifactPath,
    NrfDfuApplicationVersion, NrfDfuBankLayout, NrfSerialDfuArtifact, NrfSerialDfuTarget,
    ReleaseTarget, ReleaseVersion, Sha256Digest, SoftdeviceIdentity, Uf2ArtifactError,
    Uf2Compatibility, Uf2Part, Uf2Variant, ValidatedNrfSerialDfuSerialTransport,
};
use prns_nrf_dfu::{
    ApplicationInitPacket, ApplicationInitPacketSpec, ApplicationVersion, DfuBankLayout,
    DfuDeviceRevision, DfuDeviceType, DfuImage, DfuImageError, SoftdeviceFirmwareId,
    SoftdeviceRequirements,
};
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub(crate) enum PreparedArtifactError {
    #[error(
        "signed target {board_id} requires {expected} artifacts, but {actual} byte buffers were supplied"
    )]
    Count {
        board_id: BoardId,
        expected: usize,
        actual: usize,
    },
    #[error("artifact {path} is {actual} bytes; the signed target requires {expected}")]
    Size {
        path: ImmutableArtifactPath,
        expected: u64,
        actual: u64,
    },
    #[error("SHA-256 mismatch for {path}: expected {expected}, found {actual}")]
    Hash {
        path: ImmutableArtifactPath,
        expected: Sha256Digest,
        actual: String,
    },
    #[error("UF2 compatibility selection is required for signed target {0}")]
    CompatibilityRequired(BoardId),
    #[error("signed target {board_id} has no UF2 variant for {softdevice}")]
    CompatibilityMissing {
        board_id: BoardId,
        softdevice: SoftdeviceIdentity,
    },
    #[error("artifact {path} is not a valid signed UF2 variant: {source}")]
    Uf2 {
        path: ImmutableArtifactPath,
        source: Uf2ArtifactError,
    },
    #[error("artifact {path} is not a valid Nordic serial DFU image: {source}")]
    NrfSerialDfu {
        path: ImmutableArtifactPath,
        source: DfuImageError,
    },
}

#[derive(Debug)]
pub(crate) enum PreparedTarget {
    EspSerial(PreparedEspTarget),
    Uf2(PreparedUf2Target),
    NrfSerialDfu(PreparedNrfSerialDfuTarget),
}

impl PreparedTarget {
    pub(crate) fn bind(
        version: ReleaseVersion,
        target: ReleaseTarget,
        artifacts: Vec<Vec<u8>>,
    ) -> Result<Self, PreparedArtifactError> {
        let board_id = target.board_id().clone();
        match target {
            ReleaseTarget::EspSerial(target) => {
                let supports_tcp_client_provisioning = target
                    .provisioning()
                    .and_then(|slot| slot.tcp_client())
                    .is_some();
                let descriptors = target.parts();
                verify_artifact_count(&board_id, descriptors.len(), artifacts.len())?;
                let parts = descriptors
                    .iter()
                    .cloned()
                    .zip(artifacts)
                    .map(|(descriptor, bytes)| PreparedEspPart::bind(descriptor, bytes))
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(Self::EspSerial(PreparedEspTarget {
                    version,
                    board_id,
                    parts,
                    supports_tcp_client_provisioning,
                }))
            }
            ReleaseTarget::Uf2(target) => {
                let _ = (version, target, artifacts);
                Err(PreparedArtifactError::CompatibilityRequired(board_id))
            }
            ReleaseTarget::NrfSerialDfu(target) => {
                verify_artifact_count(&board_id, 2, artifacts.len())?;
                let [application, init_packet] =
                    <[Vec<u8>; 2]>::try_from(artifacts).map_err(|artifacts| {
                        PreparedArtifactError::Count {
                            board_id: board_id.clone(),
                            expected: 2,
                            actual: artifacts.len(),
                        }
                    })?;
                Ok(Self::NrfSerialDfu(PreparedNrfSerialDfuTarget::bind(
                    version,
                    board_id,
                    target,
                    application,
                    init_packet,
                )?))
            }
        }
    }

    pub(crate) fn bind_uf2(
        version: ReleaseVersion,
        target: ReleaseTarget,
        softdevice: &SoftdeviceIdentity,
        bytes: Vec<u8>,
    ) -> Result<Self, PreparedArtifactError> {
        let board_id = target.board_id().clone();
        let ReleaseTarget::Uf2(target) = target else {
            return Err(PreparedArtifactError::CompatibilityRequired(board_id));
        };
        let variant = target.variant_for(softdevice).cloned().ok_or_else(|| {
            PreparedArtifactError::CompatibilityMissing {
                board_id: board_id.clone(),
                softdevice: softdevice.clone(),
            }
        })?;
        Ok(Self::Uf2(PreparedUf2Target {
            version,
            board_id,
            compatibility: variant.compatibility().clone(),
            part: PreparedUf2Part::bind(variant, bytes)?,
        }))
    }

    pub(crate) fn version(&self) -> &ReleaseVersion {
        match self {
            Self::EspSerial(target) => target.version(),
            Self::Uf2(target) => target.version(),
            Self::NrfSerialDfu(target) => target.version(),
        }
    }

    pub(crate) fn board_id(&self) -> &BoardId {
        match self {
            Self::EspSerial(target) => target.board_id(),
            Self::Uf2(target) => target.board_id(),
            Self::NrfSerialDfu(target) => target.board_id(),
        }
    }
}

#[derive(Debug)]
pub(crate) struct PreparedNrfSerialDfuTarget {
    version: ReleaseVersion,
    board_id: BoardId,
    serial_transport: ValidatedNrfSerialDfuSerialTransport,
    bank_layout: DfuBankLayout,
    application: Vec<u8>,
    init_packet: ApplicationInitPacket,
}

impl PreparedNrfSerialDfuTarget {
    fn bind(
        version: ReleaseVersion,
        board_id: BoardId,
        target: NrfSerialDfuTarget,
        application: Vec<u8>,
        init_packet: Vec<u8>,
    ) -> Result<Self, PreparedArtifactError> {
        verify_nrf_serial_dfu_artifact(target.application(), &application)?;
        verify_nrf_serial_dfu_artifact(target.init_packet(), &init_packet)?;
        let compatibility = target.compatibility();
        let required_softdevice =
            SoftdeviceFirmwareId::new(compatibility.fwid()).map_err(|source| {
                PreparedArtifactError::NrfSerialDfu {
                    path: target.init_packet().path().clone(),
                    source,
                }
            })?;
        let softdevices = SoftdeviceRequirements::new(required_softdevice, std::iter::empty())
            .map_err(|source| PreparedArtifactError::NrfSerialDfu {
                path: target.init_packet().path().clone(),
                source,
            })?;
        let spec = ApplicationInitPacketSpec {
            device_type: DfuDeviceType::new(compatibility.device_type()),
            device_revision: DfuDeviceRevision::new(compatibility.device_revision()),
            application_version: match compatibility.application_version() {
                NrfDfuApplicationVersion::NotEnforced => ApplicationVersion::NotEnforced,
            },
            softdevices,
        };
        let init_packet = ApplicationInitPacket::from_artifacts(&application, &init_packet, &spec)
            .map_err(|source| PreparedArtifactError::NrfSerialDfu {
                path: target.init_packet().path().clone(),
                source,
            })?;
        let bank_layout = match compatibility.bank_layout() {
            NrfDfuBankLayout::Single => DfuBankLayout::Single,
            NrfDfuBankLayout::Dual => DfuBankLayout::Dual,
        };
        Ok(Self {
            version,
            board_id,
            serial_transport: target.serial_transport().clone(),
            bank_layout,
            application,
            init_packet,
        })
    }

    pub(crate) fn version(&self) -> &ReleaseVersion {
        &self.version
    }

    pub(crate) fn board_id(&self) -> &BoardId {
        &self.board_id
    }

    pub(crate) fn serial_transport(&self) -> &ValidatedNrfSerialDfuSerialTransport {
        &self.serial_transport
    }

    pub(crate) const fn bank_layout(&self) -> DfuBankLayout {
        self.bank_layout
    }

    pub(crate) fn image(&self) -> Result<DfuImage<'_>, DfuImageError> {
        DfuImage::new(&self.application, self.init_packet.clone())
    }
}

#[derive(Debug)]
pub(crate) struct PreparedEspTarget {
    version: ReleaseVersion,
    board_id: BoardId,
    parts: Vec<PreparedEspPart>,
    supports_tcp_client_provisioning: bool,
}

impl PreparedEspTarget {
    pub(crate) fn version(&self) -> &ReleaseVersion {
        &self.version
    }

    pub(crate) fn board_id(&self) -> &BoardId {
        &self.board_id
    }

    pub(crate) fn parts(&self) -> &[PreparedEspPart] {
        &self.parts
    }

    pub(crate) const fn supports_tcp_client_provisioning(&self) -> bool {
        self.supports_tcp_client_provisioning
    }
}

#[derive(Debug)]
pub(crate) struct PreparedEspPart {
    descriptor: EspFlashPart,
    bytes: Vec<u8>,
}

impl PreparedEspPart {
    fn bind(descriptor: EspFlashPart, bytes: Vec<u8>) -> Result<Self, PreparedArtifactError> {
        verify_prepared_artifact(
            descriptor.path(),
            descriptor.size(),
            descriptor.sha256(),
            &bytes,
        )?;
        Ok(Self { descriptor, bytes })
    }

    pub(crate) const fn offset(&self) -> u32 {
        self.descriptor.offset()
    }

    pub(crate) fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

#[derive(Debug)]
pub(crate) struct PreparedUf2Target {
    version: ReleaseVersion,
    board_id: BoardId,
    compatibility: Uf2Compatibility,
    part: PreparedUf2Part,
}

impl PreparedUf2Target {
    pub(crate) fn version(&self) -> &ReleaseVersion {
        &self.version
    }

    pub(crate) fn board_id(&self) -> &BoardId {
        &self.board_id
    }

    pub(crate) fn part(&self) -> &PreparedUf2Part {
        &self.part
    }

    pub(crate) fn compatibility(&self) -> &Uf2Compatibility {
        &self.compatibility
    }
}

#[derive(Debug)]
pub(crate) struct PreparedUf2Part {
    bytes: Vec<u8>,
}

impl PreparedUf2Part {
    fn bind(variant: Uf2Variant, bytes: Vec<u8>) -> Result<Self, PreparedArtifactError> {
        let descriptor: &Uf2Part = variant.part();
        verify_prepared_artifact(
            descriptor.path(),
            descriptor.size(),
            descriptor.sha256(),
            &bytes,
        )?;
        validate_uf2_artifact(&variant, &bytes).map_err(|source| PreparedArtifactError::Uf2 {
            path: descriptor.path().clone(),
            source,
        })?;
        Ok(Self { bytes })
    }

    pub(crate) fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

fn verify_artifact_count(
    board_id: &BoardId,
    expected: usize,
    actual: usize,
) -> Result<(), PreparedArtifactError> {
    if expected == actual {
        Ok(())
    } else {
        Err(PreparedArtifactError::Count {
            board_id: board_id.clone(),
            expected,
            actual,
        })
    }
}

fn verify_prepared_artifact(
    path: &ImmutableArtifactPath,
    expected_size: u64,
    expected_hash: &Sha256Digest,
    bytes: &[u8],
) -> Result<(), PreparedArtifactError> {
    let actual_size = bytes.len() as u64;
    if actual_size != expected_size {
        return Err(PreparedArtifactError::Size {
            path: path.clone(),
            expected: expected_size,
            actual: actual_size,
        });
    }
    let actual_hash = sha256_hex(bytes);
    if actual_hash != expected_hash.as_str() {
        return Err(PreparedArtifactError::Hash {
            path: path.clone(),
            expected: expected_hash.clone(),
            actual: actual_hash,
        });
    }
    Ok(())
}

fn verify_nrf_serial_dfu_artifact(
    descriptor: &NrfSerialDfuArtifact,
    bytes: &[u8],
) -> Result<(), PreparedArtifactError> {
    verify_prepared_artifact(
        descriptor.path(),
        descriptor.size(),
        descriptor.sha256(),
        bytes,
    )
}
