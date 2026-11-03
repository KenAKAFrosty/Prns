#![allow(clippy::unwrap_used, clippy::panic)]
use super::*;
use crate::{
    enrollment::Enrollment, retained, Client, ExecutionContext, GroupManager, Intent, Job, Outcome,
    Request, RequestToken, Submit,
};
use prns_groups_core::{ClockInterval, ClockReading, Timestamp};
use prns_host::IdentityHash;
use std::{
    future::Future,
    pin::pin,
    sync::{Arc, Mutex},
    task::{Context, Poll, Waker},
};
const NOW: u64 = 1_800_000_000;
fn identity() -> IdentityHash {
    IdentityHash::new([21; 16])
}
fn id(n: u8) -> RequestId {
    RequestToken::new(NOW, [n; 20]).unwrap().into()
}
fn context() -> ExecutionContext {
    ExecutionContext {
        clock: ClockReading::Trusted(ClockInterval::new(Timestamp(NOW), Timestamp(NOW)).unwrap()),
    }
}
fn unknown() -> ExecutionContext {
    ExecutionContext {
        clock: ClockReading::Unknown,
    }
}
fn intent() -> Intent {
    Intent::Create {
        profile: Default::default(),
        title: "Authorized native job".into(),
    }
}
fn initialize(db: &redb::Database) {
    let mut tx = db.begin_write().unwrap();
    tx.set_durability(redb::Durability::Immediate).unwrap();
    Enrollment::initialize_in(&tx, identity()).unwrap();
    retained::initialize_in(
        &tx,
        identity(),
        retained::Limits {
            max_operations: 8,
            max_bytes: 65536,
            ..Default::default()
        },
    )
    .unwrap();
    tx.commit().unwrap();
}
#[derive(Default)]
struct Gate {
    paused: bool,
    acquired: Vec<Access>,
    settled: Vec<(Access, Settlement)>,
    live: usize,
}
impl crate::admission::CarrierRecovery for Gate {}
impl Authorization for Gate {
    type Permit = Access;
    fn acquire(&mut self, access: Access) -> Result<Access, Error> {
        self.acquired.push(access);
        if self.paused && matches!(access, Access::ServiceMutation { .. }) {
            return Err(Error::Busy);
        }
        self.live += 1;
        Ok(access)
    }
    fn settle(&mut self, permit: Access, settlement: Settlement) -> Result<(), Error> {
        self.live -= 1;
        self.settled.push((permit, settlement));
        Ok(())
    }
}
#[derive(Default)]
struct Lane(Mutex<Option<Job>>, crate::InputBudget);
impl Submit for Lane {
    fn input_budget(&self) -> &crate::InputBudget {
        &self.1
    }
    fn submit(&self, job: Job) -> Result<(), Error> {
        let mut slot = self.0.lock().unwrap();
        if slot.is_some() {
            return Err(Error::Busy);
        }
        *slot = Some(job);
        Ok(())
    }
}
impl Lane {
    fn take(&self) -> Job {
        self.0.lock().unwrap().take().unwrap()
    }
}
async fn drive<F: Future>(
    future: F,
    lane: &Lane,
    manager: &mut GroupManager<'_>,
    gate: &mut Gate,
) -> F::Output {
    let mut future = pin!(future);
    let mut cx = Context::from_waker(Waker::noop());
    assert!(matches!(future.as_mut().poll(&mut cx), Poll::Pending));
    manager.execute_authorized(lane.take(), gate);
    future.await
}

#[tokio::test]
async fn dropped_retained_waiter_still_commits_wakes_and_reports_through_canonical_client() {
    let dir = tempfile::tempdir().unwrap();
    let db = redb::Database::create(dir.path().join("group")).unwrap();
    initialize(&db);
    let mut manager = GroupManager::open_managed(&db, identity()).unwrap();
    let mut gate = Gate::default();
    let lane = Arc::new(Lane::default());
    let client = Client::new(lane.clone());
    assert!(matches!(
        client.admit_submit(
            id(1),
            Intent::Create {
                profile: Default::default(),
                title: "x".repeat(129)
            },
            context()
        ),
        Err(prns_groups_core::ApiError::InvalidInput)
    ));
    assert!(lane.0.lock().unwrap().is_none());
    let waiter = client.admit_submit(id(1), intent(), context()).unwrap();
    drop(waiter);
    let notified = client.wake.notified();
    manager.execute_authorized(lane.take(), &mut gate);
    notified.await;
    assert!(gate
        .acquired
        .contains(&Access::ServiceMutation { request: id(1) }));
    assert_eq!(gate.live, 0);
    gate.paused = true;
    let operation = drive(client.operation(id(1)), &lane, &mut manager, &mut gate)
        .await
        .unwrap()
        .unwrap();
    let retained::State::Completed(Outcome::Created(group)) = operation.state else {
        panic!("create")
    };
    assert_eq!(gate.acquired.last(), Some(&Access::History));
    let page = drive(
        client.records(group, None, 8, ClockReading::Unknown),
        &lane,
        &mut manager,
        &mut gate,
    )
    .await
    .unwrap();
    assert!(page.records.is_empty());
    assert_eq!(gate.acquired.last(), Some(&Access::History));
    assert_eq!(
        drive(
            client.submit(id(1), intent(), unknown()),
            &lane,
            &mut manager,
            &mut gate
        )
        .await
        .unwrap(),
        operation
    );
    assert_eq!(gate.acquired.last(), Some(&Access::Recovery));
    let list = drive(
        client.operations(
            None,
            crate::PageLimits {
                max_rows: 8,
                max_bytes: 1024 * 1024,
            },
        ),
        &lane,
        &mut manager,
        &mut gate,
    )
    .await
    .unwrap();
    assert_eq!(list, vec![operation]);
    drive(
        client.dismiss_operation(id(1)),
        &lane,
        &mut manager,
        &mut gate,
    )
    .await
    .unwrap();
    assert_eq!(gate.acquired.last(), Some(&Access::LocalMutation));
    assert_eq!(gate.live, 0);
}

#[tokio::test]
async fn direct_without_selected_retention_recovers_only_and_carriers_remain_closed() {
    let dir = tempfile::tempdir().unwrap();
    let db = redb::Database::create(dir.path().join("group")).unwrap();
    let mut manager = GroupManager::create_managed(&db, identity()).unwrap();
    let mut gate = Gate::default();
    let lane = Arc::new(Lane::default());
    let client = Client::new(lane.clone());
    let request = Request {
        id: id(1),
        intent: intent(),
        original_scope: None,
    };
    assert!(matches!(
        drive(
            client.execute(request.clone(), context()),
            &lane,
            &mut manager,
            &mut gate
        )
        .await,
        Err(prns_groups_core::ApiError::Configuration)
    ));
    assert_eq!(gate.acquired, vec![Access::Recovery]);
    assert_eq!(gate.live, 0);
    assert!(manager.recover_request(request.clone()).unwrap().is_none());
    // An independent unmanaged owner has its own admission/retention contract.
    // The assembly path only recovers the exact already committed receipt.
    let expected = manager.execute_request(request.clone(), context()).unwrap();
    gate.paused = true;
    assert_eq!(
        drive(
            client.execute(request, unknown()),
            &lane,
            &mut manager,
            &mut gate
        )
        .await
        .unwrap(),
        expected
    );
    gate.acquired.clear();
    assert!(matches!(
        drive(
            client.enrollment_handle(IdentityHash::new([22; 16]), Vec::new(), NOW),
            &lane,
            &mut manager,
            &mut gate
        )
        .await,
        Err(Error::Configuration)
    ));
    assert!(gate.acquired.is_empty());
    assert_eq!(gate.live, 0);
}

#[tokio::test]
async fn queued_submit_checks_actual_pause_before_any_durable_acceptance() {
    let dir = tempfile::tempdir().unwrap();
    let db = redb::Database::create(dir.path().join("group")).unwrap();
    initialize(&db);
    let mut manager = GroupManager::open_managed(&db, identity()).unwrap();
    let mut gate = Gate::default();
    let lane = Arc::new(Lane::default());
    let client = Client::new(lane.clone());
    let waiter = client.admit_submit(id(1), intent(), context()).unwrap();
    gate.paused = true;
    manager.execute_authorized(lane.take(), &mut gate);
    assert!(matches!(
        waiter.await,
        Err(prns_groups_core::ApiError::Busy)
    ));
    assert!(manager.retained_operation(id(1)).unwrap().is_none());
    assert_eq!(gate.live, 0);
    gate.paused = false;
    assert!(drive(
        client.submit(id(1), intent(), context()),
        &lane,
        &mut manager,
        &mut gate
    )
    .await
    .unwrap()
    .state
    .is_terminal());
}

#[tokio::test]
async fn plain_manager_execution_never_substitutes_an_unmanaged_gate_for_retained_jobs() {
    let dir = tempfile::tempdir().unwrap();
    let db = redb::Database::create(dir.path().join("group")).unwrap();
    initialize(&db);
    let mut manager = GroupManager::open_managed(&db, identity()).unwrap();
    let lane = Arc::new(Lane::default());
    let client = Client::new(lane.clone());
    let waiter = client.admit_submit(id(1), intent(), context()).unwrap();
    manager.execute(lane.take());
    assert!(matches!(
        waiter.await,
        Err(prns_groups_core::ApiError::Configuration)
    ));
    assert!(manager.retained_operation(id(1)).unwrap().is_none());
}

#[tokio::test]
async fn managed_direct_caller_loss_retains_the_exact_request_and_recovers_after_dismissal() {
    let dir = tempfile::tempdir().unwrap();
    let db = redb::Database::create(dir.path().join("group")).unwrap();
    initialize(&db);
    let mut manager = GroupManager::open_managed(&db, identity()).unwrap();
    let mut gate = Gate::default();
    let lane = Arc::new(Lane::default());
    let client = Client::new(lane.clone());
    let request = Request {
        id: id(1),
        intent: intent(),
        original_scope: None,
    };
    drop(client.admit_execute(request.clone(), context()).unwrap());
    let notified = client.wake.notified();
    manager.execute_authorized(lane.take(), &mut gate);
    notified.await;
    let operation = manager.retained_operation(id(1)).unwrap().unwrap();
    assert_eq!(operation.request, request);
    let retained::State::Completed(expected) = operation.state else {
        panic!("completed")
    };
    assert_eq!(gate.live, 0);
    assert_eq!(
        gate.acquired
            .iter()
            .filter(|access| matches!(access, Access::ServiceMutation { .. }))
            .count(),
        1
    );
    gate.paused = true;
    assert_eq!(
        drive(
            client.execute(request.clone(), unknown()),
            &lane,
            &mut manager,
            &mut gate
        )
        .await
        .unwrap(),
        expected
    );
    let mut changed = request.clone();
    changed.intent = Intent::Create {
        profile: Default::default(),
        title: "Different input".into(),
    };
    assert!(matches!(
        drive(
            client.execute(changed, unknown()),
            &lane,
            &mut manager,
            &mut gate
        )
        .await,
        Err(prns_groups_core::ApiError::Rejected {
            reason: prns_groups_core::RejectionReason::RequestConflict
        })
    ));
    manager.dismiss_retained(id(1), &mut gate).unwrap();
    assert!(manager.retained_operation(id(1)).unwrap().is_none());
    // The exact domain receipt survives result dismissal. Its expired/floored
    // ID recovers before any attempt to accept a new native operation.
    assert_eq!(
        drive(
            client.execute(request, unknown()),
            &lane,
            &mut manager,
            &mut gate
        )
        .await
        .unwrap(),
        expected
    );
    assert!(manager.retained_operation(id(1)).unwrap().is_none());
    assert_eq!(
        gate.acquired
            .iter()
            .filter(|access| matches!(access, Access::ServiceMutation { .. }))
            .count(),
        1
    );
    assert_eq!(gate.live, 0);
}

#[tokio::test]
async fn queued_direct_checks_pause_before_native_acceptance() {
    let dir = tempfile::tempdir().unwrap();
    let db = redb::Database::create(dir.path().join("group")).unwrap();
    initialize(&db);
    let mut manager = GroupManager::open_managed(&db, identity()).unwrap();
    let mut gate = Gate::default();
    let lane = Arc::new(Lane::default());
    let client = Client::new(lane.clone());
    let request = Request {
        id: id(1),
        intent: intent(),
        original_scope: None,
    };
    let waiter = client.admit_execute(request.clone(), context()).unwrap();
    gate.paused = true;
    manager.execute_authorized(lane.take(), &mut gate);
    assert!(matches!(
        waiter.await,
        Err(prns_groups_core::ApiError::Busy)
    ));
    assert!(manager.retained_operation(id(1)).unwrap().is_none());
    assert!(manager.recover_request(request).unwrap().is_none());
    assert_eq!(gate.live, 0);
}

#[tokio::test]
async fn managed_direct_keeps_original_scope_and_rejects_scope_reuse() {
    let dir = tempfile::tempdir().unwrap();
    let db = redb::Database::create(dir.path().join("group")).unwrap();
    initialize(&db);
    let mut manager = GroupManager::open_managed(&db, identity()).unwrap();
    let mut gate = Gate::default();
    let lane = Arc::new(Lane::default());
    let client = Client::new(lane.clone());
    let retained::State::Completed(Outcome::Created(group)) = manager
        .submit_retained(id(1), intent(), context(), &mut gate)
        .unwrap()
        .state
    else {
        panic!("created")
    };
    let original = manager.group_scope(group).unwrap();
    let stale = crate::OriginalScope::Control {
        control: prns_groups_core::ControlReference {
            head: prns_groups_core::ControlId([55; 32]),
            era: prns_groups_core::AuthorizationEra(prns_groups_core::ControlId([55; 32])),
        },
        incarnation: original.incarnation(),
    };
    let mut request = Request {
        id: id(2),
        intent: Intent::Close { group },
        original_scope: Some(stale),
    };
    assert!(matches!(
        drive(
            client.execute(request.clone(), context()),
            &lane,
            &mut manager,
            &mut gate
        )
        .await,
        Err(prns_groups_core::ApiError::Rejected {
            reason: prns_groups_core::RejectionReason::StaleMembership
        })
    ));
    assert_eq!(
        manager.retained_operation(id(2)).unwrap().unwrap().request,
        request
    );
    assert_eq!(manager.group_scope(group).unwrap(), original);
    request.original_scope = Some(original);
    assert!(matches!(
        drive(
            client.execute(request.clone(), context()),
            &lane,
            &mut manager,
            &mut gate
        )
        .await,
        Err(prns_groups_core::ApiError::Rejected {
            reason: prns_groups_core::RejectionReason::RequestConflict
        })
    ));
    request.id = id(3);
    assert!(matches!(
        drive(
            client.execute(request, context()),
            &lane,
            &mut manager,
            &mut gate
        )
        .await
        .unwrap(),
        Outcome::Saved
    ));
    assert_eq!(gate.live, 0);
}

#[tokio::test]
async fn managed_direct_reopen_uses_the_same_retained_crash_recovery_and_admission() {
    for cut in [
        retained::fault::Cut::Accepted,
        retained::fault::Cut::DomainFinished,
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("group");
        let db = redb::Database::create(&path).unwrap();
        initialize(&db);
        let mut manager = GroupManager::open_managed(&db, identity()).unwrap();
        let mut gate = Gate::default();
        let lane = Arc::new(Lane::default());
        let client = Client::new(lane.clone());
        let request = Request {
            id: id(1),
            intent: intent(),
            original_scope: None,
        };
        retained::fault::set(cut);
        assert!(matches!(
            drive(
                client.execute(request.clone(), context()),
                &lane,
                &mut manager,
                &mut gate
            )
            .await,
            Err(prns_groups_core::ApiError::Unavailable)
        ));
        assert_eq!(gate.live, 0);
        assert_eq!(
            gate.settled.last(),
            Some(&(
                Access::ServiceMutation { request: id(1) },
                Settlement::Retained
            ))
        );
        assert_eq!(
            manager.retained_operation(id(1)).unwrap().unwrap().request,
            request
        );
        drop(manager);
        drop(db);
        let db = redb::Database::open(&path).unwrap();
        let mut manager = GroupManager::open_managed(&db, identity()).unwrap();
        gate.paused = true;
        let result = drive(
            client.execute(request.clone(), unknown()),
            &lane,
            &mut manager,
            &mut gate,
        )
        .await;
        if matches!(cut, retained::fault::Cut::Accepted) {
            assert!(matches!(result, Err(prns_groups_core::ApiError::Busy)));
            assert!(matches!(
                manager.retained_operation(id(1)).unwrap().unwrap().state,
                retained::State::Pending
            ));
            gate.paused = false;
            assert!(matches!(
                drive(
                    client.execute(request, context()),
                    &lane,
                    &mut manager,
                    &mut gate
                )
                .await
                .unwrap(),
                Outcome::Created(_)
            ));
        } else {
            assert!(matches!(result.unwrap(), Outcome::Created(_)));
        }
        assert!(manager
            .retained_operation(id(1))
            .unwrap()
            .unwrap()
            .state
            .is_terminal());
        assert_eq!(gate.live, 0);
    }
}
