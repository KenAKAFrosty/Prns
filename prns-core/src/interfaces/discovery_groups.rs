use core::cmp::Ordering;
use core::fmt;

use crate::crypto::{sha256, SHA256_OUTPUT_LEN};
use crate::interfaces::{InterfaceId, INTERFACE_ID_LEN};

pub const MAX_DISCOVERY_GROUP_ID_LEN: usize = 32;
pub const MAX_DISCOVERY_GROUPS: usize = 4;
pub const DEFAULT_DISCOVERY_GROUP_NAME: &str = "reticulum";
pub const DEFAULT_DISCOVERY_GROUP_HASH: DiscoveryGroupHash = DiscoveryGroupHash::from_bytes([
    0xea, 0xc4, 0xd7, 0x0b, 0xfb, 0x1c, 0x16, 0xe4, 0x5e, 0x39, 0x48, 0x5e, 0x31, 0xe1, 0xf5, 0xcc,
    0xb1, 0x8c, 0xed, 0xf8, 0x78, 0xe0, 0x31, 0x0d, 0x9a, 0x96, 0x10, 0x01, 0x68, 0xf8, 0x9f, 0x0d,
]);
pub const MAX_DISCOVERY_GROUP_INTERFACES: usize = 2;
pub const DISCOVERY_GROUP_CONFIGURATION_SNAPSHOT_VERSION: u8 = 1;
pub const DISCOVERY_GROUP_CONFIGURATION_SNAPSHOT_MAX_LEN: usize = 2
    + MAX_DISCOVERY_GROUP_INTERFACES
        * (INTERFACE_ID_LEN + 1 + MAX_DISCOVERY_GROUPS * (1 + MAX_DISCOVERY_GROUP_ID_LEN));

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryGroupIdError {
    Empty,
    TooLong,
    InvalidUtf8,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DiscoveryGroupId {
    value: [u8; MAX_DISCOVERY_GROUP_ID_LEN],
    len: u8,
}

impl DiscoveryGroupId {
    pub fn parse(value: &str) -> Result<Self, DiscoveryGroupIdError> {
        Self::from_bytes(value.as_bytes())
    }

    pub fn from_bytes(value: &[u8]) -> Result<Self, DiscoveryGroupIdError> {
        if value.is_empty() {
            return Err(DiscoveryGroupIdError::Empty);
        }
        if value.len() > MAX_DISCOVERY_GROUP_ID_LEN {
            return Err(DiscoveryGroupIdError::TooLong);
        }
        core::str::from_utf8(value).map_err(|_| DiscoveryGroupIdError::InvalidUtf8)?;
        let mut stored = [0; MAX_DISCOVERY_GROUP_ID_LEN];
        stored[..value.len()].copy_from_slice(value);
        Ok(Self {
            value: stored,
            len: value.len() as u8,
        })
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.value[..usize::from(self.len)]
    }

    #[must_use]
    pub const fn byte_len(&self) -> usize {
        self.len as usize
    }

    #[allow(clippy::expect_used)]
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(self.as_bytes())
            .expect("DiscoveryGroupId constructors establish UTF-8 validity")
    }

    pub fn hash(&self) -> DiscoveryGroupHash {
        DiscoveryGroupHash(sha256(self.as_bytes()))
    }

    const fn empty_slot() -> Self {
        Self {
            value: [0; MAX_DISCOVERY_GROUP_ID_LEN],
            len: 0,
        }
    }
}

impl fmt::Debug for DiscoveryGroupId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("DiscoveryGroupId")
            .field(&self.as_str())
            .finish()
    }
}

impl fmt::Display for DiscoveryGroupId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DiscoveryGroupHash([u8; SHA256_OUTPUT_LEN]);

impl DiscoveryGroupHash {
    pub const LEN: usize = SHA256_OUTPUT_LEN;

    pub const fn from_bytes(bytes: [u8; SHA256_OUTPUT_LEN]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; SHA256_OUTPUT_LEN] {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiscoveryGroupSetError {
    Empty,
    TooMany,
    Duplicate(DiscoveryGroupId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryGroupHashSetError {
    Empty,
    TooMany,
    Duplicate(DiscoveryGroupHash),
    Unsorted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryGroupApplyOutcome {
    Applied,
    Unchanged,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiscoveryGroupSet<const MAX: usize = MAX_DISCOVERY_GROUPS> {
    groups: [DiscoveryGroupId; MAX],
    len: u8,
}

impl<const MAX: usize> DiscoveryGroupSet<MAX> {
    pub fn singleton(group: DiscoveryGroupId) -> Result<Self, DiscoveryGroupSetError> {
        if MAX == 0 {
            return Err(DiscoveryGroupSetError::Empty);
        }
        let mut groups = [DiscoveryGroupId::empty_slot(); MAX];
        if let Some(first) = groups.first_mut() {
            *first = group;
        }
        Ok(Self { groups, len: 1 })
    }

    pub fn try_from_slice(groups: &[DiscoveryGroupId]) -> Result<Self, DiscoveryGroupSetError> {
        if groups.is_empty() || MAX == 0 {
            return Err(DiscoveryGroupSetError::Empty);
        }
        if groups.len() > MAX || groups.len() > usize::from(u8::MAX) {
            return Err(DiscoveryGroupSetError::TooMany);
        }
        let mut canonical = [DiscoveryGroupId::empty_slot(); MAX];
        for (index, group) in groups.iter().copied().enumerate() {
            let mut insertion = index;
            while insertion > 0 {
                match group.cmp(&canonical[insertion - 1]) {
                    Ordering::Less => {
                        canonical[insertion] = canonical[insertion - 1];
                        insertion -= 1;
                    }
                    Ordering::Equal => return Err(DiscoveryGroupSetError::Duplicate(group)),
                    Ordering::Greater => break,
                }
            }
            canonical[insertion] = group;
        }
        Ok(Self {
            groups: canonical,
            len: groups.len() as u8,
        })
    }

    pub fn iter(&self) -> impl Iterator<Item = &DiscoveryGroupId> {
        self.groups[..self.len()].iter()
    }

    pub fn contains(&self, group: &DiscoveryGroupId) -> bool {
        let mut index = 0;
        while index < self.len as usize {
            if &self.groups[index] == group {
                return true;
            }
            index += 1;
        }
        false
    }

    pub fn contains_reticulum(&self) -> bool {
        let mut index = 0;
        while index < self.len as usize {
            if self.groups[index].as_bytes() == DEFAULT_DISCOVERY_GROUP_NAME.as_bytes() {
                return true;
            }
            index += 1;
        }
        false
    }

    pub fn hashes(&self) -> DiscoveryGroupHashSet<MAX> {
        let mut hashes = [DiscoveryGroupHash::from_bytes([0; SHA256_OUTPUT_LEN]); MAX];
        let mut index = 0;
        while index < self.len as usize {
            let hash = self.groups[index].hash();
            let mut insertion = index;
            while insertion > 0 && hash < hashes[insertion - 1] {
                hashes[insertion] = hashes[insertion - 1];
                insertion -= 1;
            }
            hashes[insertion] = hash;
            index += 1;
        }
        DiscoveryGroupHashSet {
            hashes,
            len: self.len,
        }
    }

    pub const fn len(&self) -> usize {
        self.len as usize
    }

    pub const fn is_empty(&self) -> bool {
        false
    }
}

impl DiscoveryGroupSet<MAX_DISCOVERY_GROUPS> {
    #[must_use]
    pub const fn from_singleton(group: DiscoveryGroupId) -> Self {
        let mut groups = [DiscoveryGroupId::empty_slot(); MAX_DISCOVERY_GROUPS];
        groups[0] = group;
        Self { groups, len: 1 }
    }

    pub fn reticulum() -> Self {
        let mut value = [0; MAX_DISCOVERY_GROUP_ID_LEN];
        value[..DEFAULT_DISCOVERY_GROUP_NAME.len()]
            .copy_from_slice(DEFAULT_DISCOVERY_GROUP_NAME.as_bytes());
        let group = DiscoveryGroupId {
            value,
            len: DEFAULT_DISCOVERY_GROUP_NAME.len() as u8,
        };
        Self::from_singleton(group)
    }
}

impl Default for DiscoveryGroupSet<MAX_DISCOVERY_GROUPS> {
    fn default() -> Self {
        Self::reticulum()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiscoveryGroupHashSet<const MAX: usize = MAX_DISCOVERY_GROUPS> {
    hashes: [DiscoveryGroupHash; MAX],
    len: u8,
}

impl<const MAX: usize> DiscoveryGroupHashSet<MAX> {
    pub fn try_from_wire_ordered(
        hashes: &[DiscoveryGroupHash],
    ) -> Result<Self, DiscoveryGroupHashSetError> {
        if hashes.is_empty() || MAX == 0 {
            return Err(DiscoveryGroupHashSetError::Empty);
        }
        if hashes.len() > MAX || hashes.len() > usize::from(u8::MAX) {
            return Err(DiscoveryGroupHashSetError::TooMany);
        }
        let mut canonical = [DiscoveryGroupHash::from_bytes([0; SHA256_OUTPUT_LEN]); MAX];
        for (slot, hash) in canonical.iter_mut().zip(hashes.iter().copied()) {
            *slot = hash;
        }
        Self::try_from_wire_array(canonical, hashes.len() as u8)
    }

    pub(crate) fn try_from_wire_array(
        hashes: [DiscoveryGroupHash; MAX],
        len: u8,
    ) -> Result<Self, DiscoveryGroupHashSetError> {
        if len == 0 {
            return Err(DiscoveryGroupHashSetError::Empty);
        }
        if usize::from(len) > MAX {
            return Err(DiscoveryGroupHashSetError::TooMany);
        }
        let mut index = 1;
        while index < usize::from(len) {
            match hashes[index - 1].cmp(&hashes[index]) {
                Ordering::Less => {}
                Ordering::Equal => {
                    return Err(DiscoveryGroupHashSetError::Duplicate(hashes[index]));
                }
                Ordering::Greater => return Err(DiscoveryGroupHashSetError::Unsorted),
            }
            index += 1;
        }
        Ok(Self { hashes, len })
    }

    pub const fn len(&self) -> usize {
        self.len as usize
    }

    pub const fn is_empty(&self) -> bool {
        false
    }

    pub fn iter(&self) -> impl Iterator<Item = &DiscoveryGroupHash> {
        self.hashes[..self.len()].iter()
    }

    pub fn shares_group_with(&self, other: &Self) -> bool {
        let mut local = 0;
        let mut remote = 0;
        while local < self.len as usize && remote < other.len as usize {
            match self.hashes[local].cmp(&other.hashes[remote]) {
                Ordering::Less => local += 1,
                Ordering::Equal => return true,
                Ordering::Greater => remote += 1,
            }
        }
        false
    }

    pub fn contains(&self, hash: &DiscoveryGroupHash) -> bool {
        let mut index = 0;
        while index < self.len as usize {
            if &self.hashes[index] == hash {
                return true;
            }
            index += 1;
        }
        false
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiscoveryGroupConfigurationEntry {
    interface_id: InterfaceId,
    groups: DiscoveryGroupSet,
}

impl DiscoveryGroupConfigurationEntry {
    #[must_use]
    pub const fn new(interface_id: InterfaceId, groups: DiscoveryGroupSet) -> Self {
        Self {
            interface_id,
            groups,
        }
    }

    #[must_use]
    pub const fn interface_id(&self) -> InterfaceId {
        self.interface_id
    }

    #[must_use]
    pub const fn groups(&self) -> &DiscoveryGroupSet {
        &self.groups
    }

    const fn empty_slot() -> Self {
        Self {
            interface_id: InterfaceId::new([0; INTERFACE_ID_LEN]),
            groups: DiscoveryGroupSet {
                groups: [DiscoveryGroupId::empty_slot(); MAX_DISCOVERY_GROUPS],
                len: 0,
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryGroupConfigurationSnapshotError {
    TooManyInterfaces,
    DuplicateInterface,
    NonCanonicalInterfaceOrder,
    InvalidVersion,
    InvalidGroupCount,
    InvalidGroupId,
    NonCanonicalGroups,
    Truncated,
    TrailingBytes,
    OutputTooShort,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiscoveryGroupConfigurationSnapshot {
    entries: [DiscoveryGroupConfigurationEntry; MAX_DISCOVERY_GROUP_INTERFACES],
    len: u8,
}

impl DiscoveryGroupConfigurationSnapshot {
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            entries: [DiscoveryGroupConfigurationEntry::empty_slot();
                MAX_DISCOVERY_GROUP_INTERFACES],
            len: 0,
        }
    }

    pub fn try_from_entries(
        entries: &[DiscoveryGroupConfigurationEntry],
    ) -> Result<Self, DiscoveryGroupConfigurationSnapshotError> {
        if entries.len() > MAX_DISCOVERY_GROUP_INTERFACES {
            return Err(DiscoveryGroupConfigurationSnapshotError::TooManyInterfaces);
        }
        let mut canonical =
            [DiscoveryGroupConfigurationEntry::empty_slot(); MAX_DISCOVERY_GROUP_INTERFACES];
        for (index, entry) in entries.iter().copied().enumerate() {
            let mut insertion = index;
            while insertion > 0 {
                if entry.interface_id() >= canonical[insertion - 1].interface_id() {
                    break;
                }
                canonical[insertion] = canonical[insertion - 1];
                insertion -= 1;
            }
            canonical[insertion] = entry;
        }
        for pair in canonical[..entries.len()].windows(2) {
            if pair[0].interface_id() == pair[1].interface_id() {
                return Err(DiscoveryGroupConfigurationSnapshotError::DuplicateInterface);
            }
        }
        Ok(Self {
            entries: canonical,
            len: entries.len() as u8,
        })
    }

    pub fn upsert(
        &mut self,
        interface_id: InterfaceId,
        groups: DiscoveryGroupSet,
    ) -> Result<(), DiscoveryGroupConfigurationSnapshotError> {
        let entry = DiscoveryGroupConfigurationEntry::new(interface_id, groups);
        match usize::from(self.len) {
            0 => {
                self.entries[0] = entry;
                self.len = 1;
            }
            1 => match interface_id.cmp(&self.entries[0].interface_id) {
                Ordering::Less => {
                    self.entries[1] = self.entries[0];
                    self.entries[0] = entry;
                    self.len = 2;
                }
                Ordering::Equal => self.entries[0] = entry,
                Ordering::Greater => {
                    self.entries[1] = entry;
                    self.len = 2;
                }
            },
            2 => {
                if interface_id == self.entries[0].interface_id {
                    self.entries[0] = entry;
                } else if interface_id == self.entries[1].interface_id {
                    self.entries[1] = entry;
                } else {
                    return Err(DiscoveryGroupConfigurationSnapshotError::TooManyInterfaces);
                }
            }
            _ => unreachable!("snapshot constructors bound the entry count"),
        }
        Ok(())
    }

    pub fn entries(&self) -> impl Iterator<Item = &DiscoveryGroupConfigurationEntry> {
        self.entries[..usize::from(self.len)].iter()
    }

    #[must_use]
    pub fn groups_for(&self, interface_id: InterfaceId) -> Option<&DiscoveryGroupSet> {
        self.entries()
            .find(|entry| entry.interface_id == interface_id)
            .map(DiscoveryGroupConfigurationEntry::groups)
    }

    #[must_use]
    pub const fn len(&self) -> usize {
        self.len as usize
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn encode(
        &self,
        output: &mut [u8],
    ) -> Result<usize, DiscoveryGroupConfigurationSnapshotError> {
        let mut required = 2;
        for entry in self.entries() {
            required += INTERFACE_ID_LEN + 1;
            for group in entry.groups.iter() {
                required += 1 + group.as_bytes().len();
            }
        }
        if output.len() < required {
            return Err(DiscoveryGroupConfigurationSnapshotError::OutputTooShort);
        }
        Ok(self.write_canonical(output))
    }

    /// Encodes into the compile-time maximum snapshot buffer established by this type.
    #[must_use]
    pub fn encode_into_max(
        &self,
        output: &mut [u8; DISCOVERY_GROUP_CONFIGURATION_SNAPSHOT_MAX_LEN],
    ) -> usize {
        self.write_canonical(output)
    }

    fn write_canonical(&self, output: &mut [u8]) -> usize {
        output[0] = DISCOVERY_GROUP_CONFIGURATION_SNAPSHOT_VERSION;
        output[1] = self.len;
        let mut offset = 2;
        for entry in self.entries() {
            output[offset..offset + INTERFACE_ID_LEN]
                .copy_from_slice(entry.interface_id.as_bytes());
            offset += INTERFACE_ID_LEN;
            output[offset] = entry.groups.len() as u8;
            offset += 1;
            for group in entry.groups.iter() {
                output[offset] = group.as_bytes().len() as u8;
                offset += 1;
                output[offset..offset + group.as_bytes().len()].copy_from_slice(group.as_bytes());
                offset += group.as_bytes().len();
            }
        }
        offset
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, DiscoveryGroupConfigurationSnapshotError> {
        let Some((&version, rest)) = bytes.split_first() else {
            return Err(DiscoveryGroupConfigurationSnapshotError::Truncated);
        };
        if version != DISCOVERY_GROUP_CONFIGURATION_SNAPSHOT_VERSION {
            return Err(DiscoveryGroupConfigurationSnapshotError::InvalidVersion);
        }
        let Some((&count, _)) = rest.split_first() else {
            return Err(DiscoveryGroupConfigurationSnapshotError::Truncated);
        };
        if usize::from(count) > MAX_DISCOVERY_GROUP_INTERFACES {
            return Err(DiscoveryGroupConfigurationSnapshotError::TooManyInterfaces);
        }
        let mut offset = 2;
        let mut entries =
            [DiscoveryGroupConfigurationEntry::empty_slot(); MAX_DISCOVERY_GROUP_INTERFACES];
        for entry_index in 0..usize::from(count) {
            let id_bytes: [u8; INTERFACE_ID_LEN] = bytes
                .get(offset..offset + INTERFACE_ID_LEN)
                .ok_or(DiscoveryGroupConfigurationSnapshotError::Truncated)?
                .try_into()
                .map_err(|_| DiscoveryGroupConfigurationSnapshotError::Truncated)?;
            offset += INTERFACE_ID_LEN;
            let group_count = *bytes
                .get(offset)
                .ok_or(DiscoveryGroupConfigurationSnapshotError::Truncated)?;
            offset += 1;
            if group_count == 0 || usize::from(group_count) > MAX_DISCOVERY_GROUPS {
                return Err(DiscoveryGroupConfigurationSnapshotError::InvalidGroupCount);
            }
            let mut groups = [DiscoveryGroupId::empty_slot(); MAX_DISCOVERY_GROUPS];
            for group_index in 0..usize::from(group_count) {
                let len = usize::from(
                    *bytes
                        .get(offset)
                        .ok_or(DiscoveryGroupConfigurationSnapshotError::Truncated)?,
                );
                offset += 1;
                let value = bytes
                    .get(offset..offset + len)
                    .ok_or(DiscoveryGroupConfigurationSnapshotError::Truncated)?;
                offset += len;
                let group = DiscoveryGroupId::from_bytes(value)
                    .map_err(|_| DiscoveryGroupConfigurationSnapshotError::InvalidGroupId)?;
                if group_index > 0 && groups[group_index - 1] >= group {
                    return Err(DiscoveryGroupConfigurationSnapshotError::NonCanonicalGroups);
                }
                groups[group_index] = group;
            }
            let interface_id = InterfaceId::new(id_bytes);
            if entry_index > 0 {
                match entries[entry_index - 1].interface_id.cmp(&interface_id) {
                    Ordering::Less => {}
                    Ordering::Equal => {
                        return Err(DiscoveryGroupConfigurationSnapshotError::DuplicateInterface);
                    }
                    Ordering::Greater => {
                        return Err(
                            DiscoveryGroupConfigurationSnapshotError::NonCanonicalInterfaceOrder,
                        );
                    }
                }
            }
            entries[entry_index] = DiscoveryGroupConfigurationEntry::new(
                interface_id,
                DiscoveryGroupSet {
                    groups,
                    len: group_count,
                },
            );
        }
        if offset != bytes.len() {
            return Err(DiscoveryGroupConfigurationSnapshotError::TrailingBytes);
        }
        Ok(Self {
            entries,
            len: count,
        })
    }
}

impl Default for DiscoveryGroupConfigurationSnapshot {
    fn default() -> Self {
        Self::empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn ids_are_complete_valid_utf8_values() {
        assert_eq!(
            DiscoveryGroupId::parse(""),
            Err(DiscoveryGroupIdError::Empty)
        );
        assert_eq!(
            DiscoveryGroupId::from_bytes(&[0xff]),
            Err(DiscoveryGroupIdError::InvalidUtf8)
        );
        assert_eq!(
            DiscoveryGroupId::parse("abcdefghijklmnopqrstuvwxyz1234567"),
            Err(DiscoveryGroupIdError::TooLong)
        );
        let value = DiscoveryGroupId::parse("montréal").expect("valid group");
        assert_eq!(value.as_str(), "montréal");
        let maximum = DiscoveryGroupId::parse("0123456789abcdef0123456789abcdef")
            .expect("32-byte group is valid");
        assert_eq!(maximum.as_bytes().len(), MAX_DISCOVERY_GROUP_ID_LEN);
    }

    #[test]
    fn sets_sort_and_reject_duplicates_as_whole_values() {
        let alpha = DiscoveryGroupId::parse("alpha").expect("valid group");
        let beta = DiscoveryGroupId::parse("beta").expect("valid group");
        let set = DiscoveryGroupSet::<4>::try_from_slice(&[beta, alpha]).expect("valid set");
        assert_eq!(
            set.iter().cloned().collect::<std::vec::Vec<_>>(),
            vec![alpha, beta]
        );
        assert_eq!(
            DiscoveryGroupSet::<4>::try_from_slice(&[alpha, alpha]),
            Err(DiscoveryGroupSetError::Duplicate(alpha))
        );
        assert_eq!(
            DiscoveryGroupSet::<4>::try_from_slice(&[]),
            Err(DiscoveryGroupSetError::Empty)
        );
        let over_capacity = ["a", "b", "c", "d", "e"]
            .map(|name| DiscoveryGroupId::parse(name).expect("valid group"));
        assert_eq!(
            DiscoveryGroupSet::<4>::try_from_slice(&over_capacity),
            Err(DiscoveryGroupSetError::TooMany)
        );
    }

    #[test]
    fn hash_sets_require_unique_canonical_wire_order() {
        let alpha = DiscoveryGroupId::parse("alpha")
            .expect("valid group")
            .hash();
        let beta = DiscoveryGroupId::parse("beta").expect("valid group").hash();
        let mut ordered = [alpha, beta];
        ordered.sort_unstable();
        let set =
            DiscoveryGroupHashSet::<4>::try_from_wire_ordered(&ordered).expect("canonical hashes");
        assert_eq!(set.len(), 2);
        assert!(set.contains(&alpha));
        assert_eq!(
            DiscoveryGroupHashSet::<4>::try_from_wire_ordered(&[ordered[0], ordered[0]]),
            Err(DiscoveryGroupHashSetError::Duplicate(ordered[0]))
        );
        assert_eq!(
            DiscoveryGroupHashSet::<4>::try_from_wire_ordered(&[ordered[1], ordered[0]]),
            Err(DiscoveryGroupHashSetError::Unsorted)
        );
    }

    #[test]
    fn reticulum_hash_is_stable() {
        let set = DiscoveryGroupSet::<4>::reticulum();
        assert_eq!(
            set.hashes().iter().next().expect("default hash").as_bytes(),
            DEFAULT_DISCOVERY_GROUP_HASH.as_bytes()
        );
    }

    #[test]
    fn configuration_snapshots_round_trip_in_canonical_order() {
        let alpha = DiscoveryGroupId::parse("alpha").expect("valid group");
        let beta = DiscoveryGroupId::parse("beta").expect("valid group");
        let groups = DiscoveryGroupSet::try_from_slice(&[beta, alpha]).expect("valid groups");
        let first = InterfaceId::new([0x10; INTERFACE_ID_LEN]);
        let second = InterfaceId::new([0x20; INTERFACE_ID_LEN]);
        let snapshot = DiscoveryGroupConfigurationSnapshot::try_from_entries(&[
            DiscoveryGroupConfigurationEntry::new(second, DiscoveryGroupSet::reticulum()),
            DiscoveryGroupConfigurationEntry::new(first, groups),
        ])
        .expect("valid snapshot");
        assert_eq!(
            snapshot
                .entries()
                .map(DiscoveryGroupConfigurationEntry::interface_id)
                .collect::<std::vec::Vec<_>>(),
            vec![first, second]
        );
        assert_eq!(snapshot.groups_for(first), Some(&groups));

        let mut encoded = [0u8; DISCOVERY_GROUP_CONFIGURATION_SNAPSHOT_MAX_LEN];
        let written = snapshot.encode(&mut encoded).expect("snapshot fits");
        assert_eq!(
            DiscoveryGroupConfigurationSnapshot::decode(&encoded[..written]),
            Ok(snapshot)
        );
    }

    #[test]
    fn configuration_snapshot_decoder_refuses_noncanonical_and_trailing_values() {
        let first = InterfaceId::new([0x10; INTERFACE_ID_LEN]);
        let second = InterfaceId::new([0x20; INTERFACE_ID_LEN]);
        let snapshot = DiscoveryGroupConfigurationSnapshot::try_from_entries(&[
            DiscoveryGroupConfigurationEntry::new(first, DiscoveryGroupSet::reticulum()),
            DiscoveryGroupConfigurationEntry::new(second, DiscoveryGroupSet::reticulum()),
        ])
        .expect("valid snapshot");
        let mut encoded = [0u8; DISCOVERY_GROUP_CONFIGURATION_SNAPSHOT_MAX_LEN];
        let written = snapshot.encode(&mut encoded).expect("snapshot fits");

        let second_entry = 2 + INTERFACE_ID_LEN + 1 + 1 + DEFAULT_DISCOVERY_GROUP_NAME.len();
        encoded[2..2 + INTERFACE_ID_LEN].copy_from_slice(second.as_bytes());
        encoded[second_entry..second_entry + INTERFACE_ID_LEN].copy_from_slice(first.as_bytes());
        assert_eq!(
            DiscoveryGroupConfigurationSnapshot::decode(&encoded[..written]),
            Err(DiscoveryGroupConfigurationSnapshotError::NonCanonicalInterfaceOrder)
        );

        let written = snapshot.encode(&mut encoded).expect("snapshot fits");
        encoded[written] = 0xAA;
        assert_eq!(
            DiscoveryGroupConfigurationSnapshot::decode(&encoded[..=written]),
            Err(DiscoveryGroupConfigurationSnapshotError::TrailingBytes)
        );
    }

    #[test]
    fn configuration_snapshot_upsert_is_atomic_and_bounded() {
        let mut snapshot = DiscoveryGroupConfigurationSnapshot::empty();
        for suffix in 0..MAX_DISCOVERY_GROUP_INTERFACES {
            snapshot
                .upsert(
                    InterfaceId::new([suffix as u8; INTERFACE_ID_LEN]),
                    DiscoveryGroupSet::reticulum(),
                )
                .expect("entry fits");
        }
        let before = snapshot;
        assert_eq!(
            snapshot.upsert(
                InterfaceId::new([0xFF; INTERFACE_ID_LEN]),
                DiscoveryGroupSet::reticulum()
            ),
            Err(DiscoveryGroupConfigurationSnapshotError::TooManyInterfaces)
        );
        assert_eq!(snapshot, before);
    }

    proptest! {
        #[test]
        fn every_valid_group_list_is_canonicalized_as_one_whole_value(
            names in proptest::collection::vec("[a-z0-9_-]{1,32}", 1..=MAX_DISCOVERY_GROUPS),
        ) {
            let parsed = names
                .iter()
                .map(|name| DiscoveryGroupId::parse(name).expect("generated group is valid"))
                .collect::<std::vec::Vec<_>>();
            let mut expected = parsed.clone();
            expected.sort_unstable();
            let contains_duplicate = expected.windows(2).any(|pair| pair[0] == pair[1]);

            match DiscoveryGroupSet::<MAX_DISCOVERY_GROUPS>::try_from_slice(&parsed) {
                Ok(groups) => {
                    prop_assert!(!contains_duplicate);
                    prop_assert!(groups.iter().eq(expected.iter()));
                }
                Err(DiscoveryGroupSetError::Duplicate(_)) => {
                    prop_assert!(contains_duplicate);
                }
                Err(error) => prop_assert!(false, "unexpected generated-set error: {error:?}"),
            }
        }
    }
}
