use core::fmt;
use core::num::NonZeroU32;

use crate::crypto::{
    ed25519_verify, sha256_chunks, Ed25519Signature, Ed25519Verifier, SHA256_OUTPUT_LEN,
};
use crate::identity::{
    IdentityHash, IdentityPublicKeys, IdentitySigner, PublicIdentityMaterial,
    IDENTITY_PUBLIC_KEY_LEN,
};
use crate::remote_control::{
    RemoteControlControllerIdentity, RemoteControlRequestKind, RemoteControlRequestSet,
    RemoteControlTargetIdentity,
};
use crate::routing::links::LinkId;
use crate::units::DurationMillis;

use super::{
    RemoteControlPairingEndpoint, RemoteControlPairingPermissions,
    RemoteControlPairingPermissionsError, MAX_REMOTE_CONTROL_PAIRING_EXPIRES_AFTER,
};

pub const REMOTE_CONTROL_PAIRING_REQUEST_ENDPOINT_ID: &str = "/pair";
pub const MAX_REMOTE_CONTROL_PAIRING_ATTEMPT_TIMEOUT: DurationMillis =
    MAX_REMOTE_CONTROL_PAIRING_EXPIRES_AFTER;

const PAIRING_MESSAGE_HEADER_ENCODED_LEN: usize = 2;
const PAIRING_IDENTITY_ENCODED_LEN: usize = IDENTITY_PUBLIC_KEY_LEN;
const PAIRING_REQUEST_SET_COUNT_ENCODED_LEN: usize = 1;
const PAIRING_ATTEMPT_TIMEOUT_ENCODED_LEN: usize = 4;
const PAIRING_TRANSCRIPT_DIGEST_ENCODED_LEN: usize = SHA256_OUTPUT_LEN;
const PAIRING_SIGNATURE_ENCODED_LEN: usize = Ed25519Signature::LEN;
const PAIRING_BEGIN_ENCODED_LEN: usize =
    PAIRING_MESSAGE_HEADER_ENCODED_LEN.saturating_add(PAIRING_IDENTITY_ENCODED_LEN);
const PAIRING_COMMIT_ENCODED_LEN: usize =
    PAIRING_MESSAGE_HEADER_ENCODED_LEN.saturating_add(PAIRING_TRANSCRIPT_DIGEST_ENCODED_LEN);
const PAIRING_OFFER_MAX_ENCODED_LEN: usize = PAIRING_MESSAGE_HEADER_ENCODED_LEN
    .saturating_add(PAIRING_IDENTITY_ENCODED_LEN)
    .saturating_add(PAIRING_REQUEST_SET_COUNT_ENCODED_LEN)
    .saturating_add(RemoteControlRequestKind::ALL.len())
    .saturating_add(PAIRING_ATTEMPT_TIMEOUT_ENCODED_LEN)
    .saturating_add(PAIRING_SIGNATURE_ENCODED_LEN);
const PAIRING_COMPLETED_ENCODED_LEN: usize = PAIRING_MESSAGE_HEADER_ENCODED_LEN
    .saturating_add(PAIRING_TRANSCRIPT_DIGEST_ENCODED_LEN)
    .saturating_add(PAIRING_SIGNATURE_ENCODED_LEN);
const PAIRING_TRANSCRIPT_PERMISSION_BYTES_LEN: usize =
    PAIRING_REQUEST_SET_COUNT_ENCODED_LEN.saturating_add(RemoteControlRequestKind::ALL.len());
const PAIRING_CONFIRMATION_CODE_MODULUS: u32 = 1_000_000;
const PAIRING_TRANSCRIPT_DOMAIN: &[u8] = b"reticulum.remote.control.pairing.transcript.v1";
const PAIRING_COMPLETION_DOMAIN: &[u8] = b"reticulum.remote.control.pairing.completed.v1";

const _: () = assert!(RemoteControlRequestKind::ALL.len() <= u8::MAX as usize);
const _: () = assert!(MAX_REMOTE_CONTROL_PAIRING_ATTEMPT_TIMEOUT.0 <= u32::MAX as u64);

prns_macros::iterable_enum! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    #[repr(u8)]
    pub enum RemoteControlPairingProtocolVersion {
        V1 = 1,
    }
}

impl RemoteControlPairingProtocolVersion {
    #[must_use]
    pub const fn wire_value(self) -> u8 {
        self as u8
    }

    const fn from_wire(value: u8) -> Option<Self> {
        match value {
            1 => Some(Self::V1),
            _ => None,
        }
    }
}

prns_macros::iterable_enum! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    #[repr(u8)]
    pub enum RemoteControlPairingMessageKind {
        Begin = 1,
        Offer = 2,
        Commit = 3,
        Completed = 4,
    }
}

impl RemoteControlPairingMessageKind {
    #[must_use]
    pub const fn wire_value(self) -> u8 {
        self as u8
    }

    const fn from_wire(value: u8) -> Option<Self> {
        match value {
            1 => Some(Self::Begin),
            2 => Some(Self::Offer),
            3 => Some(Self::Commit),
            4 => Some(Self::Completed),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlPairingMessageDirection {
    Request,
    Response,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlPairingIdentityRole {
    Controller,
    Target,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlPairingAttemptTimeoutError {
    Zero,
    TooLong {
        actual: DurationMillis,
        maximum: DurationMillis,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteControlPairingAttemptTimeout(NonZeroU32);

impl RemoteControlPairingAttemptTimeout {
    #[must_use]
    pub const fn duration(self) -> DurationMillis {
        DurationMillis(self.0.get() as u64)
    }

    const fn to_wire(self) -> [u8; PAIRING_ATTEMPT_TIMEOUT_ENCODED_LEN] {
        self.0.get().to_le_bytes()
    }
}

impl TryFrom<DurationMillis> for RemoteControlPairingAttemptTimeout {
    type Error = RemoteControlPairingAttemptTimeoutError;

    fn try_from(duration: DurationMillis) -> Result<Self, Self::Error> {
        if duration > MAX_REMOTE_CONTROL_PAIRING_ATTEMPT_TIMEOUT {
            return Err(RemoteControlPairingAttemptTimeoutError::TooLong {
                actual: duration,
                maximum: MAX_REMOTE_CONTROL_PAIRING_ATTEMPT_TIMEOUT,
            });
        }
        let Some(millis) = NonZeroU32::new(duration.0 as u32) else {
            return Err(RemoteControlPairingAttemptTimeoutError::Zero);
        };
        Ok(Self(millis))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteControlPairingContext {
    endpoint: RemoteControlPairingEndpoint,
    link_id: LinkId,
}

impl RemoteControlPairingContext {
    #[must_use]
    pub const fn new(endpoint: RemoteControlPairingEndpoint, link_id: LinkId) -> Self {
        Self { endpoint, link_id }
    }

    #[must_use]
    pub const fn endpoint(self) -> RemoteControlPairingEndpoint {
        self.endpoint
    }

    #[must_use]
    pub const fn link_id(self) -> LinkId {
        self.link_id
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct RemoteControlPairingBegin {
    controller: RemoteControlControllerIdentity,
}

impl RemoteControlPairingBegin {
    #[must_use]
    pub const fn new(controller: RemoteControlControllerIdentity) -> Self {
        Self { controller }
    }

    #[must_use]
    pub const fn controller(&self) -> &RemoteControlControllerIdentity {
        &self.controller
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RemoteControlPairingTranscriptDigest([u8; PAIRING_TRANSCRIPT_DIGEST_ENCODED_LEN]);

impl RemoteControlPairingTranscriptDigest {
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; PAIRING_TRANSCRIPT_DIGEST_ENCODED_LEN] {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteControlPairingConfirmationCode(u32);

impl RemoteControlPairingConfirmationCode {
    pub const DIGIT_COUNT: usize = 6;

    #[must_use]
    pub const fn value(self) -> u32 {
        self.0
    }
}

impl fmt::Display for RemoteControlPairingConfirmationCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:06}", self.0)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct RemoteControlPairingTranscript {
    context: RemoteControlPairingContext,
    controller: RemoteControlControllerIdentity,
    target: RemoteControlTargetIdentity,
    permissions: RemoteControlPairingPermissions,
    attempt_timeout: RemoteControlPairingAttemptTimeout,
    digest: RemoteControlPairingTranscriptDigest,
}

impl RemoteControlPairingTranscript {
    fn new(
        context: RemoteControlPairingContext,
        controller: RemoteControlControllerIdentity,
        target: RemoteControlTargetIdentity,
        permissions: RemoteControlPairingPermissions,
        attempt_timeout: RemoteControlPairingAttemptTimeout,
    ) -> Self {
        let digest =
            pairing_transcript_digest(context, &controller, &target, &permissions, attempt_timeout);
        Self {
            context,
            controller,
            target,
            permissions,
            attempt_timeout,
            digest,
        }
    }

    #[must_use]
    pub const fn context(&self) -> RemoteControlPairingContext {
        self.context
    }

    #[must_use]
    pub const fn controller(&self) -> &RemoteControlControllerIdentity {
        &self.controller
    }

    #[must_use]
    pub const fn target(&self) -> &RemoteControlTargetIdentity {
        &self.target
    }

    #[must_use]
    pub const fn permissions(&self) -> &RemoteControlPairingPermissions {
        &self.permissions
    }

    #[must_use]
    pub const fn attempt_timeout(&self) -> RemoteControlPairingAttemptTimeout {
        self.attempt_timeout
    }

    #[must_use]
    pub const fn digest(&self) -> RemoteControlPairingTranscriptDigest {
        self.digest
    }

    #[must_use]
    pub fn confirmation_code(&self) -> RemoteControlPairingConfirmationCode {
        let [first, second, third, fourth, ..] = self.digest.0;
        RemoteControlPairingConfirmationCode(
            u32::from_le_bytes([first, second, third, fourth])
                .wrapping_rem(PAIRING_CONFIRMATION_CODE_MODULUS),
        )
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct RemoteControlPairingOffer {
    target: RemoteControlTargetIdentity,
    permissions: RemoteControlPairingPermissions,
    attempt_timeout: RemoteControlPairingAttemptTimeout,
    signature: Ed25519Signature,
}

impl RemoteControlPairingOffer {
    #[must_use]
    pub const fn target(&self) -> &RemoteControlTargetIdentity {
        &self.target
    }

    #[must_use]
    pub const fn permissions(&self) -> &RemoteControlPairingPermissions {
        &self.permissions
    }

    #[must_use]
    pub const fn attempt_timeout(&self) -> RemoteControlPairingAttemptTimeout {
        self.attempt_timeout
    }

    #[must_use]
    pub const fn signature(&self) -> &Ed25519Signature {
        &self.signature
    }

    pub fn verify(
        &self,
        context: RemoteControlPairingContext,
        begin: &RemoteControlPairingBegin,
    ) -> Result<RemoteControlPairingTranscript, RemoteControlPairingOfferVerificationError> {
        let transcript = RemoteControlPairingTranscript::new(
            context,
            *begin.controller(),
            RemoteControlTargetIdentity::new(*self.target.public_keys()),
            self.permissions.clone(),
            self.attempt_timeout,
        );
        ed25519_verify(
            self.target.public_keys().signing.as_ed25519(),
            transcript.digest.as_bytes(),
            &self.signature,
        )
        .map_err(|_| RemoteControlPairingOfferVerificationError::InvalidTargetSignature)?;
        Ok(transcript)
    }

    fn encoded_len(&self) -> usize {
        PAIRING_MESSAGE_HEADER_ENCODED_LEN
            .saturating_add(PAIRING_IDENTITY_ENCODED_LEN)
            .saturating_add(PAIRING_REQUEST_SET_COUNT_ENCODED_LEN)
            .saturating_add(self.permissions.permitted_requests().len())
            .saturating_add(PAIRING_ATTEMPT_TIMEOUT_ENCODED_LEN)
            .saturating_add(PAIRING_SIGNATURE_ENCODED_LEN)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct RemoteControlPairingPreparedOffer {
    offer: RemoteControlPairingOffer,
    transcript: RemoteControlPairingTranscript,
}

impl RemoteControlPairingPreparedOffer {
    #[must_use]
    pub fn new(
        target_signer: &impl IdentitySigner,
        context: RemoteControlPairingContext,
        begin: &RemoteControlPairingBegin,
        permissions: RemoteControlPairingPermissions,
        attempt_timeout: RemoteControlPairingAttemptTimeout,
    ) -> Self {
        let target_public_keys = IdentityPublicKeys {
            encryption: target_signer.encryption_public_key(),
            signing: target_signer.signing_public_key(),
        };
        let transcript = RemoteControlPairingTranscript::new(
            context,
            *begin.controller(),
            RemoteControlTargetIdentity::new(target_public_keys),
            permissions.clone(),
            attempt_timeout,
        );
        let offer = RemoteControlPairingOffer {
            target: RemoteControlTargetIdentity::new(target_public_keys),
            permissions,
            attempt_timeout,
            signature: target_signer.sign(transcript.digest.as_bytes()),
        };
        Self { offer, transcript }
    }

    #[must_use]
    pub const fn offer(&self) -> &RemoteControlPairingOffer {
        &self.offer
    }

    #[must_use]
    pub const fn transcript(&self) -> &RemoteControlPairingTranscript {
        &self.transcript
    }

    #[must_use]
    pub fn into_parts(self) -> (RemoteControlPairingOffer, RemoteControlPairingTranscript) {
        (self.offer, self.transcript)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlPairingOfferVerificationError {
    InvalidTargetSignature,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteControlPairingCommit {
    transcript: RemoteControlPairingTranscriptDigest,
}

impl RemoteControlPairingCommit {
    #[must_use]
    pub const fn new(transcript: &RemoteControlPairingTranscript) -> Self {
        Self {
            transcript: transcript.digest,
        }
    }

    #[must_use]
    pub const fn transcript(&self) -> RemoteControlPairingTranscriptDigest {
        self.transcript
    }

    #[must_use]
    pub fn matches(&self, transcript: &RemoteControlPairingTranscript) -> bool {
        self.transcript.0 == transcript.digest.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlPairingCompletionSigningError {
    TargetIdentityMismatch {
        expected: IdentityHash,
        found: IdentityHash,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlPairingCompletedVerificationError {
    TranscriptMismatch {
        expected: RemoteControlPairingTranscriptDigest,
        found: RemoteControlPairingTranscriptDigest,
    },
    InvalidTargetSignature,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteControlPairingCompleted {
    transcript: RemoteControlPairingTranscriptDigest,
    signature: Ed25519Signature,
}

impl RemoteControlPairingCompleted {
    pub fn signed_by(
        target_signer: &impl IdentitySigner,
        transcript: &RemoteControlPairingTranscript,
    ) -> Result<Self, RemoteControlPairingCompletionSigningError> {
        let signer_public_keys = IdentityPublicKeys {
            encryption: target_signer.encryption_public_key(),
            signing: target_signer.signing_public_key(),
        };
        let expected = transcript.target.identity_hash();
        let found = signer_public_keys.identity_hash();
        if signer_public_keys != *transcript.target.public_keys() {
            return Err(
                RemoteControlPairingCompletionSigningError::TargetIdentityMismatch {
                    expected,
                    found,
                },
            );
        }
        let message = pairing_completion_signature_message(transcript.digest);
        Ok(Self {
            transcript: transcript.digest,
            signature: target_signer.sign(&message),
        })
    }

    #[must_use]
    pub const fn transcript(&self) -> RemoteControlPairingTranscriptDigest {
        self.transcript
    }

    #[must_use]
    pub const fn signature(&self) -> &Ed25519Signature {
        &self.signature
    }

    pub fn verify(
        &self,
        transcript: &RemoteControlPairingTranscript,
    ) -> Result<(), RemoteControlPairingCompletedVerificationError> {
        if self.transcript != transcript.digest {
            return Err(
                RemoteControlPairingCompletedVerificationError::TranscriptMismatch {
                    expected: transcript.digest,
                    found: self.transcript,
                },
            );
        }
        let message = pairing_completion_signature_message(self.transcript);
        ed25519_verify(
            transcript.target.public_keys().signing.as_ed25519(),
            &message,
            &self.signature,
        )
        .map_err(|_| RemoteControlPairingCompletedVerificationError::InvalidTargetSignature)
    }
}

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

fn pairing_transcript_digest(
    context: RemoteControlPairingContext,
    controller: &RemoteControlControllerIdentity,
    target: &RemoteControlTargetIdentity,
    permissions: &RemoteControlPairingPermissions,
    attempt_timeout: RemoteControlPairingAttemptTimeout,
) -> RemoteControlPairingTranscriptDigest {
    let version = [RemoteControlPairingProtocolVersion::V1.wire_value()];
    let endpoint = context.endpoint.destination_hash();
    let controller_public_keys = controller.public_keys().public_key_bytes();
    let target_public_keys = target.public_keys().public_key_bytes();
    let permission_bytes = transcript_permission_bytes(permissions);
    let attempt_timeout = attempt_timeout.to_wire();
    RemoteControlPairingTranscriptDigest(sha256_chunks(&[
        PAIRING_TRANSCRIPT_DOMAIN,
        &version,
        endpoint.as_bytes(),
        context.link_id.as_bytes(),
        &controller_public_keys,
        &target_public_keys,
        &permission_bytes,
        &attempt_timeout,
    ]))
}

fn pairing_completion_signature_message(
    transcript: RemoteControlPairingTranscriptDigest,
) -> [u8; SHA256_OUTPUT_LEN] {
    sha256_chunks(&[PAIRING_COMPLETION_DOMAIN, transcript.as_bytes()])
}

fn transcript_permission_bytes(
    permissions: &RemoteControlPairingPermissions,
) -> [u8; PAIRING_TRANSCRIPT_PERMISSION_BYTES_LEN] {
    let mut bytes = [0u8; PAIRING_TRANSCRIPT_PERMISSION_BYTES_LEN];
    let Some((count, kinds)) = bytes.split_first_mut() else {
        return bytes;
    };
    *count = permission_count(permissions);
    for (out, kind) in kinds
        .iter_mut()
        .zip(permissions.permitted_requests().iter())
    {
        *out = kind.wire_value();
    }
    bytes
}

fn permission_count(permissions: &RemoteControlPairingPermissions) -> u8 {
    permissions.permitted_requests().wire_count()
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

#[cfg(test)]
mod tests {
    #![allow(
        clippy::expect_used,
        clippy::indexing_slicing,
        clippy::panic,
        clippy::unwrap_used
    )]

    use super::*;
    use crate::identity::in_memory::InMemoryNodeIdentity;
    use crate::identity::vault::IdentitySecretKey;
    use crate::remote_control::RemoteControlPairingIdentity;
    use crate::wire::TRUNCATED_HASH_BYTE_LEN;
    use proptest::prelude::*;

    struct PairingFixture {
        target_signer: InMemoryNodeIdentity,
        context: RemoteControlPairingContext,
        begin: RemoteControlPairingBegin,
    }

    impl PairingFixture {
        fn new() -> Self {
            Self {
                target_signer: signer(0x52),
                context: context(0x73, 0x84),
                begin: RemoteControlPairingBegin::new(controller(0x31)),
            }
        }

        fn prepared_offer(&self) -> RemoteControlPairingPreparedOffer {
            RemoteControlPairingPreparedOffer::new(
                &self.target_signer,
                self.context,
                &self.begin,
                permissions(RemoteControlRequestSet::only(
                    RemoteControlRequestKind::Describe,
                )),
                attempt_timeout(30_000),
            )
        }
    }

    fn signer(fill: u8) -> InMemoryNodeIdentity {
        InMemoryNodeIdentity::from_secret_key_bytes(&IdentitySecretKey::new(
            [fill; crate::identity::IDENTITY_SECRET_KEY_LEN],
        ))
    }

    fn controller(fill: u8) -> RemoteControlControllerIdentity {
        let signer = signer(fill);
        RemoteControlControllerIdentity::new(IdentityPublicKeys {
            encryption: signer.encryption_public_key(),
            signing: signer.signing_public_key(),
        })
    }

    fn context(endpoint_fill: u8, link_fill: u8) -> RemoteControlPairingContext {
        RemoteControlPairingContext::new(
            RemoteControlPairingIdentity::new(IdentityHash::new(
                [endpoint_fill; TRUNCATED_HASH_BYTE_LEN],
            ))
            .endpoint(),
            LinkId::new([link_fill; TRUNCATED_HASH_BYTE_LEN]),
        )
    }

    fn permissions(requests: RemoteControlRequestSet) -> RemoteControlPairingPermissions {
        RemoteControlPairingPermissions::try_from(requests).unwrap()
    }

    fn attempt_timeout(millis: u64) -> RemoteControlPairingAttemptTimeout {
        RemoteControlPairingAttemptTimeout::try_from(DurationMillis(millis)).unwrap()
    }

    fn encoded_request(request: &RemoteControlPairingRequest) -> std::vec::Vec<u8> {
        let mut encoded = std::vec![0u8; request.encoded_len()];
        assert_eq!(request.write_into(&mut encoded), Ok(encoded.len()));
        encoded
    }

    fn encoded_response(response: &RemoteControlPairingResponse) -> std::vec::Vec<u8> {
        let mut encoded = std::vec![0u8; response.encoded_len()];
        assert_eq!(response.write_into(&mut encoded), Ok(encoded.len()));
        encoded
    }

    fn parsed_offer(bytes: &[u8]) -> RemoteControlPairingOffer {
        let RemoteControlPairingResponse::Offer(offer) =
            RemoteControlPairingResponse::parse(bytes).unwrap()
        else {
            panic!("expected an offer")
        };
        offer
    }

    #[test]
    fn pairing_exchange_discriminants_and_bounds_are_stable() {
        assert_eq!(REMOTE_CONTROL_PAIRING_REQUEST_ENDPOINT_ID, "/pair");
        assert_eq!(
            RemoteControlPairingProtocolVersion::ALL,
            [RemoteControlPairingProtocolVersion::V1],
        );
        assert_eq!(
            RemoteControlPairingMessageKind::ALL,
            [
                RemoteControlPairingMessageKind::Begin,
                RemoteControlPairingMessageKind::Offer,
                RemoteControlPairingMessageKind::Commit,
                RemoteControlPairingMessageKind::Completed,
            ],
        );
        assert_eq!(RemoteControlPairingProtocolVersion::V1.wire_value(), 1);
        assert_eq!(RemoteControlPairingMessageKind::Begin.wire_value(), 1);
        assert_eq!(RemoteControlPairingMessageKind::Offer.wire_value(), 2);
        assert_eq!(RemoteControlPairingMessageKind::Commit.wire_value(), 3);
        assert_eq!(RemoteControlPairingMessageKind::Completed.wire_value(), 4);
        assert_eq!(RemoteControlPairingRequest::MAX_ENCODED_LEN, 66);
        assert_eq!(RemoteControlPairingResponse::MAX_ENCODED_LEN, 137);
        const {
            assert!(
                RemoteControlPairingRequest::MAX_ENCODED_LEN
                    <= crate::engine::MAX_SEND_REQUEST_DATA_LEN
            );
            assert!(
                RemoteControlPairingResponse::MAX_ENCODED_LEN
                    <= crate::engine::MAX_RESPOND_DATA_LEN
            );
        }
    }

    #[test]
    fn attempt_timeouts_are_positive_bounded_wire_values() {
        assert_eq!(
            RemoteControlPairingAttemptTimeout::try_from(DurationMillis(0)),
            Err(RemoteControlPairingAttemptTimeoutError::Zero),
        );
        let maximum = RemoteControlPairingAttemptTimeout::try_from(
            MAX_REMOTE_CONTROL_PAIRING_ATTEMPT_TIMEOUT,
        )
        .unwrap();
        assert_eq!(
            maximum.duration(),
            MAX_REMOTE_CONTROL_PAIRING_ATTEMPT_TIMEOUT
        );
        let too_long = DurationMillis(
            MAX_REMOTE_CONTROL_PAIRING_ATTEMPT_TIMEOUT
                .0
                .saturating_add(1),
        );
        assert_eq!(
            RemoteControlPairingAttemptTimeout::try_from(too_long),
            Err(RemoteControlPairingAttemptTimeoutError::TooLong {
                actual: too_long,
                maximum: MAX_REMOTE_CONTROL_PAIRING_ATTEMPT_TIMEOUT,
            }),
        );
    }

    #[test]
    fn begin_and_commit_requests_round_trip_at_their_exact_lengths() {
        let fixture = PairingFixture::new();
        let begin = RemoteControlPairingRequest::Begin(fixture.begin);
        let mut begin_out = [0xA5; RemoteControlPairingRequest::MAX_ENCODED_LEN + 1];
        assert_eq!(begin.write_into(&mut begin_out), Ok(begin.encoded_len()));
        assert_eq!(begin_out[begin.encoded_len()], 0xA5);
        assert_eq!(
            RemoteControlPairingRequest::parse(&begin_out[..begin.encoded_len()]),
            Ok(begin),
        );

        let fixture = PairingFixture::new();
        let prepared = fixture.prepared_offer();
        let commit = RemoteControlPairingRequest::Commit(RemoteControlPairingCommit::new(
            prepared.transcript(),
        ));
        let encoded = encoded_request(&commit);
        assert_eq!(encoded.len(), PAIRING_COMMIT_ENCODED_LEN);
        assert_eq!(RemoteControlPairingRequest::parse(&encoded), Ok(commit));
    }

    #[test]
    fn a_signed_offer_round_trips_to_the_same_transcript_and_code() {
        let fixture = PairingFixture::new();
        let prepared = fixture.prepared_offer();
        let expected_code = prepared.transcript().confirmation_code();
        let (offer, expected_transcript) = prepared.into_parts();
        let response = RemoteControlPairingResponse::Offer(offer);
        let encoded = encoded_response(&response);
        let parsed = parsed_offer(&encoded);
        let verified = parsed.verify(fixture.context, &fixture.begin).unwrap();

        assert_eq!(verified, expected_transcript);
        assert_eq!(verified.confirmation_code(), expected_code);
        assert!(expected_code.value() < PAIRING_CONFIRMATION_CODE_MODULUS);
        let displayed = std::format!("{expected_code}");
        assert_eq!(
            displayed.len(),
            RemoteControlPairingConfirmationCode::DIGIT_COUNT
        );
        assert!(displayed.bytes().all(|byte| byte.is_ascii_digit()));
    }

    #[test]
    fn an_offer_with_every_permission_fills_the_reported_response_bound() {
        let fixture = PairingFixture::new();
        let prepared = RemoteControlPairingPreparedOffer::new(
            &fixture.target_signer,
            fixture.context,
            &fixture.begin,
            permissions(RemoteControlRequestSet::all()),
            attempt_timeout(30_000),
        );
        let (offer, expected_transcript) = prepared.into_parts();
        let response = RemoteControlPairingResponse::Offer(offer);
        assert_eq!(
            response.encoded_len(),
            RemoteControlPairingResponse::MAX_ENCODED_LEN
        );
        let encoded = encoded_response(&response);
        let parsed = parsed_offer(&encoded);
        assert_eq!(
            parsed.verify(fixture.context, &fixture.begin),
            Ok(expected_transcript),
        );
    }

    #[test]
    fn the_transcript_and_confirmation_code_have_a_pinned_vector() {
        let fixture = PairingFixture::new();
        let prepared = fixture.prepared_offer();

        assert_eq!(
            prepared.transcript().digest().as_bytes(),
            &[
                0xa8, 0x56, 0x69, 0x47, 0xc2, 0xce, 0x4d, 0x86, 0x40, 0xa4, 0x29, 0x85, 0x56, 0x2b,
                0x0d, 0xbc, 0x47, 0x01, 0x40, 0xf0, 0xc2, 0xb4, 0xa5, 0xdc, 0x24, 0x41, 0xb0, 0x3e,
                0xfc, 0xe0, 0x70, 0x31,
            ],
        );
        assert_eq!(prepared.transcript().confirmation_code().value(), 85_800);
    }

    #[test]
    fn every_offer_transcript_fact_is_covered_by_the_target_signature() {
        let fixture = PairingFixture::new();
        let prepared = fixture.prepared_offer();
        let encoded = encoded_response(&RemoteControlPairingResponse::Offer(
            prepared.into_parts().0,
        ));

        let parsed = parsed_offer(&encoded);
        assert_eq!(
            parsed.verify(context(0x74, 0x84), &fixture.begin),
            Err(RemoteControlPairingOfferVerificationError::InvalidTargetSignature),
        );
        let parsed = parsed_offer(&encoded);
        assert_eq!(
            parsed.verify(context(0x73, 0x85), &fixture.begin),
            Err(RemoteControlPairingOfferVerificationError::InvalidTargetSignature),
        );
        let parsed = parsed_offer(&encoded);
        assert_eq!(
            parsed.verify(
                fixture.context,
                &RemoteControlPairingBegin::new(controller(0x32)),
            ),
            Err(RemoteControlPairingOfferVerificationError::InvalidTargetSignature),
        );

        for (offset, mask) in [
            (PAIRING_MESSAGE_HEADER_ENCODED_LEN, 0x01),
            (
                PAIRING_MESSAGE_HEADER_ENCODED_LEN + PAIRING_IDENTITY_ENCODED_LEN + 1,
                RemoteControlRequestKind::Describe.wire_value()
                    ^ RemoteControlRequestKind::AnnounceSelf.wire_value(),
            ),
            (
                PAIRING_MESSAGE_HEADER_ENCODED_LEN
                    + PAIRING_IDENTITY_ENCODED_LEN
                    + PAIRING_REQUEST_SET_COUNT_ENCODED_LEN
                    + 1,
                0x01,
            ),
            (encoded.len() - 1, 0x01),
        ] {
            let mut tampered = encoded.clone();
            tampered[offset] ^= mask;
            let offer = parsed_offer(&tampered);
            assert_eq!(
                offer.verify(fixture.context, &fixture.begin),
                Err(RemoteControlPairingOfferVerificationError::InvalidTargetSignature),
            );
        }
    }

    #[test]
    fn commit_and_completed_bind_the_exact_durably_committed_transcript() {
        let fixture = PairingFixture::new();
        let prepared = fixture.prepared_offer();
        let transcript = prepared.transcript();
        let commit = RemoteControlPairingCommit::new(transcript);
        assert!(commit.matches(transcript));

        let alternate = RemoteControlPairingPreparedOffer::new(
            &fixture.target_signer,
            context(0x73, 0x85),
            &fixture.begin,
            permissions(RemoteControlRequestSet::only(
                RemoteControlRequestKind::Describe,
            )),
            attempt_timeout(30_000),
        );
        assert!(!commit.matches(alternate.transcript()));

        let completed =
            RemoteControlPairingCompleted::signed_by(&fixture.target_signer, transcript).unwrap();
        assert_eq!(completed.verify(transcript), Ok(()));
        assert!(matches!(
            RemoteControlPairingCompleted::signed_by(&signer(0x53), transcript),
            Err(RemoteControlPairingCompletionSigningError::TargetIdentityMismatch { .. }),
        ));

        let response = RemoteControlPairingResponse::Completed(completed);
        let encoded = encoded_response(&response);
        let RemoteControlPairingResponse::Completed(parsed) =
            RemoteControlPairingResponse::parse(&encoded).unwrap()
        else {
            panic!("expected completed")
        };
        assert_eq!(parsed.verify(transcript), Ok(()));

        let mut wrong_transcript = encoded.clone();
        wrong_transcript[PAIRING_MESSAGE_HEADER_ENCODED_LEN] ^= 0x01;
        let RemoteControlPairingResponse::Completed(parsed) =
            RemoteControlPairingResponse::parse(&wrong_transcript).unwrap()
        else {
            panic!("expected completed")
        };
        assert!(matches!(
            parsed.verify(transcript),
            Err(RemoteControlPairingCompletedVerificationError::TranscriptMismatch { .. }),
        ));

        let mut wrong_signature = encoded;
        let last = wrong_signature.len() - 1;
        wrong_signature[last] ^= 0x01;
        let RemoteControlPairingResponse::Completed(parsed) =
            RemoteControlPairingResponse::parse(&wrong_signature).unwrap()
        else {
            panic!("expected completed")
        };
        assert_eq!(
            parsed.verify(transcript),
            Err(RemoteControlPairingCompletedVerificationError::InvalidTargetSignature),
        );
    }

    #[test]
    fn parsers_reject_wrong_directions_versions_kinds_lengths_and_noncanonical_fields() {
        let version = RemoteControlPairingProtocolVersion::V1.wire_value();
        assert_eq!(
            RemoteControlPairingRequest::parse(&[
                version,
                RemoteControlPairingMessageKind::Offer.wire_value(),
            ]),
            Err(RemoteControlPairingMessageParseError::UnexpectedKind {
                direction: RemoteControlPairingMessageDirection::Request,
                found: RemoteControlPairingMessageKind::Offer,
            }),
        );
        assert_eq!(
            RemoteControlPairingResponse::parse(&[
                version,
                RemoteControlPairingMessageKind::Begin.wire_value(),
            ]),
            Err(RemoteControlPairingMessageParseError::UnexpectedKind {
                direction: RemoteControlPairingMessageDirection::Response,
                found: RemoteControlPairingMessageKind::Begin,
            }),
        );
        assert_eq!(
            RemoteControlPairingRequest::parse(&[0x7F, 1]),
            Err(RemoteControlPairingMessageParseError::UnsupportedVersion { found: 0x7F }),
        );
        assert_eq!(
            RemoteControlPairingRequest::parse(&[version, 0x7F]),
            Err(RemoteControlPairingMessageParseError::UnknownKind { found: 0x7F }),
        );
        assert_eq!(
            RemoteControlPairingRequest::parse(&std::vec![
                0;
                RemoteControlPairingRequest::MAX_ENCODED_LEN + 1
            ]),
            Err(RemoteControlPairingMessageParseError::TooLong {
                actual: RemoteControlPairingRequest::MAX_ENCODED_LEN + 1,
                maximum: RemoteControlPairingRequest::MAX_ENCODED_LEN,
            }),
        );

        let fixture = PairingFixture::new();
        let begin = RemoteControlPairingRequest::Begin(fixture.begin);
        let mut invalid_controller = encoded_request(&begin);
        let signing_key_offset =
            PAIRING_MESSAGE_HEADER_ENCODED_LEN + crate::crypto::X25519PublicKey::LEN;
        let invalid_signing_key = invalid_controller
            .get_mut(signing_key_offset..signing_key_offset + crate::crypto::Ed25519PublicKey::LEN)
            .unwrap();
        let mut rejected_key = [0u8; crate::crypto::Ed25519PublicKey::LEN];
        rejected_key[1] = 3;
        invalid_signing_key.copy_from_slice(&rejected_key);
        assert_eq!(
            RemoteControlPairingRequest::parse(&invalid_controller),
            Err(
                RemoteControlPairingMessageParseError::InvalidSigningPublicKey {
                    role: RemoteControlPairingIdentityRole::Controller,
                },
            ),
        );

        let fixture = PairingFixture::new();
        let prepared = fixture.prepared_offer();
        let mut invalid_target = encoded_response(&RemoteControlPairingResponse::Offer(
            prepared.into_parts().0,
        ));
        let target_signing_key_offset =
            PAIRING_MESSAGE_HEADER_ENCODED_LEN + crate::crypto::X25519PublicKey::LEN;
        invalid_target[target_signing_key_offset
            ..target_signing_key_offset + crate::crypto::Ed25519PublicKey::LEN]
            .copy_from_slice(&rejected_key);
        assert_eq!(
            RemoteControlPairingResponse::parse(&invalid_target),
            Err(
                RemoteControlPairingMessageParseError::InvalidSigningPublicKey {
                    role: RemoteControlPairingIdentityRole::Target,
                },
            ),
        );

        let fixture = PairingFixture::new();
        let prepared = fixture.prepared_offer();
        let mut encoded = encoded_response(&RemoteControlPairingResponse::Offer(
            prepared.into_parts().0,
        ));
        let permission_count_offset =
            PAIRING_MESSAGE_HEADER_ENCODED_LEN + PAIRING_IDENTITY_ENCODED_LEN;
        encoded.insert(
            permission_count_offset + PAIRING_REQUEST_SET_COUNT_ENCODED_LEN + 1,
            RemoteControlRequestKind::Describe.wire_value(),
        );
        encoded[permission_count_offset] = 2;
        assert_eq!(
            RemoteControlPairingResponse::parse(&encoded),
            Err(RemoteControlPairingMessageParseError::NonCanonicalPermissions),
        );

        let fixture = PairingFixture::new();
        let prepared = fixture.prepared_offer();
        let mut unknown_permission = encoded_response(&RemoteControlPairingResponse::Offer(
            prepared.into_parts().0,
        ));
        unknown_permission[permission_count_offset + 1] = 0x7F;
        assert_eq!(
            RemoteControlPairingResponse::parse(&unknown_permission),
            Err(RemoteControlPairingMessageParseError::UnknownRequestKind { found: 0x7F }),
        );

        let fixture = PairingFixture::new();
        let prepared = fixture.prepared_offer();
        let mut empty_permissions = encoded_response(&RemoteControlPairingResponse::Offer(
            prepared.into_parts().0,
        ));
        empty_permissions.remove(permission_count_offset + 1);
        empty_permissions[permission_count_offset] = 0;
        assert_eq!(
            RemoteControlPairingResponse::parse(&empty_permissions),
            Err(RemoteControlPairingMessageParseError::InvalidPermissions(
                RemoteControlPairingPermissionsError::NoPermittedRequests,
            )),
        );

        let fixture = PairingFixture::new();
        let prepared = fixture.prepared_offer();
        let mut zero_timeout = encoded_response(&RemoteControlPairingResponse::Offer(
            prepared.into_parts().0,
        ));
        let timeout_offset = permission_count_offset + 2;
        zero_timeout[timeout_offset..timeout_offset + PAIRING_ATTEMPT_TIMEOUT_ENCODED_LEN].fill(0);
        assert_eq!(
            RemoteControlPairingResponse::parse(&zero_timeout),
            Err(
                RemoteControlPairingMessageParseError::InvalidAttemptTimeout(
                    RemoteControlPairingAttemptTimeoutError::Zero,
                )
            ),
        );

        let fixture = PairingFixture::new();
        let prepared = fixture.prepared_offer();
        let mut excessive_timeout = encoded_response(&RemoteControlPairingResponse::Offer(
            prepared.into_parts().0,
        ));
        let excessive = u32::try_from(
            MAX_REMOTE_CONTROL_PAIRING_ATTEMPT_TIMEOUT
                .0
                .saturating_add(1),
        )
        .unwrap()
        .to_le_bytes();
        excessive_timeout[timeout_offset..timeout_offset + PAIRING_ATTEMPT_TIMEOUT_ENCODED_LEN]
            .copy_from_slice(&excessive);
        assert_eq!(
            RemoteControlPairingResponse::parse(&excessive_timeout),
            Err(
                RemoteControlPairingMessageParseError::InvalidAttemptTimeout(
                    RemoteControlPairingAttemptTimeoutError::TooLong {
                        actual: DurationMillis(u64::from(u32::from_le_bytes(excessive))),
                        maximum: MAX_REMOTE_CONTROL_PAIRING_ATTEMPT_TIMEOUT,
                    },
                ),
            ),
        );
    }

    #[test]
    fn every_truncated_prefix_and_short_writer_buffer_is_rejected() {
        let fixture = PairingFixture::new();
        let begin = RemoteControlPairingRequest::Begin(fixture.begin);
        let begin_bytes = encoded_request(&begin);
        for length in 0..begin_bytes.len() {
            assert!(RemoteControlPairingRequest::parse(&begin_bytes[..length]).is_err());
            let mut out = std::vec![0u8; length];
            assert_eq!(
                begin.write_into(&mut out),
                Err(RemoteControlPairingMessageWriteError::BufferTooShort {
                    required: begin.encoded_len(),
                    actual: length,
                }),
            );
        }

        let fixture = PairingFixture::new();
        let prepared = fixture.prepared_offer();
        let offer = RemoteControlPairingResponse::Offer(prepared.into_parts().0);
        let offer_bytes = encoded_response(&offer);
        for length in 0..offer_bytes.len() {
            assert!(RemoteControlPairingResponse::parse(&offer_bytes[..length]).is_err());
            let mut out = std::vec![0u8; length];
            assert_eq!(
                offer.write_into(&mut out),
                Err(RemoteControlPairingMessageWriteError::BufferTooShort {
                    required: offer.encoded_len(),
                    actual: length,
                }),
            );
        }
    }

    #[test]
    fn valid_shorter_messages_reject_trailing_bytes() {
        let fixture = PairingFixture::new();
        let prepared = fixture.prepared_offer();
        let commit = RemoteControlPairingRequest::Commit(RemoteControlPairingCommit::new(
            prepared.transcript(),
        ));
        let mut commit_bytes = encoded_request(&commit);
        commit_bytes.push(0xA5);
        assert_eq!(
            RemoteControlPairingRequest::parse(&commit_bytes),
            Err(RemoteControlPairingMessageParseError::TrailingBytes { actual: 1 }),
        );

        let offer = RemoteControlPairingResponse::Offer(prepared.into_parts().0);
        let mut offer_bytes = encoded_response(&offer);
        offer_bytes.push(0xA5);
        assert_eq!(
            RemoteControlPairingResponse::parse(&offer_bytes),
            Err(RemoteControlPairingMessageParseError::TrailingBytes { actual: 1 }),
        );
    }

    proptest! {
        #[test]
        fn every_successfully_parsed_pairing_request_round_trips(
            bytes in proptest::collection::vec(
                any::<u8>(),
                0..=RemoteControlPairingRequest::MAX_ENCODED_LEN + 1,
            ),
        ) {
            if let Ok(request) = RemoteControlPairingRequest::parse(&bytes) {
                let encoded = encoded_request(&request);
                prop_assert_eq!(RemoteControlPairingRequest::parse(&encoded), Ok(request));
            }
        }

        #[test]
        fn every_successfully_parsed_pairing_response_round_trips(
            bytes in proptest::collection::vec(
                any::<u8>(),
                0..=RemoteControlPairingResponse::MAX_ENCODED_LEN + 1,
            ),
        ) {
            if let Ok(response) = RemoteControlPairingResponse::parse(&bytes) {
                let encoded = encoded_response(&response);
                prop_assert_eq!(RemoteControlPairingResponse::parse(&encoded), Ok(response));
            }
        }
    }
}

#[cfg_attr(mutants, mutants::skip)]
#[cfg(kani)]
mod kani_proofs {
    use super::*;

    #[kani::proof]
    #[kani::unwind(8)]
    fn pairing_request_parse_terminates_for_every_bounded_wire_shape() {
        let bytes: [u8; RemoteControlPairingRequest::MAX_ENCODED_LEN + 1] = kani::any();
        let len: usize = kani::any();
        kani::assume(len <= bytes.len());
        let _result = RemoteControlPairingRequest::parse(&bytes[..len]);
    }

    #[kani::proof]
    #[kani::unwind(8)]
    fn pairing_response_parse_terminates_for_every_bounded_wire_shape() {
        let bytes: [u8; RemoteControlPairingResponse::MAX_ENCODED_LEN + 1] = kani::any();
        let len: usize = kani::any();
        kani::assume(len <= bytes.len());
        let _result = RemoteControlPairingResponse::parse(&bytes[..len]);
    }
}
