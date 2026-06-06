mod impls;

pub use impls::*;

use crate::identity::IdentityHash;
use crate::routing::announce::{
    derive_destination_hash, derive_plain_destination_hash, expand_name, DottedNameHash,
    ExpandNameError,
};
use crate::routing::storage::ColumnsFull;
use crate::wire::{DestinationHash, DestinationType};

/// RNS 1.3.1 `Destination.PROVE_NONE` / `PROVE_ALL`: whether a destination
/// answers delivered packets with a signed proof of receipt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProofStrategy {
    ProveNone,
    ProveAll,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpstreamAppDestinationKind {
    Plain,
    Single {
        identity: IdentityHash,
        proof_strategy: ProofStrategy,
    },
}

impl UpstreamAppDestinationKind {
    pub const fn wire_type(self) -> DestinationType {
        match self {
            Self::Plain => DestinationType::Plain,
            Self::Single { .. } => DestinationType::Single,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UpstreamAppDestination {
    pub destination: DestinationHash,
    pub kind: UpstreamAppDestinationKind,
    pub name_hash: DottedNameHash,
}

pub trait UpstreamAppDestinationColumns {
    fn capacity(&self) -> usize;
    fn len(&self) -> usize;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn destinations(&self) -> &[DestinationHash];
    fn kinds(&self) -> &[UpstreamAppDestinationKind];
    fn name_hashes(&self) -> &[DottedNameHash];

    fn push(
        &mut self,
        destination: DestinationHash,
        kind: UpstreamAppDestinationKind,
        name_hash: DottedNameHash,
    ) -> Result<usize, ColumnsFull>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegisterDestinationError {
    Name(ExpandNameError),
    RegistryFull,
    UnknownIdentity,
    RatchetTableFull,
}

#[derive(Debug, Default)]
pub struct UpstreamAppDestinations<C: UpstreamAppDestinationColumns> {
    columns: C,
}

impl<C: UpstreamAppDestinationColumns> UpstreamAppDestinations<C> {
    pub fn register_plain(
        &mut self,
        app_name: &str,
        aspects: &[&str],
    ) -> Result<DestinationHash, RegisterDestinationError> {
        let name_hash = expand_name(app_name, aspects).map_err(RegisterDestinationError::Name)?;
        let destination = derive_plain_destination_hash(&name_hash);
        self.insert(destination, UpstreamAppDestinationKind::Plain, name_hash)
    }

    pub fn register_single(
        &mut self,
        identity_hash: &IdentityHash,
        app_name: &str,
        aspects: &[&str],
        proof_strategy: ProofStrategy,
    ) -> Result<DestinationHash, RegisterDestinationError> {
        let name_hash = expand_name(app_name, aspects).map_err(RegisterDestinationError::Name)?;
        let destination = derive_destination_hash(identity_hash, &name_hash);
        self.insert(
            destination,
            UpstreamAppDestinationKind::Single {
                identity: *identity_hash,
                proof_strategy,
            },
            name_hash,
        )
    }

    fn insert(
        &mut self,
        destination: DestinationHash,
        kind: UpstreamAppDestinationKind,
        name_hash: DottedNameHash,
    ) -> Result<DestinationHash, RegisterDestinationError> {
        if self.columns.destinations().contains(&destination) {
            return Ok(destination);
        }
        self.columns
            .push(destination, kind, name_hash)
            .map_err(|ColumnsFull| RegisterDestinationError::RegistryFull)?;
        Ok(destination)
    }

    pub fn lookup(
        &self,
        destination: &DestinationHash,
        destination_type: DestinationType,
    ) -> Option<UpstreamAppDestination> {
        let slot = self
            .columns
            .destinations()
            .iter()
            .position(|candidate| candidate == destination)?;
        let kind = *self.columns.kinds().get(slot)?;
        if kind.wire_type() != destination_type {
            return None;
        }
        Some(UpstreamAppDestination {
            destination: *destination,
            kind,
            name_hash: *self.columns.name_hashes().get(slot)?,
        })
    }

    pub fn len(&self) -> usize {
        self.columns.len()
    }

    pub fn is_empty(&self) -> bool {
        self.columns.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = UpstreamAppDestination> + '_ {
        self.columns
            .destinations()
            .iter()
            .zip(self.columns.kinds())
            .zip(self.columns.name_hashes())
            .map(|((destination, kind), name_hash)| UpstreamAppDestination {
                destination: *destination,
                kind: *kind,
                name_hash: *name_hash,
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type TestDestinations = UpstreamAppDestinations<FixedUpstreamAppDestinationColumns<8>>;

    fn hx<const N: usize>(s: &str) -> [u8; N] {
        let mut out = [0u8; N];
        for (i, byte) in out.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).expect("valid hex");
        }
        out
    }

    #[test]
    fn plain_registration_derives_the_rns_1_3_1_destination_hash() {
        let mut destinations = TestDestinations::default();
        assert_eq!(
            destinations.register_plain("personal", &["node"]),
            Ok(DestinationHash::new(hx("12f815e3e65add6ceb2fda0e7be33868"))),
        );
        assert_eq!(
            destinations.register_plain("rnstransport", &["path", "request"]),
            Ok(DestinationHash::new(hx("6b9f66014d9853faab220fba47d02761"))),
        );
    }

    #[test]
    fn single_registration_derives_the_rns_1_3_1_destination_hash() {
        let identity_hash = IdentityHash::new(hx("4cd0cc45a7405dbd5cf9b5be1ef92f10"));
        let mut destinations = TestDestinations::default();
        assert_eq!(
            destinations.register_single(
                &identity_hash,
                "personal",
                &["node"],
                ProofStrategy::ProveNone
            ),
            Ok(DestinationHash::new(hx("c3cfae69b36bb6e3bbfd96a3b5867a59"))),
        );
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
    fn reregistration_is_idempotent() {
        let mut destinations = TestDestinations::default();
        let first = destinations.register_plain("personal", &["node"]).unwrap();
        let second = destinations.register_plain("personal", &["node"]).unwrap();
        assert_eq!(first, second);
        assert_eq!(destinations.len(), 1);
    }

    #[test]
    fn a_full_registry_reports_itself() {
        let mut destinations =
            UpstreamAppDestinations::<FixedUpstreamAppDestinationColumns<2>>::default();
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
        let identity_hash = IdentityHash::new(hx("4cd0cc45a7405dbd5cf9b5be1ef92f10"));
        let mut destinations = TestDestinations::default();
        let plain = destinations.register_plain("personal", &["node"]).unwrap();
        let single = destinations
            .register_single(
                &identity_hash,
                "personal",
                &["node"],
                ProofStrategy::ProveNone,
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
        let identity_hash = IdentityHash::new(hx("4cd0cc45a7405dbd5cf9b5be1ef92f10"));
        let mut destinations = TestDestinations::default();
        let plain = destinations.register_plain("personal", &["node"]).unwrap();
        let single = destinations
            .register_single(
                &identity_hash,
                "personal",
                &["node"],
                ProofStrategy::ProveAll,
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
            }
        );
        assert_eq!(views[0].name_hash, views[1].name_hash);
    }

    #[test]
    fn each_registration_keeps_its_own_proof_strategy() {
        let identity_hash = IdentityHash::new(hx("4cd0cc45a7405dbd5cf9b5be1ef92f10"));
        let mut destinations = TestDestinations::default();
        let proving = destinations
            .register_single(
                &identity_hash,
                "personal",
                &["proving"],
                ProofStrategy::ProveAll,
            )
            .unwrap();
        let silent = destinations
            .register_single(
                &identity_hash,
                "personal",
                &["silent"],
                ProofStrategy::ProveNone,
            )
            .unwrap();

        assert_eq!(
            destinations
                .lookup(&proving, DestinationType::Single)
                .map(|found| found.kind),
            Some(UpstreamAppDestinationKind::Single {
                identity: identity_hash,
                proof_strategy: ProofStrategy::ProveAll,
            }),
        );
        assert_eq!(
            destinations
                .lookup(&silent, DestinationType::Single)
                .map(|found| found.kind),
            Some(UpstreamAppDestinationKind::Single {
                identity: identity_hash,
                proof_strategy: ProofStrategy::ProveNone,
            }),
        );
    }
}
