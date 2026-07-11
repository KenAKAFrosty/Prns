use crate::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};

mod impls;
pub use impls::*;

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

fn is_label_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.')
}

pub trait IdentityVault {
    type Error;

    fn load(&self, label: &IdentityLabel) -> Result<Option<IdentitySecretKey>, Self::Error>;

    fn store(
        &mut self,
        label: &IdentityLabel,
        secret: &[u8; IDENTITY_SECRET_KEY_LEN],
    ) -> Result<(), Self::Error>;

    fn remove(&mut self, label: &IdentityLabel) -> Result<bool, Self::Error>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentityOrigin {
    Loaded,
    Generated,
}

pub fn load_or_generate<V: IdentityVault>(
    vault: &mut V,
    label: &IdentityLabel,
    mut fill_entropy: impl FnMut(&mut [u8]),
) -> Result<(IdentitySecretKey, IdentityOrigin), V::Error> {
    if let Some(secret) = vault.load(label)? {
        return Ok((secret, IdentityOrigin::Loaded));
    }
    let mut secret = Zeroizing::new([0u8; IDENTITY_SECRET_KEY_LEN]);
    fill_entropy(&mut secret[..]);
    vault.store(label, &secret)?;
    Ok((secret, IdentityOrigin::Generated))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[derive(Default)]
    struct MemoryVault {
        entries: HashMap<String, [u8; IDENTITY_SECRET_KEY_LEN]>,
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

        fn remove(&mut self, label: &IdentityLabel) -> Result<bool, Self::Error> {
            Ok(self.entries.remove(label.as_str()).is_some())
        }
    }

    fn label(text: &str) -> IdentityLabel {
        IdentityLabel::new(text).unwrap()
    }

    fn counting_entropy(seed: u8) -> impl FnMut(&mut [u8]) {
        move |bytes| {
            for (offset, byte) in bytes.iter_mut().enumerate() {
                *byte = seed.wrapping_add(offset as u8);
            }
        }
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
        let (secret, origin) =
            load_or_generate(&mut vault, &label, counting_entropy(0x10)).unwrap();
        assert_eq!(origin, IdentityOrigin::Generated);
        assert_eq!(secret[0], 0x10);
        assert_eq!(secret[63], 0x10u8.wrapping_add(63));
        assert!(vault.load(&label).unwrap().is_some());
    }

    #[test]
    fn a_second_call_loads_the_same_bytes_and_reports_loaded() {
        let mut vault = MemoryVault::default();
        let label = label("primary");
        let (first, _) = load_or_generate(&mut vault, &label, counting_entropy(0x10)).unwrap();
        let (second, origin) =
            load_or_generate(&mut vault, &label, counting_entropy(0xFF)).unwrap();
        assert_eq!(origin, IdentityOrigin::Loaded);
        assert_eq!(*first, *second);
    }

    #[test]
    fn distinct_labels_keep_distinct_identities() {
        let mut vault = MemoryVault::default();
        let (transport, _) =
            load_or_generate(&mut vault, &label("transport"), counting_entropy(0x01)).unwrap();
        let (lxmf, _) =
            load_or_generate(&mut vault, &label("lxmf"), counting_entropy(0x80)).unwrap();
        assert_ne!(*transport, *lxmf);
    }

    #[test]
    fn a_store_failure_on_a_fresh_identity_propagates() {
        let mut vault = MemoryVault {
            fail_store: true,
            ..MemoryVault::default()
        };
        let outcome = load_or_generate(&mut vault, &label("primary"), counting_entropy(0x10));
        assert_eq!(outcome.unwrap_err(), MemoryVaultError::StoreRefused);
    }

    #[test]
    fn remove_reports_whether_anything_was_held() {
        let mut vault = MemoryVault::default();
        let label = label("primary");
        load_or_generate(&mut vault, &label, counting_entropy(0x10)).unwrap();
        assert!(vault.remove(&label).unwrap());
        assert!(!vault.remove(&label).unwrap());
    }
}
