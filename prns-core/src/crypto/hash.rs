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

/// One pass over the `prefix` chunks feeds both digests: the midstate is cloned, not rehashed.
/// Two independent hashes would walk the shared prefix twice, and callers pass resource-sized payloads.
pub fn sha256_prefix_and_digest_suffix(
    prefix: &[&[u8]],
    first_suffix: &[u8],
) -> SharedPrefixDigests {
    Sha256PrefixState::absorb(prefix).digests_with_suffix(first_suffix)
}

/// The absorbed prefix of [`sha256_prefix_and_digest_suffix`], held apart so one pass over the payload can serve many suffix attempts.
pub struct Sha256PrefixState {
    base: Sha256,
}

impl Sha256PrefixState {
    pub fn absorb(prefix: &[&[u8]]) -> Self {
        let mut base = Sha256::new();
        for chunk in prefix {
            base.update(chunk);
        }
        Self { base }
    }

    pub fn digests_with_suffix(&self, first_suffix: &[u8]) -> SharedPrefixDigests {
        let mut first = self.base.clone();
        first.update(first_suffix);
        let with_suffix: [u8; 32] = first.finalize().into();

        let mut second = self.base.clone();
        second.update(with_suffix);
        SharedPrefixDigests {
            with_suffix,
            with_first_digest: second.finalize().into(),
        }
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
        } = sha256_prefix_and_digest_suffix(&[b"shared ", b"payload"], b" salt");

        assert_eq!(with_suffix, sha256_chunks(&[b"shared payload", b" salt"]));
        assert_eq!(
            with_first_digest,
            sha256_chunks(&[b"shared payload", &with_suffix])
        );
    }
}
