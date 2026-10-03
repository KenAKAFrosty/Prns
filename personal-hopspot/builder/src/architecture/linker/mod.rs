pub(super) mod gnu_ld;
pub(super) mod rust_lld;

use std::collections::BTreeMap;

#[derive(Debug)]
pub struct MemoryOverflow {
    region: String,
    bytes: u64,
}

#[derive(Debug)]
pub struct MemoryOverflows {
    primary: MemoryOverflow,
    additional: Vec<MemoryOverflow>,
}

impl MemoryOverflow {
    #[must_use]
    pub fn region(&self) -> &str {
        &self.region
    }

    #[must_use]
    pub const fn bytes(&self) -> u64 {
        self.bytes
    }
}

impl MemoryOverflows {
    fn from_entries<'a>(entries: impl Iterator<Item = (&'a str, u64)>) -> Option<Self> {
        let mut regions = BTreeMap::<String, u64>::new();
        for (region, bytes) in entries {
            regions
                .entry(region.to_string())
                .and_modify(|current| *current = (*current).max(bytes))
                .or_insert(bytes);
        }
        let mut entries = regions.into_iter();
        let (region, bytes) = entries.next()?;
        let primary = MemoryOverflow { region, bytes };
        Some(entries.fold(
            Self {
                primary,
                additional: Vec::new(),
            },
            |mut overflows, (region, bytes)| {
                let overflow = MemoryOverflow { region, bytes };
                if overflow.bytes > overflows.primary.bytes {
                    overflows.additional.push(overflows.primary);
                    overflows.primary = overflow;
                } else {
                    overflows.additional.push(overflow);
                }
                overflows
            },
        ))
    }

    pub fn iter(&self) -> impl Iterator<Item = &MemoryOverflow> {
        std::iter::once(&self.primary).chain(&self.additional)
    }

    #[must_use]
    pub fn primary(&self) -> &MemoryOverflow {
        &self.primary
    }
}
