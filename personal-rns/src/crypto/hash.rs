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
}
