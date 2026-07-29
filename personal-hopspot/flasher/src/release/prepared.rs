//! Bind already-verified bytes to typed ESP and UF2 part descriptors.

use prns_flash_manifest::{
    sha256_hex, BoardId, EspFlashPart, ImmutableArtifactPath, ReleaseTarget, ReleaseVersion,
    Sha256Digest, Uf2Part,
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
}

#[derive(Debug)]
pub(crate) enum PreparedTarget {
    EspSerial(PreparedEspTarget),
    Uf2(PreparedUf2Target),
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
                let bytes = match <Vec<Vec<u8>> as TryInto<[Vec<u8>; 1]>>::try_into(artifacts) {
                    Ok([bytes]) => bytes,
                    Err(artifacts) => {
                        return Err(PreparedArtifactError::Count {
                            board_id,
                            expected: 1,
                            actual: artifacts.len(),
                        });
                    }
                };
                Ok(Self::Uf2(PreparedUf2Target {
                    version,
                    board_id,
                    part: PreparedUf2Part::bind(target.part().clone(), bytes)?,
                }))
            }
        }
    }

    pub(crate) fn version(&self) -> &ReleaseVersion {
        match self {
            Self::EspSerial(target) => target.version(),
            Self::Uf2(target) => target.version(),
        }
    }

    pub(crate) fn board_id(&self) -> &BoardId {
        match self {
            Self::EspSerial(target) => target.board_id(),
            Self::Uf2(target) => target.board_id(),
        }
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
}

#[derive(Debug)]
pub(crate) struct PreparedUf2Part {
    bytes: Vec<u8>,
}

impl PreparedUf2Part {
    fn bind(descriptor: Uf2Part, bytes: Vec<u8>) -> Result<Self, PreparedArtifactError> {
        verify_prepared_artifact(
            descriptor.path(),
            descriptor.size(),
            descriptor.sha256(),
            &bytes,
        )?;
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
