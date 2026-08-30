//! [`assemble_incoming`](super::assemble_incoming)'s open, streamed along the receive window's consecutive frontier: every part that extends it is authenticated, decrypted, and (uncompressed) hash-absorbed as it lands, so a completed transfer concludes in constant work instead of a whole-transfer sweep after the last part.
//!
//! Wire-invisible, an intentional deviation in timing only: RNS 1.4.2 `Resource.assemble` walks the joined transfer whole, and every refusal here surfaces at the same conclusion with the same cause. No decrypted byte is released before the MAC verdict.

use crate::crypto::{Sha256PrefixState, TokenOpenStream};
use crate::routing::links::resources::assemble_incoming::{
    verify_absorbed_and_prove, verify_and_prove, OpenTransferError, VerifyResourceError,
};
use crate::routing::links::resources::{
    ResourceCompression, ResourceHash, ResourceProof, SaltNonce, RESOURCE_NONCE_LEN,
};
use crate::routing::links::LinkKey;

pub struct StreamedOpen {
    token: TokenOpenStream,
    /// The verify hash's running prefix over the decrypted stream past its nonce; `None` on a compressed row, whose hash covers the inflated data and verifies at the host seam.
    stream_digest: Option<Sha256PrefixState>,
    plaintext_seen_byte_len: usize,
}

impl core::fmt::Debug for StreamedOpen {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("StreamedOpen").finish_non_exhaustive()
    }
}

impl StreamedOpen {
    /// `None` when the sealed length is no token shape at all. The conclusion's whole-transfer fallback then refuses it with the stock `Malformed`.
    pub fn begin(key: &LinkKey, sealed: &[u8], compression: ResourceCompression) -> Option<Self> {
        let iv = sealed.get(..16)?.try_into().ok()?;
        let token = key.open_stream(iv, sealed.len()).ok()?;
        let stream_digest = match compression {
            ResourceCompression::Uncompressed => Some(Sha256PrefixState::absorb(&[])),
            ResourceCompression::Bz2 => None,
        };
        Some(Self {
            token,
            stream_digest,
            plaintext_seen_byte_len: 0,
        })
    }

    pub fn advance(&mut self, transfer: &mut [u8], contiguous_byte_len: usize) {
        let span = self.token.pending_span(contiguous_byte_len);
        self.chew_span(&mut transfer[span]);
    }

    /// The next chewable whole-block span in token coordinates; empty when the open has caught
    /// up with the frontier (or nothing has landed past it yet).
    pub fn pending_span(&self, contiguous_byte_len: usize) -> core::ops::Range<usize> {
        self.token.pending_span(contiguous_byte_len)
    }

    /// Whether [`conclude`](Self::conclude) has no chewing left to do — the offloaded lane's
    /// gate between finishing in constant work and parking for another span verdict.
    pub fn caught_up(&self) -> bool {
        self.token.fully_absorbed()
    }

    /// [`advance`](Self::advance) for a span carried away from its transfer — exactly the bytes
    /// [`pending_span`](Self::pending_span) named. This is the pool worker's whole job: the
    /// span decrypts in place here and the verdict copies it back over its ciphertext.
    pub fn chew_span(&mut self, span: &mut [u8]) {
        self.token.absorb_span(span);
        let nonce_still_owed = RESOURCE_NONCE_LEN.saturating_sub(self.plaintext_seen_byte_len);
        self.plaintext_seen_byte_len += span.len();
        let Some(digest) = &mut self.stream_digest else {
            return;
        };
        digest.update(&span[nonce_still_owed.min(span.len())..]);
    }

    /// [`open_transfer`](super::assemble_incoming::open_transfer)'s refusals in the same order, then the stream with everything absorbed so far banked for the verify.
    pub fn conclude(mut self, transfer: &mut [u8]) -> Result<OpenedStream<'_>, OpenTransferError> {
        self.advance(transfer, transfer.len());
        let plaintext = self
            .token
            .finalize(transfer)
            .map_err(OpenTransferError::Open)?;
        if plaintext.len() < RESOURCE_NONCE_LEN {
            return Err(OpenTransferError::StreamTooShort);
        }
        let stream = &plaintext[RESOURCE_NONCE_LEN..];
        let absorbed = self.stream_digest.map(|mut digest| {
            let digested = self
                .plaintext_seen_byte_len
                .saturating_sub(RESOURCE_NONCE_LEN)
                .min(stream.len());
            digest.update(&stream[digested..]);
            digest
        });
        Ok(OpenedStream { stream, absorbed })
    }
}

/// An incoming row's streamed-open slot across the offload round trip: the state parks here
/// between chews, and leaves a span marker behind while a pool worker holds it — the verdict's
/// identity check against a row that died or was replaced mid-chew.
// The large variant is the point of the column: the crypto midstates a fixed layout must size inline.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Default)]
pub enum OpenProgress {
    #[default]
    NotBegun,
    Parked(StreamedOpen),
    Chewing {
        dispatched: core::ops::Range<usize>,
    },
    ExternallyOpened {
        plaintext_byte_len: usize,
        verification: ExternalOpenVerification,
    },
}

#[derive(Debug)]
pub enum ExternalOpenVerification {
    Rehash,
    Verified(ResourceProof),
}

/// Who runs the chew — the runtime declares its capability once at construction.
/// `Inline` chews on the engine thread under each part arrival (the only choice without a
/// crypto pool); `PoolWhenContended` leaves spans parked for the runtime to walk through
/// [`owed_open_span`](crate::engine::EngineState::owed_open_span) and a worker's verdict,
/// but only while another open is live — a lone chew is one serial chain the pool cannot
/// overlap with anything, so its round trips buy nothing and the engine keeps it inline.
/// Either way the conclusion's catch-up keeps every row correct if no one chews.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ResourceOpenLane {
    #[default]
    Inline,
    PoolWhenContended,
    ExternalWhole,
}

/// A concluded transfer's nonce-stripped stream, carrying the verify midstate when one streamed in.
pub struct OpenedStream<'t> {
    pub stream: &'t [u8],
    absorbed: Option<Sha256PrefixState>,
}

impl<'t> OpenedStream<'t> {
    /// The whole-transfer fallback: no midstate banked, so the verify walks the stream whole.
    pub fn rehashing(stream: &'t [u8]) -> Self {
        Self {
            stream,
            absorbed: None,
        }
    }

    pub fn verify_and_prove(
        &self,
        salt_nonce: &SaltNonce,
        advertised: &ResourceHash,
    ) -> Result<ResourceProof, VerifyResourceError> {
        match &self.absorbed {
            Some(midstate) => verify_absorbed_and_prove(midstate, salt_nonce, advertised),
            None => verify_and_prove(self.stream, salt_nonce, advertised),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::{sha256, x25519_diffie_hellman, X25519PublicKey, X25519SecretKey};
    use crate::routing::links::resources::build_outgoing::{build_outgoing_resource, BuildRegions};
    use crate::routing::links::resources::{resource_sdu, ResourceBody, ResourceMetadata};
    use crate::routing::links::LinkId;
    use crate::wire::BROADCAST_MTU;

    fn link_key() -> LinkKey {
        let shared = x25519_diffie_hellman(
            &X25519SecretKey::new([0x33; 32]),
            &X25519PublicKey([0x55; 32]),
        );
        LinkKey::derive(&LinkId::new([0x07; 16]), &shared)
    }

    fn nonces() -> impl FnMut() -> [u8; RESOURCE_NONCE_LEN] {
        let mut drawn = 0;
        move || {
            drawn += 1;
            if drawn == 1 {
                [0x51, 0x52, 0x53, 0x54]
            } else {
                [0x61, 0x62, 0x63, 0x64]
            }
        }
    }

    fn payload() -> std::vec::Vec<u8> {
        let mut seed = sha256(b"streamed-open");
        let mut data = std::vec::Vec::new();
        for _ in 0..47 {
            data.extend_from_slice(&seed);
            seed = sha256(&seed);
        }
        data.truncate(1_400);
        data
    }

    fn built_transfer(
        body: &ResourceBody<'_>,
    ) -> (
        std::vec::Vec<u8>,
        crate::routing::links::resources::build_outgoing::BuiltResource,
        usize,
    ) {
        let mut transfer = std::vec![0u8; 4_096];
        let mut hashmap = std::vec![0u8; 64];
        let sdu = resource_sdu(BROADCAST_MTU);
        let built = build_outgoing_resource(
            body,
            &link_key(),
            &[0xA1; 16],
            nonces(),
            sdu,
            BuildRegions {
                transfer: &mut transfer,
                hashmap: &mut hashmap,
            },
        )
        .unwrap();
        transfer.truncate(built.sealed_transfer_bytes);
        (transfer, built, sdu)
    }

    #[test]
    fn an_uncompressed_transfer_streamed_part_by_part_verifies_off_the_midstate() {
        let data = payload();
        let (mut transfer, built, sdu) = built_transfer(&ResourceBody {
            data: &data,
            compressed_candidate: None,
            metadata: ResourceMetadata::None,
        });

        let mut open =
            StreamedOpen::begin(&link_key(), &transfer, ResourceCompression::Uncompressed).unwrap();
        for part_index in 0..built.part_count {
            let contiguous = ((part_index + 1) * sdu).min(transfer.len());
            open.advance(&mut transfer, contiguous);
        }
        let opened = open.conclude(&mut transfer).unwrap();
        assert_eq!(opened.stream, &data[..]);
        assert_eq!(
            opened
                .verify_and_prove(&built.salt_nonce, &built.hash)
                .unwrap(),
            built.expected_proof,
        );
    }

    #[test]
    fn a_stalled_frontier_that_jumps_at_the_end_still_verifies() {
        let data = payload();
        let (mut transfer, built, _) = built_transfer(&ResourceBody {
            data: &data,
            compressed_candidate: None,
            metadata: ResourceMetadata::None,
        });

        let mut open =
            StreamedOpen::begin(&link_key(), &transfer, ResourceCompression::Uncompressed).unwrap();
        open.advance(&mut transfer, 100);
        open.advance(&mut transfer, 100);
        let opened = open.conclude(&mut transfer).unwrap();
        assert_eq!(opened.stream, &data[..]);
        assert_eq!(
            opened
                .verify_and_prove(&built.salt_nonce, &built.hash)
                .unwrap(),
            built.expected_proof,
        );
    }

    #[test]
    fn a_compressed_transfer_streams_the_decrypt_and_yields_the_bz2_stream() {
        let data = payload();
        let candidate = b"pretend bz2, just visibly shorter".to_vec();
        let (mut transfer, built, sdu) = built_transfer(&ResourceBody {
            data: &data,
            compressed_candidate: Some(&candidate),
            metadata: ResourceMetadata::None,
        });

        let mut open =
            StreamedOpen::begin(&link_key(), &transfer, ResourceCompression::Bz2).unwrap();
        let contiguous = sdu.min(transfer.len());
        open.advance(&mut transfer, contiguous);
        let opened = open.conclude(&mut transfer).unwrap();
        assert_eq!(opened.stream, &candidate[..]);
        assert_eq!(built.compression, ResourceCompression::Bz2);
    }

    #[test]
    fn a_tampered_transfer_refuses_to_conclude() {
        let data = payload();
        let (mut transfer, _, _) = built_transfer(&ResourceBody {
            data: &data,
            compressed_candidate: None,
            metadata: ResourceMetadata::None,
        });
        *transfer.last_mut().unwrap() ^= 1;

        let mut open =
            StreamedOpen::begin(&link_key(), &transfer, ResourceCompression::Uncompressed).unwrap();
        open.advance(&mut transfer, usize::MAX);
        assert!(matches!(
            open.conclude(&mut transfer),
            Err(OpenTransferError::Open(_)),
        ));
    }

    #[test]
    fn a_buffer_too_short_for_any_token_never_begins() {
        assert!(
            StreamedOpen::begin(&link_key(), &[0u8; 12], ResourceCompression::Uncompressed,)
                .is_none()
        );
        assert!(
            StreamedOpen::begin(&link_key(), &[0u8; 63], ResourceCompression::Uncompressed,)
                .is_none()
        );
    }
}
