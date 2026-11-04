//! One fresh receive attempt, retaining its exact physical recovery identity.
use super::*;
use crate::{
    admission::{Access, Authorization, CarrierEffectId, Settlement},
    attachment::{PendingCarrier, PendingReceive},
};
use prns_groups_reticulum::SyncOptions;
use sha2::{Digest, Sha256};

enum Prepared {
    Recovered(Incoming),
    Fresh(OperationTicket),
}

pub(crate) fn receive_authorized_with_owner<A: Authorization>(
    database: &Database,
    owner: &mut RedbStoreOwner<'_>,
    cfg: &Provisioned,
    input: (&[u8], SyncOptions),
    pending: &mut Option<PendingCarrier>,
    authority: &mut A,
) -> Result<Reply, Error> {
    let (wire, options) = input;
    if pending.is_some() {
        return Err(Error::Busy);
    }
    if cfg.owner_identity.is_none() || options.incarnation != cfg.incarnation {
        return Err(Error::Configuration);
    }
    // The same service limit applies before any ticket or carrier permit is
    // reserved; oversized input never enters a physical transaction.
    if wire.len() > cfg.mls_limits.max_wire_bytes {
        return Err(Error::InvalidInput);
    }
    let permit = authority.acquire(Access::Recovery)?;
    let prepared = SessionView {
        database,
        owner,
        provisioned: cfg,
    }
    .with_service(|service, cfg| {
        maintain(service, cfg, options.clock)?;
        if let Some(incoming) = service.received_result(wire)? {
            if let Incoming::Record(record) = incoming {
                repair_receipt(service, cfg, record)?;
            }
            return Ok(Prepared::Recovered(incoming));
        }
        Ok(Prepared::Fresh(
            OperationTicket::from_operation_id(next(service, cfg)?).map_err(|_| Error::Corrupt)?,
        ))
    });
    authority.settle(permit, Settlement::NotDispatched)?;
    let ticket = match prepared? {
        Prepared::Recovered(incoming) => return Ok(Reply::Incoming(incoming)),
        Prepared::Fresh(ticket) => ticket,
    };
    let mut digest = Sha256::new();
    digest.update(b"prns.groups.native.carrier.receive.v1\0");
    digest.update(cfg.incarnation.as_bytes());
    digest.update(prns_groups_service::envelope_id(cfg.group, wire).0);
    let effect = CarrierEffectId(digest.finalize().into());
    // The embedding stores the original Permit+KeyLease before this succeeds.
    // No allocation/callback separates that reservation from the group fence.
    authority.acquire_carrier(effect)?;
    *pending = Some(PendingCarrier::Receive(PendingReceive {
        namespace: cfg.namespace,
        ticket,
        limits: cfg.store_limits,
        effect,
        known: None,
    }));
    #[cfg(test)]
    fault::cut(fault::Cut::BeforeReceive)?;
    let mut entered = false;
    let result = SessionView {
        database,
        owner,
        provisioned: cfg,
    }
    .with_service(|service, _| {
        entered = true;
        service
            .receive(ticket.operation_id(), wire, options.clock)
            .map_err(Error::from)
    });
    #[cfg(test)]
    fault::cut(fault::Cut::AfterReceive)?;
    let Some(PendingCarrier::Receive(continuation)) = pending.as_mut() else {
        return Err(Error::Corrupt);
    };
    if !entered {
        // Managed service construction is read-only; the actual receive was
        // never invoked. This records a stage fact, not an error inference.
        continuation.known = Some(Settlement::NotDispatched);
    } else if result.is_ok() {
        continuation.known = Some(Settlement::Completed);
    } else if result.as_ref().is_err_and(exact_rejection) {
        continuation.known = Some(Settlement::Rejected);
    }
    let settlement = continuation.settlement(database)?;
    authority.validate_carrier(effect)?;
    authority.settle_carrier(effect, settlement)?;
    *pending = None;
    let incoming = result?;
    // Receipt emission has its own already-committed durable q intent. It must
    // not blur the receive's physical completion or consume fresh authority.
    if let Incoming::Record(record) = incoming {
        let permit = authority.acquire(Access::Recovery)?;
        let repair = SessionView {
            database,
            owner,
            provisioned: cfg,
        }
        .with_service(|service, cfg| repair_receipt(service, cfg, record));
        authority.settle(permit, Settlement::NotDispatched)?;
        repair?;
    }
    Ok(Reply::Incoming(incoming))
}

fn exact_rejection(error: &Error) -> bool {
    use prns_groups_mls::Error as MlsError;
    use prns_groups_service::Error as ServiceError;
    use prns_groups_store::StoreError;
    matches!(
        error,
        Error::Service(ServiceError::Store(StoreError::OperationRejected(_)))
            | Error::Service(ServiceError::Security(MlsError::Store(
                StoreError::OperationRejected(_)
            )))
            | Error::Service(ServiceError::Security(MlsError::NotCommitted))
    )
}

#[cfg(test)]
pub(crate) mod fault {
    use std::cell::Cell;
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(crate) enum Cut {
        BeforeReceive,
        AfterReceive,
    }
    thread_local! { static NEXT: Cell<Option<Cut>> = const { Cell::new(None) }; }
    pub(crate) fn set(cut: Cut) {
        NEXT.with(|next| next.set(Some(cut)));
    }
    pub(super) fn cut(at: Cut) -> Result<(), crate::Error> {
        NEXT.with(|next| {
            if next.get() == Some(at) {
                next.set(None);
                Err(crate::Error::Unavailable)
            } else {
                Ok(())
            }
        })
    }
}
