use crate::crypto::ratchets::RatchetPolicy;
use crate::identity::IdentityHash;
use crate::routing::announce::emit::AnnounceAppDataBytes;
use crate::routing::announce::{
    derive_destination_hash, derive_plain_destination_hash, expand_name, DottedNameHash,
    ExpandNameError,
};
use crate::routing::links::resources::ResourceStrategy;
use crate::storage::TablePushError;
use crate::units::ByteLimit;
use crate::wire::{DestinationHash, DestinationType};

/// RNS 1.4.2 `Destination.PROVE_NONE` / `PROVE_ALL` / `PROVE_APP`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProofStrategy {
    ProveNone,
    ProveAll,
    /// RNS 1.4.2 `PROVE_APP`: the app decides per delivered packet.
    ProveIf,
}

/// RNS 1.4.2 `Destination.accept_link_requests`: `AcceptNone` announces but answers no link request — reachable for singles and announces, silent to `LINKREQUEST`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkRequestPolicy {
    AcceptAll,
    AcceptNone,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpstreamAppDestinationKind {
    Plain,
    Single {
        identity: IdentityHash,
        proof_strategy: ProofStrategy,
        link_request_policy: LinkRequestPolicy,
        /// How links answered for this destination greet inbound resource advertisements the moment they activate: set once per destination, stamped onto every responder-side link at birth, so no per-link command can race a sender who advertises instantly.
        resource_strategy: ResourceStrategy,
        maximum_request_bytes: ByteLimit,
        /// Read at decrypt: `RatchetsRequired` refuses the identity-key fallback on inbound singles. The retained secrets themselves live in the engine's self-ratchets table.
        ratchet_policy: RatchetPolicy,
    },
    Group,
}

impl UpstreamAppDestinationKind {
    pub const fn wire_type(self) -> DestinationType {
        match self {
            Self::Plain => DestinationType::Plain,
            Self::Single { .. } => DestinationType::Single,
            Self::Group => DestinationType::Group,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UpstreamAppDestination {
    pub destination: DestinationHash,
    pub kind: UpstreamAppDestinationKind,
    pub name_hash: DottedNameHash,
}

/// The [`UpstreamAppDestinationKind::Single`] levers, kind-narrowed by [`UpstreamAppDestinations::lookup_single`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegisteredSingle {
    pub identity: IdentityHash,
    pub proof_strategy: ProofStrategy,
    pub link_request_policy: LinkRequestPolicy,
    pub resource_strategy: ResourceStrategy,
    pub maximum_request_bytes: ByteLimit,
    pub ratchet_policy: RatchetPolicy,
}

pub trait UpstreamAppDestinationTable {
    fn capacity(&self) -> usize;
    fn len(&self) -> usize;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn destinations(&self) -> &[DestinationHash];
    fn kinds(&self) -> &[UpstreamAppDestinationKind];
    fn name_hashes(&self) -> &[DottedNameHash];
    fn app_data_at(&self, index: usize) -> Option<&[u8]>;
    fn app_data_at_mut(&mut self, index: usize) -> Option<&mut AnnounceAppDataBytes>;

    fn kind_mut(&mut self, index: usize) -> &mut UpstreamAppDestinationKind;
    fn swap_remove(&mut self, index: usize) -> Option<AnnounceAppDataBytes>;
    fn upsert(
        &mut self,
        destination: DestinationHash,
        kind: UpstreamAppDestinationKind,
        name_hash: DottedNameHash,
        app_data: AnnounceAppDataBytes,
    ) -> Result<usize, TablePushError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegisterDestinationError {
    Name(ExpandNameError),
    RegistryFull,
    UnknownIdentity,
    RatchetTableFull,
    AppDataTooLong,
    InvalidGroupKey,
}

#[derive(Debug, PartialEq, Eq)]
pub enum UnregisterDestinationOutcome {
    Unregistered {
        registration: UpstreamAppDestination,
    },
    NotRegistered,
}

#[derive(Debug, Default)]
pub struct UpstreamAppDestinations<C: UpstreamAppDestinationTable> {
    table: C,
}

impl<C: UpstreamAppDestinationTable> UpstreamAppDestinations<C> {
    pub fn register_plain(
        &mut self,
        app_name: &str,
        aspects: &[&str],
    ) -> Result<DestinationHash, RegisterDestinationError> {
        let name_hash = expand_name(app_name, aspects).map_err(RegisterDestinationError::Name)?;
        let destination = derive_plain_destination_hash(&name_hash);
        self.upsert(
            destination,
            UpstreamAppDestinationKind::Plain,
            name_hash,
            AnnounceAppDataBytes::new(),
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn register_single(
        &mut self,
        identity_hash: &IdentityHash,
        app_name: &str,
        aspects: &[&str],
        app_data: &[u8],
        proof_strategy: ProofStrategy,
        link_request_policy: LinkRequestPolicy,
        ratchet_policy: RatchetPolicy,
    ) -> Result<DestinationHash, RegisterDestinationError> {
        let name_hash = expand_name(app_name, aspects).map_err(RegisterDestinationError::Name)?;
        let app_data = AnnounceAppDataBytes::from_slice(app_data)
            .map_err(|()| RegisterDestinationError::AppDataTooLong)?;
        let destination = derive_destination_hash(identity_hash, &name_hash);
        self.upsert(
            destination,
            UpstreamAppDestinationKind::Single {
                identity: *identity_hash,
                proof_strategy,
                link_request_policy,
                resource_strategy: ResourceStrategy::AcceptNone,
                maximum_request_bytes: ByteLimit::Unlimited,
                ratchet_policy,
            },
            name_hash,
            app_data,
        )
    }

    /// The destination's standing answer to inbound resource offers, stamped onto its links at activation. Anything but a registered `Single` refuses.
    pub fn default_resource_strategy(&self, destination: &DestinationHash) -> ResourceStrategy {
        let Some(index) = self
            .table
            .destinations()
            .iter()
            .position(|candidate| candidate == destination)
        else {
            return ResourceStrategy::AcceptNone;
        };
        match self.table.kinds()[index] {
            UpstreamAppDestinationKind::Single {
                resource_strategy, ..
            } => resource_strategy,
            _ => ResourceStrategy::AcceptNone,
        }
    }

    pub fn set_default_resource_strategy(
        &mut self,
        destination: &DestinationHash,
        strategy: ResourceStrategy,
    ) -> bool {
        let Some(index) = self
            .table
            .destinations()
            .iter()
            .position(|candidate| candidate == destination)
        else {
            return false;
        };
        if let UpstreamAppDestinationKind::Single {
            resource_strategy, ..
        } = self.table.kind_mut(index)
        {
            *resource_strategy = strategy;
            true
        } else {
            false
        }
    }

    pub fn set_maximum_request_bytes(
        &mut self,
        destination: &DestinationHash,
        maximum: ByteLimit,
    ) -> bool {
        let Some(index) = self
            .table
            .destinations()
            .iter()
            .position(|candidate| candidate == destination)
        else {
            return false;
        };
        if let UpstreamAppDestinationKind::Single {
            maximum_request_bytes,
            ..
        } = self.table.kind_mut(index)
        {
            *maximum_request_bytes = maximum;
            true
        } else {
            false
        }
    }

    pub fn register_group(
        &mut self,
        identity_hash: &IdentityHash,
        app_name: &str,
        aspects: &[&str],
    ) -> Result<DestinationHash, RegisterDestinationError> {
        let name_hash = expand_name(app_name, aspects).map_err(RegisterDestinationError::Name)?;
        let destination = derive_destination_hash(identity_hash, &name_hash);
        self.upsert(
            destination,
            UpstreamAppDestinationKind::Group,
            name_hash,
            AnnounceAppDataBytes::new(),
        )
    }

    fn upsert(
        &mut self,
        destination: DestinationHash,
        kind: UpstreamAppDestinationKind,
        name_hash: DottedNameHash,
        app_data: AnnounceAppDataBytes,
    ) -> Result<DestinationHash, RegisterDestinationError> {
        self.table
            .upsert(destination, kind, name_hash, app_data)
            .map_err(|TablePushError::TableFull| RegisterDestinationError::RegistryFull)?;
        Ok(destination)
    }

    pub fn app_data_for(&self, destination: &DestinationHash) -> Option<&[u8]> {
        let slot = self
            .table
            .destinations()
            .iter()
            .position(|candidate| candidate == destination)?;
        self.table.app_data_at(slot)
    }

    pub(crate) fn replace_registered_announce_app_data(
        &mut self,
        destination: &DestinationHash,
        app_data: AnnounceAppDataBytes,
    ) -> Option<AnnounceAppDataBytes> {
        let slot = self
            .table
            .destinations()
            .iter()
            .position(|candidate| candidate == destination)?;
        let registered = self.table.app_data_at_mut(slot)?;
        Some(core::mem::replace(registered, app_data))
    }

    pub fn registration_for(
        &self,
        destination: &DestinationHash,
    ) -> Option<(UpstreamAppDestination, &[u8])> {
        let slot = self
            .table
            .destinations()
            .iter()
            .position(|candidate| candidate == destination)?;
        let registered = UpstreamAppDestination {
            destination: *destination,
            kind: *self.table.kinds().get(slot)?,
            name_hash: *self.table.name_hashes().get(slot)?,
        };
        Some((registered, self.table.app_data_at(slot)?))
    }

    pub fn unregister(&mut self, destination: &DestinationHash) -> UnregisterDestinationOutcome {
        let Some(slot) = self
            .table
            .destinations()
            .iter()
            .position(|candidate| candidate == destination)
        else {
            return UnregisterDestinationOutcome::NotRegistered;
        };
        let Some(kind) = self.table.kinds().get(slot).copied() else {
            return UnregisterDestinationOutcome::NotRegistered;
        };
        let Some(name_hash) = self.table.name_hashes().get(slot).copied() else {
            return UnregisterDestinationOutcome::NotRegistered;
        };
        let registered = UpstreamAppDestination {
            destination: *destination,
            kind,
            name_hash,
        };
        if self.table.swap_remove(slot).is_none() {
            return UnregisterDestinationOutcome::NotRegistered;
        }
        UnregisterDestinationOutcome::Unregistered {
            registration: registered,
        }
    }

    pub fn lookup(
        &self,
        destination: &DestinationHash,
        destination_type: DestinationType,
    ) -> Option<UpstreamAppDestination> {
        let slot = self
            .table
            .destinations()
            .iter()
            .position(|candidate| candidate == destination)?;
        let kind = *self.table.kinds().get(slot)?;
        if kind.wire_type() != destination_type {
            return None;
        }
        Some(UpstreamAppDestination {
            destination: *destination,
            kind,
            name_hash: *self.table.name_hashes().get(slot)?,
        })
    }

    pub fn lookup_single(&self, destination: &DestinationHash) -> Option<RegisteredSingle> {
        match self.lookup(destination, DestinationType::Single)?.kind {
            UpstreamAppDestinationKind::Single {
                identity,
                proof_strategy,
                link_request_policy,
                resource_strategy,
                maximum_request_bytes,
                ratchet_policy,
            } => Some(RegisteredSingle {
                identity,
                proof_strategy,
                link_request_policy,
                resource_strategy,
                maximum_request_bytes,
                ratchet_policy,
            }),
            UpstreamAppDestinationKind::Plain | UpstreamAppDestinationKind::Group => None,
        }
    }

    pub fn len(&self) -> usize {
        self.table.len()
    }

    pub fn is_empty(&self) -> bool {
        self.table.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = UpstreamAppDestination> + '_ {
        self.table
            .destinations()
            .iter()
            .zip(self.table.kinds())
            .zip(self.table.name_hashes())
            .map(|((destination, kind), name_hash)| UpstreamAppDestination {
                destination: *destination,
                kind: *kind,
                name_hash: *name_hash,
            })
    }
}

#[cfg(test)]
mod tests {
    use super::super::*;
    use super::*;

    type TestDestinations = UpstreamAppDestinations<FixedUpstreamAppDestinationTable<8>>;

    fn bytes_from_hex<const N: usize>(s: &str) -> [u8; N] {
        let mut out = [0u8; N];
        for (i, byte) in out.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).expect("valid hex");
        }
        out
    }

    #[test]
    fn plain_registration_derives_the_rns_1_4_2_destination_hash() {
        let mut destinations = TestDestinations::default();
        assert_eq!(
            destinations.register_plain("personal", &["node"]),
            Ok(DestinationHash::new(bytes_from_hex(
                "12f815e3e65add6ceb2fda0e7be33868"
            ))),
        );
        assert_eq!(
            destinations.register_plain("rnstransport", &["path", "request"]),
            Ok(DestinationHash::new(bytes_from_hex(
                "6b9f66014d9853faab220fba47d02761"
            ))),
        );
    }

    #[test]
    fn single_registration_derives_the_rns_1_4_2_destination_hash() {
        let identity_hash = IdentityHash::new(bytes_from_hex("4cd0cc45a7405dbd5cf9b5be1ef92f10"));
        let mut destinations = TestDestinations::default();
        assert_eq!(
            destinations.register_single(
                &identity_hash,
                "personal",
                &["node"],
                b"",
                ProofStrategy::ProveNone,
                LinkRequestPolicy::AcceptAll,
                RatchetPolicy::NoRatchets,
            ),
            Ok(DestinationHash::new(bytes_from_hex(
                "c3cfae69b36bb6e3bbfd96a3b5867a59"
            ))),
        );
    }

    #[test]
    fn lookup_single_narrows_to_single_registrations_only() {
        let identity_hash = IdentityHash::new(bytes_from_hex("4cd0cc45a7405dbd5cf9b5be1ef92f10"));
        let mut destinations = TestDestinations::default();
        let plain = destinations.register_plain("personal", &["node"]).unwrap();
        let single = destinations
            .register_single(
                &identity_hash,
                "personal",
                &["single"],
                b"",
                ProofStrategy::ProveAll,
                LinkRequestPolicy::AcceptAll,
                RatchetPolicy::RatchetsRequired,
            )
            .unwrap();

        assert_eq!(destinations.lookup_single(&plain), None);
        assert_eq!(
            destinations.lookup_single(&single),
            Some(RegisteredSingle {
                identity: identity_hash,
                proof_strategy: ProofStrategy::ProveAll,
                link_request_policy: LinkRequestPolicy::AcceptAll,
                resource_strategy: ResourceStrategy::AcceptNone,
                maximum_request_bytes: ByteLimit::Unlimited,
                ratchet_policy: RatchetPolicy::RatchetsRequired,
            }),
        );
    }

    #[test]
    fn unregister_returns_the_exact_registration_and_reclaims_its_slot() {
        let identity_hash = IdentityHash::new([0x21; 16]);
        let mut destinations =
            UpstreamAppDestinations::<FixedUpstreamAppDestinationTable<2>>::default();
        let first = destinations
            .register_single(
                &identity_hash,
                "personal",
                &["first"],
                b"first-data",
                ProofStrategy::ProveAll,
                LinkRequestPolicy::AcceptAll,
                RatchetPolicy::NoRatchets,
            )
            .unwrap();
        let second = destinations
            .register_plain("personal", &["second"])
            .unwrap();
        assert_eq!(
            destinations.register_plain("personal", &["full"]),
            Err(RegisterDestinationError::RegistryFull),
        );
        let expected = destinations.registration_for(&first).unwrap().0;

        assert_eq!(
            destinations.unregister(&first),
            UnregisterDestinationOutcome::Unregistered {
                registration: expected,
            },
        );
        assert_eq!(
            destinations.unregister(&first),
            UnregisterDestinationOutcome::NotRegistered,
        );
        assert!(destinations.registration_for(&first).is_none());
        assert_eq!(destinations.app_data_for(&second), Some([].as_slice()));
        assert_eq!(destinations.len(), 1);
        assert!(destinations.register_plain("personal", &["third"]).is_ok());
        assert_eq!(destinations.len(), 2);
    }

    #[test]
    fn lookup_requires_both_the_hash_and_the_wire_type_to_match() {
        let mut destinations = TestDestinations::default();
        let plain = destinations.register_plain("personal", &["node"]).unwrap();

        let found = destinations
            .lookup(&plain, DestinationType::Plain)
            .expect("registered plain destination answers a plain lookup");
        assert_eq!(found.destination, plain);
        assert_eq!(found.kind, UpstreamAppDestinationKind::Plain);

        assert_eq!(destinations.lookup(&plain, DestinationType::Single), None);
        assert_eq!(destinations.lookup(&plain, DestinationType::Group), None);
        assert_eq!(destinations.lookup(&plain, DestinationType::Link), None);

        let unknown = DestinationHash::new([0x99; 16]);
        assert_eq!(destinations.lookup(&unknown, DestinationType::Plain), None);
    }

    #[test]
    fn reregistration_keeps_one_row_and_takes_the_new_params() {
        let identity_hash = IdentityHash::new(bytes_from_hex("4cd0cc45a7405dbd5cf9b5be1ef92f10"));
        let mut destinations = TestDestinations::default();
        let first = destinations
            .register_single(
                &identity_hash,
                "personal",
                &["node"],
                b"",
                ProofStrategy::ProveNone,
                LinkRequestPolicy::AcceptAll,
                RatchetPolicy::NoRatchets,
            )
            .unwrap();
        let second = destinations
            .register_single(
                &identity_hash,
                "personal",
                &["node"],
                b"app",
                ProofStrategy::ProveAll,
                LinkRequestPolicy::AcceptAll,
                RatchetPolicy::NoRatchets,
            )
            .unwrap();

        assert_eq!(first, second);
        assert_eq!(destinations.len(), 1);
        assert_eq!(
            destinations
                .lookup(&first, DestinationType::Single)
                .map(|found| found.kind),
            Some(UpstreamAppDestinationKind::Single {
                identity: identity_hash,
                proof_strategy: ProofStrategy::ProveAll,
                link_request_policy: LinkRequestPolicy::AcceptAll,
                resource_strategy: ResourceStrategy::AcceptNone,
                maximum_request_bytes: ByteLimit::Unlimited,
                ratchet_policy: RatchetPolicy::NoRatchets,
            }),
            "re-registration overwrites the proof strategy in place",
        );
        assert_eq!(destinations.app_data_for(&first), Some(b"app".as_slice()));
    }

    #[test]
    fn a_full_registry_reports_itself() {
        let mut destinations =
            UpstreamAppDestinations::<FixedUpstreamAppDestinationTable<2>>::default();
        assert!(destinations.register_plain("personal", &["a"]).is_ok());
        assert!(destinations.register_plain("personal", &["b"]).is_ok());
        assert_eq!(
            destinations.register_plain("personal", &["overflow"]),
            Err(RegisterDestinationError::RegistryFull),
        );
        assert_eq!(destinations.len(), 2);
    }

    #[test]
    fn invalid_names_surface_the_expand_error() {
        let mut destinations = TestDestinations::default();
        assert_eq!(
            destinations.register_plain("perso.nal", &[]),
            Err(RegisterDestinationError::Name(
                ExpandNameError::DotInComponent
            )),
        );
        assert!(destinations.is_empty());
    }

    #[test]
    fn the_same_name_yields_distinct_plain_and_single_addresses() {
        let identity_hash = IdentityHash::new(bytes_from_hex("4cd0cc45a7405dbd5cf9b5be1ef92f10"));
        let mut destinations = TestDestinations::default();
        let plain = destinations.register_plain("personal", &["node"]).unwrap();
        let single = destinations
            .register_single(
                &identity_hash,
                "personal",
                &["node"],
                b"",
                ProofStrategy::ProveNone,
                LinkRequestPolicy::AcceptAll,
                RatchetPolicy::NoRatchets,
            )
            .unwrap();

        assert_ne!(plain, single);
        assert_eq!(destinations.len(), 2);
        assert!(destinations
            .lookup(&plain, DestinationType::Plain)
            .is_some());
        assert!(destinations
            .lookup(&single, DestinationType::Single)
            .is_some());
    }

    #[test]
    fn iter_walks_the_columns_as_composed_views() {
        let identity_hash = IdentityHash::new(bytes_from_hex("4cd0cc45a7405dbd5cf9b5be1ef92f10"));
        let mut destinations = TestDestinations::default();
        let plain = destinations.register_plain("personal", &["node"]).unwrap();
        let single = destinations
            .register_single(
                &identity_hash,
                "personal",
                &["node"],
                b"",
                ProofStrategy::ProveAll,
                LinkRequestPolicy::AcceptAll,
                RatchetPolicy::NoRatchets,
            )
            .unwrap();

        let views: heapless::Vec<UpstreamAppDestination, 8> = destinations.iter().collect();
        assert_eq!(views.len(), 2);
        assert_eq!(views[0].destination, plain);
        assert_eq!(views[0].kind, UpstreamAppDestinationKind::Plain);
        assert_eq!(views[1].destination, single);
        assert_eq!(
            views[1].kind,
            UpstreamAppDestinationKind::Single {
                identity: identity_hash,
                proof_strategy: ProofStrategy::ProveAll,
                link_request_policy: LinkRequestPolicy::AcceptAll,
                resource_strategy: ResourceStrategy::AcceptNone,
                maximum_request_bytes: ByteLimit::Unlimited,
                ratchet_policy: RatchetPolicy::NoRatchets,
            }
        );
        assert_eq!(views[0].name_hash, views[1].name_hash);
    }

    #[test]
    fn each_registration_keeps_its_own_proof_strategy() {
        let identity_hash = IdentityHash::new(bytes_from_hex("4cd0cc45a7405dbd5cf9b5be1ef92f10"));
        let mut destinations = TestDestinations::default();
        let proving = destinations
            .register_single(
                &identity_hash,
                "personal",
                &["proving"],
                b"",
                ProofStrategy::ProveAll,
                LinkRequestPolicy::AcceptAll,
                RatchetPolicy::NoRatchets,
            )
            .unwrap();
        let silent = destinations
            .register_single(
                &identity_hash,
                "personal",
                &["silent"],
                b"",
                ProofStrategy::ProveNone,
                LinkRequestPolicy::AcceptAll,
                RatchetPolicy::NoRatchets,
            )
            .unwrap();

        assert_eq!(
            destinations
                .lookup(&proving, DestinationType::Single)
                .map(|found| found.kind),
            Some(UpstreamAppDestinationKind::Single {
                identity: identity_hash,
                proof_strategy: ProofStrategy::ProveAll,
                link_request_policy: LinkRequestPolicy::AcceptAll,
                resource_strategy: ResourceStrategy::AcceptNone,
                maximum_request_bytes: ByteLimit::Unlimited,
                ratchet_policy: RatchetPolicy::NoRatchets,
            }),
        );
        assert_eq!(
            destinations
                .lookup(&silent, DestinationType::Single)
                .map(|found| found.kind),
            Some(UpstreamAppDestinationKind::Single {
                identity: identity_hash,
                proof_strategy: ProofStrategy::ProveNone,
                link_request_policy: LinkRequestPolicy::AcceptAll,
                resource_strategy: ResourceStrategy::AcceptNone,
                maximum_request_bytes: ByteLimit::Unlimited,
                ratchet_policy: RatchetPolicy::NoRatchets,
            }),
        );
    }
}
