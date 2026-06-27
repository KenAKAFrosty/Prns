//! The plain (non-msgpack) control plaintexts of the resource family: the
//! part request (context 0x03); the proof (0x05); and the two cancels,
//! initiator's (0x06) and receiver's (0x07). All but the proof seal under the
//! link key like any link traffic; the proof rides unencrypted as a
//! PROOF-type packet (two hashes, nothing to hide. RNS 1.3.1 "Resource
//! proofs are not encrypted")

use crate::routing::links::resources::{
    ResourceHash, ResourceProof, MAP_HASH_LEN, RESOURCE_HASH_LEN, WINDOW_MAX,
};

/// RNS 1.3.1 `Resource.HASHMAP_IS_EXHAUSTED` / `HASHMAP_IS_NOT_EXHAUSTED`:
/// the part request's first byte. The reference tests equality with 0xFF
/// only, so any other value reads as not-exhausted.
pub const HASHMAP_IS_EXHAUSTED: u8 = 0xFF;
pub const HASHMAP_IS_NOT_EXHAUSTED: u8 = 0x00;

/// The largest part request the protocol can express: flag, the exhausted
/// marker's map hash, the resource hash, and a full window of requested map hashes.
pub const PART_REQUEST_PLAINTEXT_CAP: usize =
    1 + MAP_HASH_LEN + RESOURCE_HASH_LEN + WINDOW_MAX * MAP_HASH_LEN;

/// A proof plaintext is exactly the resource hash and the proof hash;
/// the reference refuses any other length.
pub const PROOF_PLAINTEXT_LEN: usize = 2 * RESOURCE_HASH_LEN;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourcePartRequestError {
    TooManyMapHashes,
    RaggedMapHashes,
    BufferTooShort,
    Malformed,
}

/// `Resource.request_next`'s pack: `[flag]` ‖ `[last known map hash]` (only
/// when exhausted) ‖ resource hash ‖ the requested map hashes back to back.
/// `last_known_map_hash` set means the receiver ran past the map hashes it
/// holds and the sender owes a hashmap update naming the run after that mark.
pub fn write_part_request_plaintext(
    hash: &ResourceHash,
    last_known_map_hash: Option<&[u8; MAP_HASH_LEN]>,
    requested: &[u8],
    buf: &mut [u8],
) -> Result<usize, ResourcePartRequestError> {
    if requested.len() > WINDOW_MAX * MAP_HASH_LEN {
        return Err(ResourcePartRequestError::TooManyMapHashes);
    }
    if !requested.len().is_multiple_of(MAP_HASH_LEN) {
        return Err(ResourcePartRequestError::RaggedMapHashes);
    }
    let total =
        1 + last_known_map_hash.map_or(0, |_| MAP_HASH_LEN) + RESOURCE_HASH_LEN + requested.len();
    if buf.len() < total {
        return Err(ResourcePartRequestError::BufferTooShort);
    }
    let mut at = 1;
    match last_known_map_hash {
        None => buf[0] = HASHMAP_IS_NOT_EXHAUSTED,
        Some(mark) => {
            buf[0] = HASHMAP_IS_EXHAUSTED;
            buf[at..at + MAP_HASH_LEN].copy_from_slice(mark);
            at += MAP_HASH_LEN;
        }
    }
    buf[at..at + RESOURCE_HASH_LEN].copy_from_slice(hash.as_bytes());
    at += RESOURCE_HASH_LEN;
    buf[at..total].copy_from_slice(requested);
    Ok(total)
}

#[derive(Debug)]
pub struct ParsedPartRequest<'a> {
    pub hash: ResourceHash,
    pub last_known_map_hash: Option<[u8; MAP_HASH_LEN]>,
    pub requested: &'a [u8],
}

///  Only a 0xFF flag means exhausted,
/// the way the reference compares; a ragged tail of requested bytes is
/// tolerated and its remainder ignored, the way the reference's floor
/// division ignores it.
pub fn parse_part_request_plaintext(
    plaintext: &[u8],
) -> Result<ParsedPartRequest<'_>, ResourcePartRequestError> {
    parse_part_request_fields(plaintext).ok_or(ResourcePartRequestError::Malformed)
}

fn parse_part_request_fields(plaintext: &[u8]) -> Option<ParsedPartRequest<'_>> {
    let flag = *plaintext.first()?;
    let mut at = 1;
    let last_known_map_hash = if flag == HASHMAP_IS_EXHAUSTED {
        let mark = plaintext.get(at..at + MAP_HASH_LEN)?.try_into().ok()?;
        at += MAP_HASH_LEN;
        Some(mark)
    } else {
        None
    };
    let hash_bytes = plaintext.get(at..at + RESOURCE_HASH_LEN)?.try_into().ok()?;
    at += RESOURCE_HASH_LEN;
    Some(ParsedPartRequest {
        hash: ResourceHash::new(hash_bytes),
        last_known_map_hash,
        requested: &plaintext[at..],
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceProofError {
    BufferTooShort,
    Malformed,
}

/// `Resource.prove`'s pack: the resource hash, then
/// `Identity.full_hash(data + hash)`.
pub fn write_proof_plaintext(
    hash: &ResourceHash,
    proof: &ResourceProof,
    buf: &mut [u8],
) -> Result<usize, ResourceProofError> {
    if buf.len() < PROOF_PLAINTEXT_LEN {
        return Err(ResourceProofError::BufferTooShort);
    }
    buf[..RESOURCE_HASH_LEN].copy_from_slice(hash.as_bytes());
    buf[RESOURCE_HASH_LEN..PROOF_PLAINTEXT_LEN].copy_from_slice(proof.as_bytes());
    Ok(PROOF_PLAINTEXT_LEN)
}

/// `Resource.validate_proof`'s read: exactly two hashes or nothing — the
/// reference ignores proof data of any other length.
pub fn parse_proof_plaintext(
    plaintext: &[u8],
) -> Result<(ResourceHash, ResourceProof), ResourceProofError> {
    if plaintext.len() != PROOF_PLAINTEXT_LEN {
        return Err(ResourceProofError::Malformed);
    }
    let hash = plaintext[..RESOURCE_HASH_LEN]
        .try_into()
        .map_err(|_| ResourceProofError::Malformed)?;
    let proof = plaintext[RESOURCE_HASH_LEN..]
        .try_into()
        .map_err(|_| ResourceProofError::Malformed)?;
    Ok((ResourceHash::new(hash), ResourceProof::new(proof)))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceCancelError {
    BufferTooShort,
    Malformed,
}

/// Both cancel plaintexts (`RESOURCE_ICL` from the sending end,
/// `RESOURCE_RCL` from the receiving end) are just the resource hash; the
/// context byte tells them apart at the framing layer.
pub fn write_cancel_plaintext(
    hash: &ResourceHash,
    buf: &mut [u8],
) -> Result<usize, ResourceCancelError> {
    if buf.len() < RESOURCE_HASH_LEN {
        return Err(ResourceCancelError::BufferTooShort);
    }
    buf[..RESOURCE_HASH_LEN].copy_from_slice(hash.as_bytes());
    Ok(RESOURCE_HASH_LEN)
}

/// The reference slices the first 32 bytes and ignores anything after.
pub fn parse_cancel_plaintext(plaintext: &[u8]) -> Result<ResourceHash, ResourceCancelError> {
    let hash = plaintext
        .get(..RESOURCE_HASH_LEN)
        .ok_or(ResourceCancelError::Malformed)?
        .try_into()
        .map_err(|_| ResourceCancelError::Malformed)?;
    Ok(ResourceHash::new(hash))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routing::links::data::LINK_MDU;

    fn h() -> ResourceHash {
        let mut bytes = [0u8; RESOURCE_HASH_LEN];
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = i as u8;
        }
        ResourceHash::new(bytes)
    }

    #[test]
    fn a_part_request_lays_out_flag_hash_then_wanted_hashes() {
        let requested = [0x11, 0x12, 0x13, 0x14, 0x21, 0x22, 0x23, 0x24];
        let mut buf = [0u8; PART_REQUEST_PLAINTEXT_CAP];
        let n = write_part_request_plaintext(&h(), None, &requested, &mut buf).unwrap();
        assert_eq!(n, 1 + RESOURCE_HASH_LEN + 8);
        assert_eq!(buf[0], HASHMAP_IS_NOT_EXHAUSTED);
        assert_eq!(&buf[1..33], h().as_bytes());
        assert_eq!(&buf[33..n], &requested);

        let parsed = parse_part_request_plaintext(&buf[..n]).unwrap();
        assert_eq!(parsed.hash, h());
        assert_eq!(parsed.last_known_map_hash, None);
        assert_eq!(parsed.requested, &requested);
    }

    #[test]
    fn an_exhausted_request_carries_the_last_known_map_hash_first() {
        let mark = [0xAB, 0xCD, 0xEF, 0x01];
        let requested = [0x31, 0x32, 0x33, 0x34];
        let mut buf = [0u8; PART_REQUEST_PLAINTEXT_CAP];
        let n = write_part_request_plaintext(&h(), Some(&mark), &requested, &mut buf).unwrap();
        assert_eq!(n, 1 + MAP_HASH_LEN + RESOURCE_HASH_LEN + 4);
        assert_eq!(buf[0], HASHMAP_IS_EXHAUSTED);
        assert_eq!(&buf[1..5], &mark);
        assert_eq!(&buf[5..37], h().as_bytes());

        let parsed = parse_part_request_plaintext(&buf[..n]).unwrap();
        assert_eq!(parsed.hash, h());
        assert_eq!(parsed.last_known_map_hash, Some(mark));
        assert_eq!(parsed.requested, &requested);
    }

    #[test]
    fn only_0xff_reads_as_exhausted_the_way_the_reference_compares() {
        let mut buf = [0u8; PART_REQUEST_PLAINTEXT_CAP];
        let n = write_part_request_plaintext(&h(), None, &[1, 2, 3, 4], &mut buf).unwrap();
        buf[0] = 0x01;
        let parsed = parse_part_request_plaintext(&buf[..n]).unwrap();
        assert_eq!(parsed.last_known_map_hash, None);
        assert_eq!(parsed.hash, h());
    }

    #[test]
    fn ragged_requests_refuse_to_write_but_tolerate_on_parse() {
        let mut buf = [0u8; PART_REQUEST_PLAINTEXT_CAP];
        assert_eq!(
            write_part_request_plaintext(&h(), None, &[1, 2, 3], &mut buf).unwrap_err(),
            ResourcePartRequestError::RaggedMapHashes,
        );
        let too_many = [0u8; (WINDOW_MAX + 1) * MAP_HASH_LEN];
        assert_eq!(
            write_part_request_plaintext(&h(), None, &too_many, &mut buf).unwrap_err(),
            ResourcePartRequestError::TooManyMapHashes,
        );
        assert_eq!(
            write_part_request_plaintext(&h(), None, &[1, 2, 3, 4], &mut buf[..20]).unwrap_err(),
            ResourcePartRequestError::BufferTooShort,
        );

        let n = write_part_request_plaintext(&h(), None, &[1, 2, 3, 4], &mut buf).unwrap();
        let parsed = parse_part_request_plaintext(&buf[..n - 1]).unwrap();
        assert_eq!(parsed.requested, &[1, 2, 3]);
        assert_eq!(
            parse_part_request_plaintext(&buf[..20]).unwrap_err(),
            ResourcePartRequestError::Malformed,
        );
        assert!(parse_part_request_plaintext(&[]).is_err());
    }

    #[test]
    fn a_full_window_request_fits_the_base_link_mdu() {
        assert_eq!(PART_REQUEST_PLAINTEXT_CAP, 337);
        const { assert!(PART_REQUEST_PLAINTEXT_CAP <= LINK_MDU) };
    }

    #[test]
    fn the_proof_is_two_hashes_and_nothing_else() {
        let proof = ResourceProof::new([0xEE; RESOURCE_HASH_LEN]);
        let mut buf = [0u8; PROOF_PLAINTEXT_LEN];
        let n = write_proof_plaintext(&h(), &proof, &mut buf).unwrap();
        assert_eq!(n, 64);
        assert_eq!(&buf[..32], h().as_bytes());
        assert_eq!(&buf[32..], proof.as_bytes());

        let (parsed_hash, parsed_proof) = parse_proof_plaintext(&buf).unwrap();
        assert_eq!(parsed_hash, h());
        assert_eq!(parsed_proof, proof);
        assert_eq!(
            parse_proof_plaintext(&buf[..63]).unwrap_err(),
            ResourceProofError::Malformed,
        );
        let mut long = [0u8; 65];
        long[..64].copy_from_slice(&buf);
        assert_eq!(
            parse_proof_plaintext(&long).unwrap_err(),
            ResourceProofError::Malformed,
        );
        assert_eq!(
            write_proof_plaintext(&h(), &proof, &mut [0u8; 63]).unwrap_err(),
            ResourceProofError::BufferTooShort,
        );
    }

    #[test]
    fn cancels_are_the_bare_hash_with_trailing_bytes_ignored() {
        let mut buf = [0u8; 40];
        let n = write_cancel_plaintext(&h(), &mut buf).unwrap();
        assert_eq!(n, RESOURCE_HASH_LEN);
        assert_eq!(parse_cancel_plaintext(&buf[..n]).unwrap(), h());
        assert_eq!(parse_cancel_plaintext(&buf).unwrap(), h());
        assert_eq!(
            parse_cancel_plaintext(&buf[..31]).unwrap_err(),
            ResourceCancelError::Malformed,
        );
        assert_eq!(
            write_cancel_plaintext(&h(), &mut buf[..31]).unwrap_err(),
            ResourceCancelError::BufferTooShort,
        );
    }
}

#[cfg_attr(mutants, mutants::skip)]
#[cfg(kani)]
mod kani_proofs {
    use super::*;

    #[kani::proof]
    fn proof_plaintext_round_trips_for_any_hash_pair() {
        let hash = ResourceHash::new(kani::any());
        let proof = ResourceProof::new(kani::any());
        let mut buf = [0u8; PROOF_PLAINTEXT_LEN];

        assert_eq!(
            write_proof_plaintext(&hash, &proof, &mut buf),
            Ok(PROOF_PLAINTEXT_LEN)
        );
        assert_eq!(parse_proof_plaintext(&buf), Ok((hash, proof)));
    }

    #[kani::proof]
    fn cancel_plaintext_round_trips_for_any_resource_hash() {
        let hash = ResourceHash::new(kani::any());
        let mut buf = [0u8; RESOURCE_HASH_LEN];

        assert_eq!(
            write_cancel_plaintext(&hash, &mut buf),
            Ok(RESOURCE_HASH_LEN)
        );
        assert_eq!(parse_cancel_plaintext(&buf), Ok(hash));
    }
}
