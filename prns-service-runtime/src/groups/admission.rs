use super::*;

pub(super) struct PendingCarrier {
    effect: groups::admission::CarrierEffectId,
    // Canonical settlement clears this field only on success. A failed probe,
    // settlement or panic leaves the original key use owned by the Entry.
    permit: Option<owned::Permit>,
}
pub(super) struct Gate<'a> {
    pub identity: core::IdentityReference,
    pub admission: &'a mut dyn owned::Admission,
    pub network: Option<runtime::NetworkPermit>,
    pub pending: &'a mut Option<PendingCarrier>,
}
impl groups::admission::Authorization for Gate<'_> {
    type Permit = owned::Permit;
    fn acquire(
        &mut self,
        access: groups::admission::Access,
    ) -> Result<Self::Permit, groups::Error> {
        if self.pending.is_some() && access != groups::admission::Access::History {
            return Err(groups::Error::Busy);
        }
        let access = match access {
            groups::admission::Access::History => owned::Access::History,
            groups::admission::Access::Recovery => owned::Access::Recovery,
            groups::admission::Access::LocalMutation => owned::Access::LocalMutation,
            groups::admission::Access::ServiceMutation { request } => {
                // Full canonical caller ID remains durable in groups. This
                // separate domain tag only correlates one live mutation lease.
                // Keep its bytes stable across crate renames.
                let digest = prns_core::crypto::sha256_chunks(&[
                    b"prns-native/groups/mutation/v1",
                    &request.0,
                ]);
                let mut operation = [0; 16];
                operation.copy_from_slice(&digest[..16]);
                owned::Access::ServiceMutation {
                    service: SERVICE,
                    operation: runtime::MutationId(operation),
                }
            }
        };
        self.admission
            .acquire(self.identity, access)
            .map_err(map_error)
    }
    fn settle(
        &mut self,
        permit: Self::Permit,
        settlement: groups::admission::Settlement,
    ) -> Result<(), groups::Error> {
        let settlement = match settlement {
            groups::admission::Settlement::NotDispatched => runtime::Settlement::Rejected,
            groups::admission::Settlement::Completed => runtime::Settlement::Committed,
            groups::admission::Settlement::Rejected => runtime::Settlement::Rejected,
            groups::admission::Settlement::Retained => runtime::Settlement::RecoveryRetained,
        };
        self.admission.settle(permit, settlement).map_err(map_error)
    }
    fn acquire_carrier(
        &mut self,
        effect: groups::admission::CarrierEffectId,
    ) -> Result<(), groups::Error> {
        if self.pending.is_some() {
            return Err(groups::Error::Busy);
        }
        let network = self.network.ok_or(groups::Error::Unavailable)?;
        if network.service() != SERVICE || reference(network.admission().identity) != self.identity
        {
            return Err(groups::Error::Configuration);
        }
        let permit = self
            .admission
            .acquire_network(network, carrier_operation(effect))
            .map_err(map_error)?;
        // No allocation, callback or other fallible step separates admission
        // from ownership. Group dispatch starts only after this returns.
        *self.pending = Some(PendingCarrier {
            effect,
            permit: Some(permit),
        });
        Ok(())
    }
}

pub(super) struct RecoveryGate<'a> {
    pub identity: core::IdentityReference,
    pub admission: &'a mut dyn owned::RecoveryAdmission,
    pub pending: &'a mut Option<PendingCarrier>,
}
impl groups::admission::CarrierRecovery for Gate<'_> {
    fn validate_carrier(
        &mut self,
        effect: groups::admission::CarrierEffectId,
    ) -> Result<(), groups::Error> {
        RecoveryGate {
            identity: self.identity,
            admission: self.admission,
            pending: self.pending,
        }
        .validate_carrier(effect)
    }
    fn settle_carrier(
        &mut self,
        effect: groups::admission::CarrierEffectId,
        settlement: groups::admission::Settlement,
    ) -> Result<(), groups::Error> {
        RecoveryGate {
            identity: self.identity,
            admission: self.admission,
            pending: self.pending,
        }
        .settle_carrier(effect, settlement)
    }
}
impl groups::admission::CarrierRecovery for RecoveryGate<'_> {
    fn validate_carrier(
        &mut self,
        effect: groups::admission::CarrierEffectId,
    ) -> Result<(), groups::Error> {
        let pending = self.pending.as_ref().ok_or(groups::Error::Configuration)?;
        if pending.effect != effect {
            return Err(groups::Error::Configuration);
        }
        self.admission
            .validate_existing(
                self.identity,
                SERVICE,
                carrier_operation(effect),
                pending
                    .permit
                    .as_ref()
                    .ok_or(groups::Error::Configuration)?,
            )
            .map_err(map_error)
    }
    fn settle_carrier(
        &mut self,
        effect: groups::admission::CarrierEffectId,
        settlement: groups::admission::Settlement,
    ) -> Result<(), groups::Error> {
        let settlement = match settlement {
            groups::admission::Settlement::Completed => runtime::Settlement::Committed,
            groups::admission::Settlement::NotDispatched
            | groups::admission::Settlement::Rejected => runtime::Settlement::Rejected,
            // This path has no durable retained-client row to take ownership.
            groups::admission::Settlement::Retained => return Err(groups::Error::Configuration),
        };
        self.validate_carrier(effect)?;
        let pending = self.pending.as_mut().ok_or(groups::Error::Configuration)?;
        self.admission
            .settle_existing(&mut pending.permit, settlement)
            .map_err(map_error)?;
        *self.pending = None;
        Ok(())
    }
}
fn carrier_operation(effect: groups::admission::CarrierEffectId) -> runtime::MutationId {
    // Domain separation is stable across crate renames.
    let digest =
        prns_core::crypto::sha256_chunks(&[b"prns-native/groups/carrier/v1", effect.as_bytes()]);
    let mut operation = [0; 16];
    operation.copy_from_slice(&digest[..16]);
    runtime::MutationId(operation)
}
pub(super) fn map_error(error: owned::AdmissionError) -> groups::Error {
    match error {
        owned::AdmissionError::Authority(core::Fault::Busy)
        | owned::AdmissionError::Runtime(runtime::Fault::CapacityExceeded) => groups::Error::Busy,
        _ => groups::Error::Unavailable,
    }
}
