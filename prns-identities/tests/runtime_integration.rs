//! Real native catalog/key storage composed with the bounded runtime controller.
//! Service traffic and retained application work are fixtures, not a live host.
#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::panic, clippy::drop_non_drop)]

use prns_core::identity::PrivateIdentityMaterial;
use prns_identities::IdentityStore;
use prns_identities_core as core;
use prns_identities_runtime as runtime;
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

type Disk = IdentityStore<2, 16, 1>;
type Catalog = core::Catalog<2, 16, 1>;
type Coordinator<'a> = core::Coordinator<'a, 2, 16, 1>;
type Controller<'a> = runtime::Controller<'a, 2, 1, 4>;
const SERVICE: core::ServiceId = core::ServiceId(19);
static NEXT: AtomicU64 = AtomicU64::new(0);

struct Root(PathBuf);
impl Root {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!(
            "prns-identity-composition-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )))
    }
}
impl Drop for Root {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn resources(keys: u32, services: u32) -> runtime::Resources {
    runtime::Resources {
        held_keys: keys,
        destinations: services,
        request_handlers: services,
        links: services,
        queue_slots: services,
    }
}
fn budget() -> runtime::Budget {
    // Two application identities plus management's three keys/destinations and
    // two handlers. Management resources never enter a persona driver scope.
    runtime::Budget {
        capacity: runtime::Resources {
            held_keys: 5,
            destinations: 5,
            request_handlers: 4,
            links: 2,
            queue_slots: 2,
        },
        infrastructure_reserve: runtime::Resources {
            held_keys: 3,
            destinations: 3,
            request_handlers: 2,
            links: 0,
            queue_slots: 0,
        },
    }
}
fn plan() -> runtime::ActivationPlan<1> {
    runtime::ActivationPlan::new(
        resources(1, 0),
        &[runtime::ServicePlan {
            service: SERVICE,
            resources: resources(0, 1),
            requires_receive_state: true,
            capabilities: runtime::Capabilities::QUALIFIED,
        }],
    )
    .unwrap()
}

#[derive(Default)]
struct Services {
    active: Vec<core::LocalIdentityId>,
    accepted: Vec<(core::LocalIdentityId, runtime::MutationId)>,
    dispatches: usize,
}
impl Services {
    fn dependency(&self, id: core::LocalIdentityId) -> core::ServiceDependency {
        let pending = self
            .accepted
            .iter()
            .filter(|(owner, _)| *owner == id)
            .count() as u32;
        let mut d = core::ServiceDependency::clear(SERVICE, u64::from(pending));
        d.pending_messages = pending;
        d
    }
    fn report(&self, id: core::LocalIdentityId) -> core::DependencyReport<1> {
        core::DependencyReport::new(&[self.dependency(id)], 0).unwrap()
    }
    fn done<T>(
        &mut self,
        scope: runtime::Scope,
        value: T,
    ) -> Result<runtime::Progress<runtime::Evidence<T>>, ()> {
        self.dispatches += 1;
        Ok(runtime::Progress::Complete(runtime::Evidence {
            scope,
            value,
        }))
    }
}
impl runtime::ScopedDriver<1> for Services {
    type Error = ();
    fn preflight(
        &mut self,
        scope: runtime::Scope,
        plan: &runtime::ActivationPlan<1>,
    ) -> Result<runtime::Progress<runtime::Evidence<runtime::Resources>>, ()> {
        self.done(scope, plan.total().unwrap())
    }
    fn prepare_service(
        &mut self,
        scope: runtime::Scope,
        _key: core::KeyReference,
        _plan: runtime::ServicePlan,
    ) -> Result<runtime::Progress<runtime::Evidence<runtime::PreparedService>>, ()> {
        self.done(
            scope,
            runtime::PreparedService {
                storage_ready: true,
                receive_state_restored: true,
            },
        )
    }
    fn activate_service(
        &mut self,
        scope: runtime::Scope,
        _plan: runtime::ServicePlan,
    ) -> Result<runtime::Progress<runtime::Evidence<()>>, ()> {
        if !self.active.contains(&scope.identity.id) {
            self.active.push(scope.identity.id);
        }
        self.done(scope, ())
    }
    fn fence_previous_workers(
        &mut self,
        scope: runtime::Scope,
    ) -> Result<runtime::Progress<runtime::Evidence<bool>>, ()> {
        self.done(scope, true)
    }
    fn quiesce_service(
        &mut self,
        scope: runtime::Scope,
        _plan: runtime::ServicePlan,
    ) -> Result<runtime::Progress<runtime::Evidence<runtime::QuiescedService>>, ()> {
        self.active.retain(|id| *id != scope.identity.id);
        self.done(
            scope,
            runtime::QuiescedService {
                evidence: runtime::QuiescenceEvidence::COMPLETE,
                dependencies: self.dependency(scope.identity.id),
            },
        )
    }
    fn release_identity(
        &mut self,
        scope: runtime::Scope,
    ) -> Result<runtime::Progress<runtime::Evidence<runtime::ReleasedIdentity<1>>>, ()> {
        self.done(
            scope,
            runtime::ReleasedIdentity {
                owned_reservations_released: true,
                shared_owners_preserved: true,
                report: self.report(scope.identity.id),
            },
        )
    }
}

fn create(disk: &mut Disk, c: &mut Coordinator<'_>, label: &str, tag: u8) -> core::LocalIdentityId {
    let command = core::Command::Create(core::CreateRequest {
        source: core::KeySource::Generate,
        metadata: core::Metadata::new(label, None).unwrap(),
        services: core::ServiceSet::new(&[SERVICE]).unwrap(),
    });
    let token = c
        .reserve(disk, command, core::CommandDigest([tag; 32]))
        .unwrap();
    c.begin(disk, token, &command).unwrap();
    let ticket = c.action(token).unwrap().unwrap().ticket();
    let key = disk.publish_key(c, ticket, None).unwrap();
    let core::OperationResult::Created { identity } = c.ack_key(disk, ticket, key).unwrap() else {
        panic!("creation")
    };
    identity
}

fn enable_action(
    disk: &mut Disk,
    c: &mut Coordinator<'_>,
    services: &Services,
    id: core::LocalIdentityId,
    tag: u8,
) -> core::Action<1> {
    let prepared = c
        .prepare(id, core::TransitionTarget::Enable, services.report(id))
        .unwrap();
    let command = core::Command::Transition(prepared);
    let token = c
        .reserve(disk, command, core::CommandDigest([tag; 32]))
        .unwrap();
    c.begin(disk, token, &command).unwrap();
    c.action(token).unwrap().unwrap()
}

fn activate(
    disk: &mut Disk,
    c: &mut Coordinator<'_>,
    runtime: &mut Controller<'_>,
    services: &mut Services,
    action: core::Action<1>,
) -> runtime::OwnerToken {
    let ticket = action.ticket();
    let material = PrivateIdentityMaterial::from(disk.activate_key(c, ticket).unwrap());
    assert_eq!(
        material.identity_hash().as_bytes(),
        &c.identity(ticket.identity).unwrap().public_hash.0
    );
    let receipt = {
        let authority = disk.runtime_authority(c);
        let owner = runtime
            .begin_activation(&authority, action, plan())
            .unwrap();
        assert_eq!(
            runtime
                .fence_previous_workers(&authority, services, owner)
                .unwrap(),
            runtime::Progress::Complete(())
        );
        assert_eq!(
            runtime.preflight(&authority, services, owner).unwrap(),
            runtime::Progress::Complete(())
        );
        assert_eq!(
            runtime
                .prepare_service(&authority, services, owner, SERVICE)
                .unwrap(),
            runtime::Progress::Complete(())
        );
        assert_eq!(
            runtime
                .activate_service(&authority, services, owner, SERVICE)
                .unwrap(),
            runtime::Progress::Complete(())
        );
        runtime.finish_activation(&authority, owner).unwrap()
    };
    c.ack_activated(disk, receipt.ticket()).unwrap();
    let guard = c
        .admit(
            ticket.identity,
            c.identity(ticket.identity).unwrap().public_hash,
            core::AdmissionClass::Network,
        )
        .unwrap();
    runtime
        .confirm_activation(&disk.runtime_authority(c), receipt, guard)
        .unwrap();
    receipt.owner()
}

#[test]
fn two_real_keys_pause_and_reopen_with_isolated_service_work_and_management_reserve() {
    let root = Root::new();
    let mut disk = Disk::create_new(&root.0).unwrap();
    let mut working = Catalog::new([0; 16]);
    let mut staging = working.clone();
    let mut c = Coordinator::open(&mut disk, &mut working, &mut staging).unwrap();
    let first = create(&mut disk, &mut c, "Sensor", 1);
    let second = create(&mut disk, &mut c, "Control", 2);
    let first_hash = c.identity(first).unwrap().public_hash;
    let second_hash = c.identity(second).unwrap().public_hash;
    assert_ne!(first_hash, second_hash);
    let mut state = runtime::RuntimeState::new([1; 16], budget()).unwrap();
    let mut r = Controller::new(&mut state);
    let mut services = Services::default();
    for (id, tag) in [(first, 3), (second, 4)] {
        let action = enable_action(&mut disk, &mut c, &services, id, tag);
        activate(&mut disk, &mut c, &mut r, &mut services, action);
    }
    assert_eq!(r.state().reserved_resources(), resources(2, 2));
    let first_guard = c
        .admit(first, first_hash, core::AdmissionClass::ServiceMutation)
        .unwrap();
    let request = runtime::MutationId([0x73; 16]);
    let lease = r
        .admit_mutation(&disk.runtime_authority(&c), first_guard, SERVICE, request)
        .unwrap();
    let second_guard = c
        .admit(second, second_hash, core::AdmissionClass::Network)
        .unwrap();
    let second_permit = r
        .network_permit(&disk.runtime_authority(&c), second_guard, SERVICE)
        .unwrap();
    let pause = c
        .prepare(first, core::TransitionTarget::Pause, services.report(first))
        .unwrap();
    let token = c
        .apply_stop(&mut disk, pause, core::CommandDigest([5; 32]))
        .unwrap();
    let owner = r
        .begin_quiescence(
            &disk.runtime_authority(&c),
            c.action(token).unwrap().unwrap(),
        )
        .unwrap();
    assert!(matches!(
        r.quiesce_service(&disk.runtime_authority(&c), &mut services, owner, SERVICE),
        Err(runtime::Error::Fault(runtime::Fault::MutationsPending))
    ));
    // The already admitted service mutation settles under its original identity
    // after admission closes. Its retained result outlives runtime shutdown.
    services.accepted.push((first, request));
    r.complete_mutation(lease, runtime::Settlement::Committed)
        .unwrap();
    r.quiesce_service(&disk.runtime_authority(&c), &mut services, owner, SERVICE)
        .unwrap();
    let runtime::Progress::Complete(receipt) = r
        .finish_quiescence(&disk.runtime_authority(&c), &mut services, owner)
        .unwrap()
    else {
        panic!("quiescence")
    };
    c.ack_quiesced(&mut disk, receipt.ticket(), receipt.report())
        .unwrap();
    r.confirm_quiescence(&disk.runtime_authority(&c), receipt)
        .unwrap();
    r.validate_network(&disk.runtime_authority(&c), second_permit)
        .unwrap();
    assert_eq!(services.active, vec![second]);
    assert_eq!(services.accepted, vec![(first, request)]);
    assert_eq!(r.state().reserved_resources(), resources(1, 1));
    assert_eq!(r.state().budget(), budget());
    drop(r);
    drop(c);
    drop(disk);

    // Simulate process loss of runtime objects, retaining the service fixture's
    // accepted record; the real identity catalog/key files are reopened.
    services.active.clear();
    let mut disk = Disk::open(&root.0).unwrap();
    let mut c = Coordinator::open(&mut disk, &mut working, &mut staging).unwrap();
    let mut state = runtime::RuntimeState::new([2; 16], budget()).unwrap();
    let mut r = Controller::new(&mut state);
    assert_eq!(c.identity(first).unwrap().public_hash, first_hash);
    assert!(c.recovery_action(first).unwrap().is_none());
    let restore = c.recovery_action(second).unwrap().unwrap();
    activate(&mut disk, &mut c, &mut r, &mut services, restore);
    let action = enable_action(&mut disk, &mut c, &services, first, 6);
    activate(&mut disk, &mut c, &mut r, &mut services, action);
    assert_eq!(services.accepted, vec![(first, request)]);
    assert_eq!(r.state().reserved_resources(), resources(2, 2));
    assert!(r
        .validate_network(&disk.runtime_authority(&c), second_permit)
        .is_err());
}

#[test]
fn newer_durable_session_prevents_old_runtime_dispatch_even_with_matching_local_ticket() {
    let root = Root::new();
    let mut disk = Disk::create_new(&root.0).unwrap();
    let mut working = Catalog::new([0; 16]);
    let mut staging = working.clone();
    let mut c = Coordinator::open(&mut disk, &mut working, &mut staging).unwrap();
    let id = create(&mut disk, &mut c, "Sensor", 1);
    let mut services = Services::default();
    let action = enable_action(&mut disk, &mut c, &services, id, 2);
    let mut state = runtime::RuntimeState::new([3; 16], budget()).unwrap();
    let mut r = Controller::new(&mut state);
    let owner = r
        .begin_activation(&disk.runtime_authority(&c), action, plan())
        .unwrap();
    let mut new_working = Catalog::new([0; 16]);
    let mut new_staging = new_working.clone();
    let newer = Coordinator::open(&mut disk, &mut new_working, &mut new_staging).unwrap();
    assert!(newer.snapshot().session > c.snapshot().session);
    assert!(
        c.validate_effect(action.ticket()).is_ok(),
        "local check alone cannot see durable replacement"
    );
    assert!(matches!(
        r.fence_previous_workers(&disk.runtime_authority(&c), &mut services, owner),
        Err(runtime::Error::Fault(runtime::Fault::AuthorityUnavailable))
    ));
    assert_eq!(services.dispatches, 0);
}
