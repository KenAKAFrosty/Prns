//! Authenticated protocol effects use the same bounded carrier slot as records.
use super::*;
use crate::admission::{Access, Authorization, CarrierEffectId, Settlement};
use crate::attachment::PendingCarrier;
use crate::client::{Command as NativeCommand, Reply as NativeReply};
use crate::Error;

impl<'db> Enrollment<'db> {
    pub(crate) fn execute_carrier<A: Authorization>(
        &self,
        owner: &mut RedbStoreOwner<'db>,
        command: NativeCommand,
        pending: &mut Option<PendingCarrier>,
        authority: &mut A,
    ) -> Result<NativeReply, Error> {
        if pending.is_some() {
            return Err(Error::Busy);
        }
        match command {
            NativeCommand::EnrollmentHandle {
                requester,
                frame,
                now,
            } => {
                let permit = authority.acquire(Access::Recovery)?;
                let prepared = self.prepare_handle(&frame);
                authority.settle(permit, Settlement::NotDispatched)?;
                let prepared = prepared.map_err(Error::Enrollment)?;
                let effect = self.carrier_effect(b"handle", requester.as_bytes(), &frame);
                self.run_carrier(effect, pending, authority, |engine| {
                    engine
                        .handle_prepared(owner, requester, prepared, now)
                        .map(NativeReply::EnrollmentFrame)
                })
            }
            NativeCommand::EnrollmentComplete { id, response, now } => {
                let permit = authority.acquire(Access::Recovery)?;
                let prepared = self.prepare_complete(owner, id, &response, now);
                authority.settle(permit, Settlement::NotDispatched)?;
                let Some((work, reply)) = prepared.map_err(Error::Enrollment)? else {
                    return Ok(NativeReply::EnrollmentCompleted(CompletionActivity::Idle));
                };
                let effect = self.carrier_effect(b"complete", &id, &response);
                self.run_carrier(effect, pending, authority, |engine| {
                    engine
                        .complete_prepared(owner, work, reply, now)
                        .map(NativeReply::EnrollmentCompleted)
                })
            }
            _ => Err(Error::Configuration),
        }
    }
    fn carrier_effect(&self, kind: &[u8], peer_or_work: &[u8], input: &[u8]) -> CarrierEffectId {
        let mut digest = Sha256::new();
        digest.update(b"prns.groups.native.carrier.enrollment.v1\0");
        digest.update(kind);
        digest.update(self.local.as_bytes());
        digest.update(peer_or_work);
        digest.update(input);
        CarrierEffectId(digest.finalize().into())
    }
    fn run_carrier<A: Authorization>(
        &self,
        effect: CarrierEffectId,
        pending: &mut Option<PendingCarrier>,
        authority: &mut A,
        action: impl FnOnce(&Self) -> Result<NativeReply, EnrollmentError>,
    ) -> Result<NativeReply, Error> {
        let attempt = attempt::Attempt::new();
        let engine = self.with_attempt(attempt.clone());
        let continuation = PendingCarrier::Enrollment(attempt::PendingEnrollment::new(
            self.local,
            effect,
            attempt.clone(),
        ));
        // Allocate every shared witness and prepare every continuation field
        // before the embedding reserves the original permit and key guard.
        authority.acquire_carrier(effect)?;
        *pending = Some(continuation);
        let result = action(&engine);
        #[cfg(test)]
        attempt::fault::after_effect()?;
        if result.is_ok() {
            attempt.completed();
        }
        let settlement = pending
            .as_ref()
            .ok_or(Error::Corrupt)?
            .settlement(self.database)?;
        authority.validate_carrier(effect)?;
        authority.settle_carrier(effect, settlement)?;
        *pending = None;
        result.map_err(Error::Enrollment)
    }
}
