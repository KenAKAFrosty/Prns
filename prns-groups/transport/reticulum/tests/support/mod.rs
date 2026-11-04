#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
pub mod wire;
use prns_groups_core::{AuthorizationEra, ControlId};
use prns_groups_mls::{Adapter, LocalIdentity, MemberBinding, Policy};
use prns_groups_service::{
    ClockInterval, ClockReading, ControlReference, Endpoint, EndpointId, EndpointIncarnation,
    GroupId, Limits, NamespaceId, OperationId, Publication, PublicationValidity, SchemaId, Seconds,
    Timestamp,
};
use prns_groups_store::{Coordinator, OperationTicket, StoreIncarnation};
use prns_groups_store_redb::{RedbBackend, RedbLimits, RedbStoreOwner, StoreNamespace};
use redb::Database;

pub const GROUP: GroupId = GroupId([71; 32]);
pub const STORAGE: StoreNamespace = StoreNamespace([72; 16]);
pub const NOW: u64 = 1_792_000_000;
pub const CHAT: NamespaceId = NamespaceId(1);
pub const TELEMETRY: NamespaceId = NamespaceId(2);

pub fn control() -> ControlReference {
    ControlReference {
        head: ControlId([73; 32]),
        era: AuthorizationEra(ControlId([74; 32])),
    }
}
pub fn clock() -> ClockReading {
    ClockReading::Trusted(ClockInterval::new(Timestamp(NOW + 1), Timestamp(NOW + 1)).unwrap())
}
pub fn endpoint(peer: usize) -> Endpoint {
    Endpoint {
        id: EndpointId([(peer + 1) as u8; 32]),
        incarnation: EndpointIncarnation::new(1).unwrap(),
    }
}
pub fn incarnation(peer: usize) -> StoreIncarnation {
    StoreIncarnation::new([(peer + 1) as u8; 24]).unwrap()
}
pub fn service_limits() -> Limits {
    Limits {
        max_records: 32,
        max_targets: 3,
        max_body_bytes: 8192,
        max_changes: 128,
        max_envelopes: 256,
        max_retained_wire_bytes: 1_048_576,
        maximum_window: Seconds(3600),
        future_skew: Seconds(0),
    }
}
pub fn mls_limits() -> prns_groups_mls::Limits {
    prns_groups_mls::Limits {
        max_members: 3,
        max_groups: 1,
        max_wire_bytes: 65_536,
        max_plaintext_bytes: 16384,
        max_snapshot_bytes: 65_536,
        max_key_packages: 4,
        max_retained_epochs: 1,
        max_pending_proposals: 8,
        max_pending_outputs: 128,
    }
}
pub fn store_limits() -> RedbLimits {
    RedbLimits {
        max_changes: 128,
        max_key_bytes: 256,
        max_value_bytes: 131_072,
        max_staged_bytes: 1_048_576,
        max_live_objects: 2048,
        max_live_bytes: 8_388_608,
        max_receipts: 256,
    }
}
pub fn coordinator_limits() -> prns_groups_store::Limits {
    store_limits().transaction_limits().unwrap()
}
pub fn next(peer: usize, coordinator: &Coordinator<RedbBackend<'_>>) -> OperationId {
    OperationTicket::new(incarnation(peer), coordinator.head().unwrap().0 + 1)
        .unwrap()
        .operation_id()
}
pub fn publication<'a>(
    nonce: u8,
    namespace: NamespaceId,
    targets: &'a [Endpoint],
    body: &'a [u8],
) -> Publication<'a> {
    Publication {
        nonce: [nonce; 32],
        namespace,
        schema: SchemaId(1),
        validity: PublicationValidity::new(Timestamp(NOW), Timestamp(NOW + 600), Seconds(3600))
            .unwrap(),
        targets,
        body,
    }
}

pub struct Trio {
    pub directory: tempfile::TempDir,
    pub bindings: [MemberBinding; 3],
    pub policy: Policy,
    pub readers: [Endpoint; 3],
    mls_limits: [prns_groups_mls::Limits; 3],
}
impl Trio {
    pub fn enrolled() -> Self {
        Self::enrolled_with_mls_limits([mls_limits(); 3])
    }
    fn enrolled_with_mls_limits(mls_limits: [prns_groups_mls::Limits; 3]) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let identities = [
            LocalIdentity::generate([1; 32], 1).unwrap(),
            LocalIdentity::generate([2; 32], 1).unwrap(),
            LocalIdentity::generate([3; 32], 1).unwrap(),
        ];
        let bindings = identities.each_ref().map(LocalIdentity::binding);
        let policy = Policy::new(bindings[0].clone(), bindings.to_vec()).unwrap();
        for (peer, identity) in identities.into_iter().enumerate() {
            // Separate plaintext test identity fixture, not a platform key store.
            let (_, secret) = identity.into_parts();
            std::fs::write(
                directory.path().join(format!("identity-{peer}")),
                secret.as_ref(),
            )
            .unwrap();
            let db = Database::create(directory.path().join(format!("peer-{peer}.redb"))).unwrap();
            let mut owner = RedbStoreOwner::new(&db);
            owner
                .initialize(STORAGE, incarnation(peer), store_limits())
                .unwrap();
        }
        let peers = Self {
            directory,
            bindings,
            policy,
            readers: [endpoint(0), endpoint(1), endpoint(2)],
            mls_limits,
        };
        peers.with_adapter(0, |adapter| {
            adapter
                .create(next(0, adapter.coordinator()), GROUP)
                .unwrap();
        });
        let bob = peers.with_adapter(1, |adapter| {
            adapter
                .key_package(next(1, adapter.coordinator()), NOW)
                .unwrap()
        });
        let carol = peers.with_adapter(2, |adapter| {
            adapter
                .key_package(next(2, adapter.coordinator()), NOW)
                .unwrap()
        });
        let added = peers.with_adapter(0, |adapter| {
            adapter
                .add(
                    next(0, adapter.coordinator()),
                    GROUP,
                    &[&bob.output.messages[0], &carol.output.messages[0]],
                    NOW,
                )
                .unwrap()
        });
        for peer in [1, 2] {
            peers.with_adapter(peer, |adapter| {
                adapter
                    .join(
                        next(peer, adapter.coordinator()),
                        GROUP,
                        &added.output.messages[1],
                        NOW,
                    )
                    .unwrap();
            });
        }
        peers
    }
    pub fn identity(&self, peer: usize) -> LocalIdentity {
        let secret = std::fs::read(self.directory.path().join(format!("identity-{peer}"))).unwrap();
        LocalIdentity::from_parts(self.bindings[peer].clone(), secret.into()).unwrap()
    }
    fn with_adapter<T>(
        &self,
        peer: usize,
        action: impl FnOnce(&Adapter<RedbBackend<'_>>) -> T,
    ) -> T {
        let db = Database::open(self.directory.path().join(format!("peer-{peer}.redb"))).unwrap();
        let mut owner = RedbStoreOwner::new(&db);
        let backend = owner
            .open(STORAGE, incarnation(peer), store_limits())
            .unwrap();
        let coordinator = Coordinator::new(backend, coordinator_limits()).unwrap();
        let adapter = Adapter::new(
            coordinator,
            self.identity(peer),
            self.policy.clone(),
            self.mls_limits[peer],
        )
        .unwrap();
        action(&adapter)
    }
}
