use crate::crypto::{Ed25519Signature, Ed25519Verifier};
use crate::identity::{IdentityPublicKeys, PublicIdentityMaterial};
use crate::remote_control::{
    RemoteControlControllerIdentity, RemoteControlRequestKind, RemoteControlRequestSet,
    RemoteControlTargetIdentity,
};
use crate::units::DurationMillis;

use super::super::{RemoteControlPairingPermissions, RemoteControlPairingPermissionsError};
use super::transcript::permission_count;
use super::{
    RemoteControlPairingAttemptTimeout, RemoteControlPairingAttemptTimeoutError,
    RemoteControlPairingBegin, RemoteControlPairingCommit, RemoteControlPairingCompleted,
    RemoteControlPairingIdentityRole, RemoteControlPairingMessageDirection,
    RemoteControlPairingMessageKind, RemoteControlPairingOffer,
    RemoteControlPairingProtocolVersion, RemoteControlPairingTranscriptDigest,
    PAIRING_BEGIN_ENCODED_LEN, PAIRING_COMMIT_ENCODED_LEN, PAIRING_COMPLETED_ENCODED_LEN,
    PAIRING_OFFER_MAX_ENCODED_LEN,
};

#[derive(Debug, PartialEq, Eq)]
pub enum RemoteControlPairingRequest {
    Begin(RemoteControlPairingBegin),
    Commit(RemoteControlPairingCommit),
}

impl RemoteControlPairingRequest {
    pub const MAX_ENCODED_LEN: usize =
        maximum(PAIRING_BEGIN_ENCODED_LEN, PAIRING_COMMIT_ENCODED_LEN);

    #[must_use]
    pub const fn kind(&self) -> RemoteControlPairingMessageKind {
        match self {
            Self::Begin(_) => RemoteControlPairingMessageKind::Begin,
            Self::Commit(_) => RemoteControlPairingMessageKind::Commit,
        }
    }

    #[must_use]
    pub const fn encoded_len(&self) -> usize {
        match self {
            Self::Begin(_) => PAIRING_BEGIN_ENCODED_LEN,
            Self::Commit(_) => PAIRING_COMMIT_ENCODED_LEN,
        }
    }

    pub fn parse(bytes: &[u8]) -> Result<Self, RemoteControlPairingMessageParseError> {
        if bytes.len() > Self::MAX_ENCODED_LEN {
            return Err(RemoteControlPairingMessageParseError::TooLong {
                actual: bytes.len(),
                maximum: Self::MAX_ENCODED_LEN,
            });
        }
        let mut reader = PairingWireReader::new(bytes);
        let kind = read_header(&mut reader)?;
        let request = match kind {
            RemoteControlPairingMessageKind::Begin => {
                let public_keys = read_identity_public_keys(
                    &mut reader,
                    RemoteControlPairingIdentityRole::Controller,
                )?;
                Self::Begin(RemoteControlPairingBegin::new(
                    RemoteControlControllerIdentity::new(public_keys),
                ))
            }
            RemoteControlPairingMessageKind::Commit => {
                let transcript = RemoteControlPairingTranscriptDigest(*reader.take()?);
                Self::Commit(RemoteControlPairingCommit { transcript })
            }
            RemoteControlPairingMessageKind::Offer | RemoteControlPairingMessageKind::Completed => {
                return Err(RemoteControlPairingMessageParseError::UnexpectedKind {
                    direction: RemoteControlPairingMessageDirection::Request,
                    found: kind,
                })
            }
        };
        reader.finish()?;
        Ok(request)
    }

    pub fn write_into(
        &self,
        out: &mut [u8],
    ) -> Result<usize, RemoteControlPairingMessageWriteError> {
        let encoded_len = self.encoded_len();
        let mut writer = PairingWireWriter::new(out, encoded_len)?;
        write_header(&mut writer, self.kind())?;
        match self {
            Self::Begin(begin) => {
                writer.write(&begin.controller.public_keys().public_key_bytes())?;
            }
            Self::Commit(commit) => writer.write(commit.transcript.as_bytes())?,
        }
        Ok(encoded_len)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum RemoteControlPairingResponse {
    Offer(RemoteControlPairingOffer),
    Completed(RemoteControlPairingCompleted),
}

impl RemoteControlPairingResponse {
    pub const MAX_ENCODED_LEN: usize =
        maximum(PAIRING_OFFER_MAX_ENCODED_LEN, PAIRING_COMPLETED_ENCODED_LEN);

    #[must_use]
    pub const fn kind(&self) -> RemoteControlPairingMessageKind {
        match self {
            Self::Offer(_) => RemoteControlPairingMessageKind::Offer,
            Self::Completed(_) => RemoteControlPairingMessageKind::Completed,
        }
    }

    #[must_use]
    pub fn encoded_len(&self) -> usize {
        match self {
            Self::Offer(offer) => offer.encoded_len(),
            Self::Completed(_) => PAIRING_COMPLETED_ENCODED_LEN,
        }
    }

    pub fn parse(bytes: &[u8]) -> Result<Self, RemoteControlPairingMessageParseError> {
        if bytes.len() > Self::MAX_ENCODED_LEN {
            return Err(RemoteControlPairingMessageParseError::TooLong {
                actual: bytes.len(),
                maximum: Self::MAX_ENCODED_LEN,
            });
        }
        let mut reader = PairingWireReader::new(bytes);
        let kind = read_header(&mut reader)?;
        let response = match kind {
            RemoteControlPairingMessageKind::Offer => {
                let target = RemoteControlTargetIdentity::new(read_identity_public_keys(
                    &mut reader,
                    RemoteControlPairingIdentityRole::Target,
                )?);
                let permissions = read_permissions(&mut reader)?;
                let attempt_timeout = read_attempt_timeout(&mut reader)?;
                let signature = Ed25519Signature(*reader.take()?);
                Self::Offer(RemoteControlPairingOffer {
                    target,
                    permissions,
                    attempt_timeout,
                    signature,
                })
            }
            RemoteControlPairingMessageKind::Completed => {
                let transcript = RemoteControlPairingTranscriptDigest(*reader.take()?);
                let signature = Ed25519Signature(*reader.take()?);
                Self::Completed(RemoteControlPairingCompleted {
                    transcript,
                    signature,
                })
            }
            RemoteControlPairingMessageKind::Begin | RemoteControlPairingMessageKind::Commit => {
                return Err(RemoteControlPairingMessageParseError::UnexpectedKind {
                    direction: RemoteControlPairingMessageDirection::Response,
                    found: kind,
                })
            }
        };
        reader.finish()?;
        Ok(response)
    }

    pub fn write_into(
        &self,
        out: &mut [u8],
    ) -> Result<usize, RemoteControlPairingMessageWriteError> {
        let encoded_len = self.encoded_len();
        let mut writer = PairingWireWriter::new(out, encoded_len)?;
        write_header(&mut writer, self.kind())?;
        match self {
            Self::Offer(offer) => {
                writer.write(&offer.target.public_keys().public_key_bytes())?;
                write_permissions(&mut writer, &offer.permissions)?;
                writer.write(&offer.attempt_timeout.to_wire())?;
                writer.write(&offer.signature.0)?;
            }
            Self::Completed(completed) => {
                writer.write(completed.transcript.as_bytes())?;
                writer.write(&completed.signature.0)?;
            }
        }
        Ok(encoded_len)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlPairingMessageParseError {
    TooLong {
        actual: usize,
        maximum: usize,
    },
    Truncated,
    UnsupportedVersion {
        found: u8,
    },
    UnknownKind {
        found: u8,
    },
    UnexpectedKind {
        direction: RemoteControlPairingMessageDirection,
        found: RemoteControlPairingMessageKind,
    },
    InvalidSigningPublicKey {
        role: RemoteControlPairingIdentityRole,
    },
    UnknownRequestKind {
        found: u8,
    },
    NonCanonicalPermissions,
    InvalidPermissions(RemoteControlPairingPermissionsError),
    InvalidAttemptTimeout(RemoteControlPairingAttemptTimeoutError),
    TrailingBytes {
        actual: usize,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlPairingMessageWriteError {
    BufferTooShort { required: usize, actual: usize },
}

fn read_header(
    reader: &mut PairingWireReader<'_>,
) -> Result<RemoteControlPairingMessageKind, RemoteControlPairingMessageParseError> {
    let &[version] = reader.take()?;
    if RemoteControlPairingProtocolVersion::from_wire(version).is_none() {
        return Err(RemoteControlPairingMessageParseError::UnsupportedVersion { found: version });
    }
    let &[kind] = reader.take()?;
    RemoteControlPairingMessageKind::from_wire(kind)
        .ok_or(RemoteControlPairingMessageParseError::UnknownKind { found: kind })
}

fn write_header(
    writer: &mut PairingWireWriter<'_>,
    kind: RemoteControlPairingMessageKind,
) -> Result<(), RemoteControlPairingMessageWriteError> {
    writer.write(&[RemoteControlPairingProtocolVersion::V1.wire_value()])?;
    writer.write(&[kind.wire_value()])
}

fn read_identity_public_keys(
    reader: &mut PairingWireReader<'_>,
    role: RemoteControlPairingIdentityRole,
) -> Result<IdentityPublicKeys, RemoteControlPairingMessageParseError> {
    let material = PublicIdentityMaterial::from_bytes(*reader.take()?);
    let public_keys = material.public_keys();
    Ed25519Verifier::new(public_keys.signing.as_ed25519())
        .map_err(|_| RemoteControlPairingMessageParseError::InvalidSigningPublicKey { role })?;
    Ok(public_keys)
}

fn read_permissions(
    reader: &mut PairingWireReader<'_>,
) -> Result<RemoteControlPairingPermissions, RemoteControlPairingMessageParseError> {
    let &[count] = reader.take()?;
    let kinds = reader.take_slice(usize::from(count))?;
    let mut requests = RemoteControlRequestSet::empty();
    let mut previous = None;
    for wire_value in kinds {
        let Some(kind) = RemoteControlRequestKind::from_wire(*wire_value) else {
            return Err(RemoteControlPairingMessageParseError::UnknownRequestKind {
                found: *wire_value,
            });
        };
        if previous.is_some_and(|value| value >= *wire_value) || !requests.insert(kind) {
            return Err(RemoteControlPairingMessageParseError::NonCanonicalPermissions);
        }
        previous = Some(*wire_value);
    }
    RemoteControlPairingPermissions::try_from(requests)
        .map_err(RemoteControlPairingMessageParseError::InvalidPermissions)
}

fn write_permissions(
    writer: &mut PairingWireWriter<'_>,
    permissions: &RemoteControlPairingPermissions,
) -> Result<(), RemoteControlPairingMessageWriteError> {
    writer.write(&[permission_count(permissions)])?;
    for kind in permissions.permitted_requests().iter() {
        writer.write(&[kind.wire_value()])?;
    }
    Ok(())
}

fn read_attempt_timeout(
    reader: &mut PairingWireReader<'_>,
) -> Result<RemoteControlPairingAttemptTimeout, RemoteControlPairingMessageParseError> {
    let millis = u32::from_le_bytes(*reader.take()?);
    RemoteControlPairingAttemptTimeout::try_from(DurationMillis(u64::from(millis)))
        .map_err(RemoteControlPairingMessageParseError::InvalidAttemptTimeout)
}

struct PairingWireReader<'a> {
    remaining: &'a [u8],
}

impl<'a> PairingWireReader<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { remaining: bytes }
    }

    fn take<const N: usize>(
        &mut self,
    ) -> Result<&'a [u8; N], RemoteControlPairingMessageParseError> {
        let Some((taken, remaining)) = self.remaining.split_first_chunk::<N>() else {
            return Err(RemoteControlPairingMessageParseError::Truncated);
        };
        self.remaining = remaining;
        Ok(taken)
    }

    fn take_slice(
        &mut self,
        len: usize,
    ) -> Result<&'a [u8], RemoteControlPairingMessageParseError> {
        let Some((taken, remaining)) = self.remaining.split_at_checked(len) else {
            return Err(RemoteControlPairingMessageParseError::Truncated);
        };
        self.remaining = remaining;
        Ok(taken)
    }

    fn finish(self) -> Result<(), RemoteControlPairingMessageParseError> {
        if self.remaining.is_empty() {
            return Ok(());
        }
        Err(RemoteControlPairingMessageParseError::TrailingBytes {
            actual: self.remaining.len(),
        })
    }
}

struct PairingWireWriter<'a> {
    remaining: &'a mut [u8],
}

impl<'a> PairingWireWriter<'a> {
    fn new(
        out: &'a mut [u8],
        required: usize,
    ) -> Result<Self, RemoteControlPairingMessageWriteError> {
        let actual = out.len();
        let Some(remaining) = out.get_mut(..required) else {
            return Err(RemoteControlPairingMessageWriteError::BufferTooShort { required, actual });
        };
        Ok(Self { remaining })
    }

    fn write(&mut self, bytes: &[u8]) -> Result<(), RemoteControlPairingMessageWriteError> {
        let remaining = core::mem::take(&mut self.remaining);
        let actual = remaining.len();
        let Some((target, tail)) = remaining.split_at_mut_checked(bytes.len()) else {
            return Err(RemoteControlPairingMessageWriteError::BufferTooShort {
                required: bytes.len(),
                actual,
            });
        };
        target.copy_from_slice(bytes);
        self.remaining = tail;
        Ok(())
    }
}

const fn maximum(left: usize, right: usize) -> usize {
    if left > right {
        left
    } else {
        right
    }
}
