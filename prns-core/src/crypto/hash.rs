use sha2::{Digest, Sha256};

pub fn sha256(data: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hasher.finalize().into()
}

pub fn sha256_chunks(chunks: &[&[u8]]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    for chunk in chunks {
        hasher.update(chunk);
    }
    hasher.finalize().into()
}

pub struct SharedPrefixDigests {
    pub with_suffix: [u8; 32],
    pub with_first_digest: [u8; 32],
}

/// One pass over `prefix` feeds both digests: the midstate is cloned, not rehashed.
/// Two independent hashes would walk the shared prefix twice, and callers pass resource-sized payloads.
pub fn sha256_prefix_and_digest_suffix(prefix: &[u8], first_suffix: &[u8]) -> SharedPrefixDigests {
    let mut base = Sha256::new();
    base.update(prefix);

    let mut first = base.clone();
    first.update(first_suffix);
    let with_suffix: [u8; 32] = first.finalize().into();

    base.update(with_suffix);
    SharedPrefixDigests {
        with_suffix,
        with_first_digest: base.finalize().into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunked_hashing_equals_hashing_the_concatenation() {
        let joined = sha256(b"personal-reticulum");
        let chunked = sha256_chunks(&[b"personal", b"-", b"reticulum"]);
        assert_eq!(joined, chunked);
        assert_eq!(sha256_chunks(&[b"personal-reticulum"]), joined);
    }

    #[test]
    fn prefix_and_digest_suffix_hashes_match_independent_hashes() {
        let SharedPrefixDigests {
            with_suffix,
            with_first_digest,
        } = sha256_prefix_and_digest_suffix(b"shared payload", b" salt");

        assert_eq!(with_suffix, sha256_chunks(&[b"shared payload", b" salt"]));
        assert_eq!(
            with_first_digest,
            sha256_chunks(&[b"shared payload", &with_suffix])
        );
    }
}
