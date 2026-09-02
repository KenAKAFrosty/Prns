use core::fmt;
use core::num::NonZeroU32;

use crate::crypto::{ed25519_verify, sha256_chunks, Ed25519Signature, SHA256_OUTPUT_LEN};
use crate::identity::{IdentityHash, IdentityPublicKeys, IdentitySigner};
use crate::remote_control::{RemoteControlControllerIdentity, RemoteControlTargetIdentity};
use crate::routing::links::LinkId;
use crate::units::DurationMillis;

use super::super::{RemoteControlPairingEndpoint, RemoteControlPairingPermissions};
use super::{
    RemoteControlPairingInvitationCode, RemoteControlPairingInvitationProof,
    RemoteControlPairingProtocolVersion, MAX_REMOTE_CONTROL_PAIRING_ATTEMPT_TIMEOUT,
    PAIRING_ATTEMPT_TIMEOUT_ENCODED_LEN, PAIRING_COMPLETION_DOMAIN,
    PAIRING_CONFIRMATION_CODE_MODULUS, PAIRING_TRANSCRIPT_DIGEST_ENCODED_LEN,
    PAIRING_TRANSCRIPT_DOMAIN, PAIRING_TRANSCRIPT_PERMISSION_BYTES_LEN,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteControlPairingAttemptTimeoutError {
    Zero,
    TooLong {
        actual: DurationMillis,
        maximum: DurationMillis,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RemoteControlPairingAttemptTimeout(pub(super) NonZeroU32);

impl RemoteControlPairingAttemptTimeout {
    #[must_use]
    pub const fn duration(self) -> DurationMillis {
        DurationMillis(self.0.get() as u64)
    }

    pub(super) const fn to_wire(self) -> [u8; PAIRING_ATTEMPT_TIMEOUT_ENCODED_LEN] {
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
    pub(super) controller: RemoteControlControllerIdentity,
    pub(super) invitation_proof: RemoteControlPairingInvitationProof,
}

impl RemoteControlPairingBegin {
    #[must_use]
    pub fn new(
        controller: RemoteControlControllerIdentity,
        endpoint: RemoteControlPairingEndpoint,
        invitation_code: RemoteControlPairingInvitationCode,
    ) -> Self {
        let invitation_proof = invitation_code.into_proof(endpoint, &controller);
        Self {
            controller,
            invitation_proof,
        }
    }

    pub(super) const fn from_wire(
        controller: RemoteControlControllerIdentity,
        invitation_proof: RemoteControlPairingInvitationProof,
    ) -> Self {
        Self {
            controller,
            invitation_proof,
        }
    }

    #[must_use]
    pub const fn controller(&self) -> &RemoteControlControllerIdentity {
        &self.controller
    }

    #[must_use]
    pub const fn invitation_proof(&self) -> &RemoteControlPairingInvitationProof {
        &self.invitation_proof
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RemoteControlPairingTranscriptDigest(
    pub(super) [u8; super::PAIRING_TRANSCRIPT_DIGEST_ENCODED_LEN],
);

impl RemoteControlPairingTranscriptDigest {
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; PAIRING_TRANSCRIPT_DIGEST_ENCODED_LEN] {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RemoteControlPairingAttemptId(RemoteControlPairingTranscriptDigest);

impl RemoteControlPairingAttemptId {
    #[must_use]
    pub const fn transcript(self) -> RemoteControlPairingTranscriptDigest {
        self.0
    }

    #[must_use]
    pub const fn confirmation_code(self) -> RemoteControlPairingConfirmationCode {
        let [first, second, third, fourth, ..] = *self.0.as_bytes();
        RemoteControlPairingConfirmationCode(
            u32::from_le_bytes([first, second, third, fourth])
                .wrapping_rem(PAIRING_CONFIRMATION_CODE_MODULUS),
        )
    }
}

impl From<&RemoteControlPairingTranscript> for RemoteControlPairingAttemptId {
    fn from(transcript: &RemoteControlPairingTranscript) -> Self {
        Self(transcript.digest())
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
        RemoteControlPairingAttemptId::from(self).confirmation_code()
    }

    #[must_use]
    pub fn into_parts(
        self,
    ) -> (
        RemoteControlPairingContext,
        RemoteControlControllerIdentity,
        RemoteControlTargetIdentity,
        RemoteControlPairingPermissions,
        RemoteControlPairingAttemptTimeout,
    ) {
        (
            self.context,
            self.controller,
            self.target,
            self.permissions,
            self.attempt_timeout,
        )
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct RemoteControlPairingOffer {
    pub(super) target: RemoteControlTargetIdentity,
    pub(super) permissions: RemoteControlPairingPermissions,
    pub(super) attempt_timeout: RemoteControlPairingAttemptTimeout,
    pub(super) signature: Ed25519Signature,
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
        self.verify_controller(context, begin.controller())
    }

    pub(crate) fn verify_controller(
        &self,
        context: RemoteControlPairingContext,
        controller: &RemoteControlControllerIdentity,
    ) -> Result<RemoteControlPairingTranscript, RemoteControlPairingOfferVerificationError> {
        let transcript = RemoteControlPairingTranscript::new(
            context,
            *controller,
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

    pub(super) fn encoded_len(&self) -> usize {
        super::PAIRING_MESSAGE_HEADER_ENCODED_LEN
            .saturating_add(super::PAIRING_IDENTITY_ENCODED_LEN)
            .saturating_add(super::PAIRING_REQUEST_SET_COUNT_ENCODED_LEN)
            .saturating_add(self.permissions.permitted_requests().len())
            .saturating_add(PAIRING_ATTEMPT_TIMEOUT_ENCODED_LEN)
            .saturating_add(super::PAIRING_SIGNATURE_ENCODED_LEN)
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
    pub(super) transcript: RemoteControlPairingTranscriptDigest,
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
    pub(super) transcript: RemoteControlPairingTranscriptDigest,
    pub(super) signature: Ed25519Signature,
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

fn pairing_transcript_digest(
    context: RemoteControlPairingContext,
    controller: &RemoteControlControllerIdentity,
    target: &RemoteControlTargetIdentity,
    permissions: &RemoteControlPairingPermissions,
    attempt_timeout: RemoteControlPairingAttemptTimeout,
) -> RemoteControlPairingTranscriptDigest {
    let version = [RemoteControlPairingProtocolVersion::V2.wire_value()];
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

pub(super) fn permission_count(permissions: &RemoteControlPairingPermissions) -> u8 {
    permissions.permitted_requests().wire_count()
}
