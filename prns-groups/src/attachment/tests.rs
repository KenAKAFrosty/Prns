#![allow(clippy::unwrap_used, clippy::panic)]
use super::*;
use crate::admission::{CarrierEffectId, CarrierRecovery, Settlement};
use prns_groups_store::{AtomicBackend, CommitOutcome, OperationTicket, Revision};
use prns_groups_store_redb::{change, RedbStoreOwner, StoreNamespace};

#[derive(Default)]
struct Recovery {
    validation_fails: bool,
    settlement_fails: bool,
    settled: Vec<Settlement>,
}
impl CarrierRecovery for Recovery {
    fn validate_carrier(&mut self, _: CarrierEffectId) -> Result<(), Error> {
        if self.validation_fails {
            Err(Error::Unavailable)
        } else {
            Ok(())
        }
    }
    fn settle_carrier(&mut self, _: CarrierEffectId, settlement: Settlement) -> Result<(), Error> {
        if self.settlement_fails {
            return Err(Error::Unavailable);
        }
        self.settled.push(settlement);
        Ok(())
    }
}
fn setup(database: &Database) -> Attachment {
    let identity = IdentityHash::new([44; 16]);
    let tx = database.begin_write().unwrap();
    Enrollment::initialize_in(&tx, identity).unwrap();
    change::initialize_in(&tx, StoreIncarnation::new([41; 24]).unwrap()).unwrap();
    tx.commit().unwrap();
    let incarnation = StoreIncarnation::new([42; 24]).unwrap();
    RedbStoreOwner::new(database)
        .initialize(
            StoreNamespace([43; 16]),
            incarnation,
            crate::profile::store_limits(),
        )
        .unwrap();
    let mut attachment = Attachment::open(database, identity).unwrap();
    attachment.carrier = Some(PendingCarrier::Receive(PendingReceive {
        namespace: StoreNamespace([43; 16]),
        ticket: incarnation.first(),
        limits: crate::profile::store_limits(),
        effect: CarrierEffectId([45; 32]),
        known: None,
    }));
    attachment
}

#[test]
fn exact_carrier_probe_preserves_both_fences_on_authority_or_settlement_errors() {
    let dir = tempfile::tempdir().unwrap();
    let database = Database::create(dir.path().join("group")).unwrap();
    let mut attachment = setup(&database);
    let effect = attachment.pending_carrier().unwrap();
    assert!(matches!(
        attachment.set_profile_support(
            &database,
            crate::ProfileSupport::new(usize::MAX, usize::MAX).unwrap()
        ),
        Err(Error::Busy)
    ));
    assert!(attachment.profile_support.is_none());
    let mut recovery = Recovery {
        validation_fails: true,
        ..Recovery::default()
    };
    assert!(attachment
        .recover_carrier(&database, &mut recovery)
        .is_err());
    assert_eq!(attachment.pending_carrier(), Some(effect));
    recovery.validation_fails = false;
    recovery.settlement_fails = true;
    assert!(attachment
        .recover_carrier(&database, &mut recovery)
        .is_err());
    assert_eq!(attachment.pending_carrier(), Some(effect));
    assert!(recovery.settled.is_empty());
    recovery.settlement_fails = false;
    assert!(attachment
        .recover_carrier(&database, &mut recovery)
        .unwrap());
    assert_eq!(recovery.settled, vec![Settlement::Rejected]);
    assert!(attachment.pending_carrier().is_none());
    assert!(!attachment
        .recover_carrier(&database, &mut recovery)
        .unwrap());
}

#[test]
fn exact_carrier_probe_distinguishes_committed_from_unproven_or_foreign_attempts() {
    let dir = tempfile::tempdir().unwrap();
    let database = Database::create(dir.path().join("group")).unwrap();
    let mut attachment = setup(&database);
    let Some(PendingCarrier::Receive(pending)) = attachment.carrier.as_ref() else {
        panic!("expected receive")
    };
    let pending = *pending;
    let mut owner = RedbStoreOwner::new(&database);
    let mut backend = owner
        .open(
            pending.namespace,
            pending.ticket.incarnation(),
            pending.limits,
        )
        .unwrap();
    assert!(matches!(
        backend
            .commit(Revision(0), pending.ticket.operation_id(), &[])
            .unwrap(),
        CommitOutcome::Committed(_)
    ));
    let mut recovery = Recovery::default();
    assert!(attachment
        .recover_carrier(&database, &mut recovery)
        .unwrap());
    assert_eq!(recovery.settled, vec![Settlement::Completed]);
    // A future ticket beyond the exact next revision cannot resolve a captured
    // attempt. Its absence is not evidence of a definite original noncommit.
    attachment.carrier = Some(PendingCarrier::Receive(PendingReceive {
        ticket: OperationTicket::new(pending.ticket.incarnation(), 3).unwrap(),
        ..pending
    }));
    assert!(attachment
        .recover_carrier(&database, &mut recovery)
        .is_err());
    assert!(attachment.pending_carrier().is_some());
    let foreign = Database::create(dir.path().join("foreign")).unwrap();
    let tx = foreign.begin_write().unwrap();
    change::initialize_in(&tx, StoreIncarnation::new([99; 24]).unwrap()).unwrap();
    tx.commit().unwrap();
    assert!(matches!(
        attachment.recover_carrier(&foreign, &mut recovery),
        Err(Error::Configuration)
    ));
    assert!(attachment.pending_carrier().is_some());
    assert_eq!(recovery.settled, vec![Settlement::Completed]);
}
