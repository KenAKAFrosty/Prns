use crate::entropy::{EntropySource, RuntimeEntropy};
use crate::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};

pub type IdentitySecretKey = Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>;

pub const MAX_IDENTITY_LABEL_LEN: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityLabelError {
    Empty,
    TooLong,
    LeadingNonAlphanumeric,
    InvalidCharacter,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct IdentityLabel(heapless::String<MAX_IDENTITY_LABEL_LEN>);

impl IdentityLabel {
    pub fn new(label: &str) -> Result<Self, IdentityLabelError> {
        let bytes = label.as_bytes();
        let Some(&first) = bytes.first() else {
            return Err(IdentityLabelError::Empty);
        };
        if bytes.len() > MAX_IDENTITY_LABEL_LEN {
            return Err(IdentityLabelError::TooLong);
        }
        if !first.is_ascii_alphanumeric() {
            return Err(IdentityLabelError::LeadingNonAlphanumeric);
        }
        if bytes.iter().any(|byte| !is_label_byte(*byte)) {
            return Err(IdentityLabelError::InvalidCharacter);
        }
        let mut inline = heapless::String::new();
        inline
            .push_str(label)
            .map_err(|_| IdentityLabelError::TooLong)?;
        Ok(Self(inline))
    }

    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl core::fmt::Display for IdentityLabel {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl core::str::FromStr for IdentityLabel {
    type Err = IdentityLabelError;

    fn from_str(label: &str) -> Result<Self, Self::Err> {
        Self::new(label)
    }
}

impl TryFrom<&str> for IdentityLabel {
    type Error = IdentityLabelError;

    fn try_from(label: &str) -> Result<Self, Self::Error> {
        Self::new(label)
    }
}

pub fn is_label_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.')
}

/// Identity secrets are fixed 64-byte entries; blobs are variable-length secret records
/// (self-ratchet state) sharing the same trust domain and label namespace, so `remove`
/// clears either kind and a label addresses exactly one entry of one kind.
pub trait IdentityVault {
    type Error;

    fn load(&self, label: &IdentityLabel) -> Result<Option<IdentitySecretKey>, Self::Error>;

    fn store(
        &mut self,
        label: &IdentityLabel,
        secret: &[u8; IDENTITY_SECRET_KEY_LEN],
    ) -> Result<(), Self::Error>;

    fn remove(&mut self, label: &IdentityLabel) -> Result<Removal, Self::Error>;

    fn stored_blob_len(&self, label: &IdentityLabel) -> Result<Option<usize>, Self::Error>;

    /// A `buf` shorter than the stored blob is the impl's error, never a silent truncation.
    fn load_blob<'b>(
        &self,
        label: &IdentityLabel,
        buf: &'b mut [u8],
    ) -> Result<Option<&'b [u8]>, Self::Error>;

    fn store_blob(&mut self, label: &IdentityLabel, blob: &[u8]) -> Result<(), Self::Error>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Removal {
    Removed,
    NothingStored,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityOrigin {
    Loaded,
    Generated,
}

/// Loads an identity or generates it from an initialized runtime random stream.
///
/// Arbitrary callbacks cannot cross this bootstrap boundary.
///
/// ```compile_fail
/// use prns_core::identity::vault::{
///     load_or_generate_with_runtime_entropy, IdentityLabel, IdentityVault,
/// };
///
/// fn bootstrap<V: IdentityVault>(vault: &mut V, label: &IdentityLabel) {
///     let mut callback = |output: &mut [u8]| output.fill(0x42);
///     let _ = load_or_generate_with_runtime_entropy(vault, label, &mut callback);
/// }
/// ```
pub fn load_or_generate_with_runtime_entropy<V, S>(
    vault: &mut V,
    label: &IdentityLabel,
    entropy: &mut RuntimeEntropy<S>,
) -> Result<(IdentitySecretKey, IdentityOrigin), V::Error>
where
    V: IdentityVault,
    S: EntropySource,
{
    load_or_generate_inner(vault, label, |output| entropy.fill_random(output))
}

fn load_or_generate_inner<V: IdentityVault>(
    vault: &mut V,
    label: &IdentityLabel,
    mut fill_random: impl FnMut(&mut [u8]),
) -> Result<(IdentitySecretKey, IdentityOrigin), V::Error> {
    if let Some(secret) = vault.load(label)? {
        return Ok((secret, IdentityOrigin::Loaded));
    }
    let mut secret = Zeroizing::new([0u8; IDENTITY_SECRET_KEY_LEN]);
    fill_random(&mut secret[..]);
    vault.store(label, &secret)?;
    Ok((secret, IdentityOrigin::Generated))
}

#[cfg(test)]
mod tests {
    use core::convert::Infallible;

    use super::*;
    use std::collections::HashMap;

    #[derive(Default)]
    struct MemoryVault {
        entries: HashMap<String, [u8; IDENTITY_SECRET_KEY_LEN]>,
        blobs: HashMap<String, Vec<u8>>,
        fail_store: bool,
    }

    #[derive(Debug, PartialEq, Eq)]
    enum MemoryVaultError {
        StoreRefused,
    }

    impl IdentityVault for MemoryVault {
        type Error = MemoryVaultError;

        fn load(
            &self,
            label: &IdentityLabel,
        ) -> Result<Option<Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>>, Self::Error> {
            Ok(self
                .entries
                .get(label.as_str())
                .map(|bytes| Zeroizing::new(*bytes)))
        }

        fn store(
            &mut self,
            label: &IdentityLabel,
            secret: &[u8; IDENTITY_SECRET_KEY_LEN],
        ) -> Result<(), Self::Error> {
            if self.fail_store {
                return Err(MemoryVaultError::StoreRefused);
            }
            self.entries.insert(label.as_str().to_owned(), *secret);
            Ok(())
        }

        fn remove(&mut self, label: &IdentityLabel) -> Result<Removal, Self::Error> {
            let entry = self.entries.remove(label.as_str());
            let blob = self.blobs.remove(label.as_str());
            Ok(match (entry, blob) {
                (None, None) => Removal::NothingStored,
                _ => Removal::Removed,
            })
        }

        fn stored_blob_len(&self, label: &IdentityLabel) -> Result<Option<usize>, Self::Error> {
            Ok(self.blobs.get(label.as_str()).map(Vec::len))
        }

        fn load_blob<'b>(
            &self,
            label: &IdentityLabel,
            buf: &'b mut [u8],
        ) -> Result<Option<&'b [u8]>, Self::Error> {
            let Some(blob) = self.blobs.get(label.as_str()) else {
                return Ok(None);
            };
            buf[..blob.len()].copy_from_slice(blob);
            Ok(Some(&buf[..blob.len()]))
        }

        fn store_blob(&mut self, label: &IdentityLabel, blob: &[u8]) -> Result<(), Self::Error> {
            if self.fail_store {
                return Err(MemoryVaultError::StoreRefused);
            }
            self.blobs.insert(label.as_str().to_owned(), blob.to_vec());
            Ok(())
        }
    }

    fn label(text: &str) -> IdentityLabel {
        IdentityLabel::new(text).unwrap()
    }

    struct TestEntropySource(u8);

    impl EntropySource for TestEntropySource {
        type Error = Infallible;

        fn try_fill_entropy(&mut self, output: &mut [u8]) -> Result<(), Self::Error> {
            output.fill(self.0);
            Ok(())
        }
    }

    fn runtime_entropy(seed: u8) -> RuntimeEntropy<TestEntropySource> {
        RuntimeEntropy::try_new(TestEntropySource(seed)).unwrap()
    }

    fn generated_secret(seed: u8) -> [u8; IDENTITY_SECRET_KEY_LEN] {
        let mut entropy = runtime_entropy(seed);
        let mut secret = [0_u8; IDENTITY_SECRET_KEY_LEN];
        entropy.fill_random(&mut secret);
        secret
    }

    #[test]
    fn a_plain_label_round_trips_through_validation() {
        let parsed = IdentityLabel::new("lxmf-primary.v2").unwrap();
        assert_eq!(parsed.as_str(), "lxmf-primary.v2");
    }

    #[test]
    fn an_empty_label_is_rejected() {
        assert_eq!(IdentityLabel::new(""), Err(IdentityLabelError::Empty));
    }

    #[test]
    fn a_label_past_the_ceiling_is_rejected() {
        let too_long = "a".repeat(MAX_IDENTITY_LABEL_LEN + 1);
        assert_eq!(
            IdentityLabel::new(&too_long),
            Err(IdentityLabelError::TooLong)
        );
        let at_ceiling = "a".repeat(MAX_IDENTITY_LABEL_LEN);
        assert!(IdentityLabel::new(&at_ceiling).is_ok());
    }

    #[test]
    fn a_label_must_start_alphanumeric_so_it_cannot_traverse_or_inject() {
        assert_eq!(
            IdentityLabel::new("..").unwrap_err(),
            IdentityLabelError::LeadingNonAlphanumeric,
        );
        assert_eq!(
            IdentityLabel::new(".hidden").unwrap_err(),
            IdentityLabelError::LeadingNonAlphanumeric,
        );
        assert_eq!(
            IdentityLabel::new("-rf").unwrap_err(),
            IdentityLabelError::LeadingNonAlphanumeric,
        );
    }

    #[test]
    fn a_label_with_a_path_separator_is_an_invalid_character() {
        assert_eq!(
            IdentityLabel::new("a/b").unwrap_err(),
            IdentityLabelError::InvalidCharacter,
        );
        assert_eq!(
            IdentityLabel::new("a b").unwrap_err(),
            IdentityLabelError::InvalidCharacter,
        );
    }

    #[test]
    fn a_miss_generates_persists_and_reports_generated() {
        let mut vault = MemoryVault::default();
        let label = label("primary");
        let mut entropy = runtime_entropy(0x10);
        let (secret, origin) =
            load_or_generate_with_runtime_entropy(&mut vault, &label, &mut entropy).unwrap();
        assert_eq!(origin, IdentityOrigin::Generated);
        assert_eq!(*secret, generated_secret(0x10));
        assert!(vault.load(&label).unwrap().is_some());
    }

    #[test]
    fn a_second_call_loads_without_advancing_runtime_entropy() {
        let mut vault = MemoryVault::default();
        let label = label("primary");
        let mut initial_entropy = runtime_entropy(0x10);
        let (first, _) =
            load_or_generate_with_runtime_entropy(&mut vault, &label, &mut initial_entropy)
                .unwrap();
        let mut load_entropy = runtime_entropy(0xFF);
        let mut untouched_entropy = runtime_entropy(0xFF);
        let (second, origin) =
            load_or_generate_with_runtime_entropy(&mut vault, &label, &mut load_entropy).unwrap();
        assert_eq!(origin, IdentityOrigin::Loaded);
        assert_eq!(*first, *second);

        let mut after_load = [0_u8; 32];
        let mut untouched = [0_u8; 32];
        load_entropy.fill_random(&mut after_load);
        untouched_entropy.fill_random(&mut untouched);
        assert_eq!(after_load, untouched);
    }

    #[test]
    fn distinct_labels_keep_distinct_identities() {
        let mut vault = MemoryVault::default();
        let mut entropy = runtime_entropy(0x21);
        let (transport, _) =
            load_or_generate_with_runtime_entropy(&mut vault, &label("transport"), &mut entropy)
                .unwrap();
        let (lxmf, _) =
            load_or_generate_with_runtime_entropy(&mut vault, &label("lxmf"), &mut entropy)
                .unwrap();
        assert_ne!(*transport, *lxmf);
    }

    #[test]
    fn a_store_failure_on_a_fresh_identity_propagates() {
        let mut vault = MemoryVault {
            fail_store: true,
            ..MemoryVault::default()
        };
        let mut entropy = runtime_entropy(0x10);
        let outcome =
            load_or_generate_with_runtime_entropy(&mut vault, &label("primary"), &mut entropy);
        assert_eq!(outcome.unwrap_err(), MemoryVaultError::StoreRefused);
    }

    #[test]
    fn remove_reports_whether_anything_was_held() {
        let mut vault = MemoryVault::default();
        let label = label("primary");
        load_or_generate_with_runtime_entropy(&mut vault, &label, &mut runtime_entropy(0x10))
            .unwrap();
        assert_eq!(vault.remove(&label).unwrap(), Removal::Removed);
        assert_eq!(vault.remove(&label).unwrap(), Removal::NothingStored);
    }
}
