use super::*;
use prns_core::identity::PrivateIdentityMaterial;
use runtime::{Evidence, Progress, Scope};

/// A native-only capability. It grants key access, not service dispatch. Leases
/// remain counted until their actual holders drop them; a release receipt alone
/// cannot cause the catalog owner to unlock while a task still holds its key.
pub struct KeyLease {
    inner: Arc<Key>,
    _notify: Notify,
}
struct Notify(Waker);
impl Drop for Notify {
    fn drop(&mut self) {
        self.0.wake_by_ref();
    }
}
pub(super) struct Key {
    pub material: PrivateIdentityMaterial,
    pub wake: Waker,
    // Material drops before its last ownership guard, even after a driver panic.
    pub _lock: Arc<crate::DirectoryLock>,
}
impl Clone for KeyLease {
    fn clone(&self) -> Self {
        Self::new(self.inner.clone())
    }
}
impl KeyLease {
    pub(super) fn new(inner: Arc<Key>) -> Self {
        let notify = Notify(inner.wake.clone());
        Self {
            inner,
            _notify: notify,
        }
    }
    pub fn material(&self) -> &PrivateIdentityMaterial {
        &self.inner.material
    }
    /// Sign a bounded public service claim using this already-owned key lease.
    /// This grants no registration, transmission or peer-trust authority.
    #[cfg(feature = "contact-card")]
    pub fn contact_card(
        &self,
        service: crate::contact_card::ServiceNameHash,
        display_name: &str,
    ) -> Result<Vec<u8>, core::ApiError> {
        crate::contact_card::issue(self.material(), service, display_name)
    }
}

/// Native construction hooks around the existing scoped effect protocol.
///
/// Every Pending effect must retain its result and captured exact Scope in this
/// object and wake the factory-supplied Waker when ready. Re-polling a step must
/// never repeat an already committed side effect. Async operations may finish
/// previously authorized work; fresh dispatch must occur only in the scoped
/// methods called by this owner. Never move a borrowed Controller/store into a
/// future, start an independent lifecycle loop, or own a runtime per service.
///
/// The factory and `bind_key` only assemble/retain state. Live resources may be
/// acquired only by scoped methods. Cleanup must retain pending effects and
/// their leases until completion, including effects from the previous scope.
/// Every externally retained identity task/resource holds a KeyLease even if it
/// does not directly sign; the lease also retains the exclusive native lock on
/// panic, while a successful driver release attests that those uses have ended.
pub trait Driver<const S: usize>:
    runtime::ScopedDriver<S>
    + runtime::configuration::ServiceInitializer<Error = <Self as runtime::ScopedDriver<S>>::Error>
{
    /// Advance bounded read-only observations or poll already-owned completion
    /// evidence on this same worker. Request another step with the supplied
    /// context's weak waker; retain bounded state/results in the driver. This
    /// hook grants no authority for fresh mutation, activation or lifecycle work.
    /// Disposable scans may stop at closure; actual retained effects still drain
    /// through the scoped protocol. Never block or invoke consumer callbacks.
    fn poll_observations(&mut self, _context: &mut std::task::Context<'_>) {}

    /// Resolve bounded already-owned domain effects before lifecycle progress,
    /// including while closing. The narrow context can only validate and settle
    /// an original retained permit; it cannot admit fresh work or grant network
    /// authority. Keep uncertain results/keys owned and wake this lane when
    /// evidence becomes available. Never block or invoke consumer callbacks.
    fn poll_recovery(
        &mut self,
        _context: &mut std::task::Context<'_>,
        _admission: &mut dyn RecoveryAdmission,
    ) {
    }

    /// Bind an additional restrictive dispatch fence to this exact runtime
    /// owner. It begins closed and never replaces canonical admission. Retain
    /// it with external work; no live effect may start merely from binding it.
    fn bind_dispatch(&mut self, _owner: runtime::OwnerToken, _lease: DispatchLease) {}

    /// Validate a selected adapter's public metadata requirements before create
    /// or update. Pure validation only: no acquisition or traffic.
    fn validate_metadata(&self, _metadata: core::Metadata) -> Result<(), core::Fault> {
        Ok(())
    }

    /// Current committed metadata for an exact runtime owner, supplied during
    /// activation and after metadata edits. Adapters may retain a bounded public
    /// projection; local labels must never become published service data.
    /// This notification does not authorize unsolicited network traffic.
    fn bind_metadata(
        &mut self,
        _owner: runtime::OwnerToken,
        _identity: core::IdentityReference,
        _metadata: core::Metadata,
    ) {
    }

    /// Bounded notification for each selected service after canonical activation
    /// confirmation. Retain this exact permit with the already-owned runner;
    /// never resolve a replacement generation when its callback later executes.
    /// The additional dispatch lease is open unless global closure already won.
    /// Only install/wake owned state here; scoped effects acquire resources.
    fn dispatch_opened(&mut self, _permit: runtime::NetworkPermit) {}

    fn activation_plan(
        &self,
        identity: &core::IdentityRecord<S>,
    ) -> Result<runtime::ActivationPlan<S>, <Self as runtime::ScopedDriver<S>>::Error>;
    fn dependencies(
        &mut self,
        identity: core::IdentityReference,
        services: core::ServiceSet<S>,
    ) -> Result<core::DependencyReport<S>, <Self as runtime::ScopedDriver<S>>::Error>;
    fn bind_key(&mut self, owner: runtime::OwnerToken, key: KeyLease);
    fn classify_failure(
        &self,
        error: &<Self as runtime::ScopedDriver<S>>::Error,
    ) -> core::ServiceFailure;
}

/// Catalog/key-only adapter. Unknown configured services fail explicitly; no
/// messaging, host, service registry, or implicit service initialization exists.
#[derive(Default)]
pub struct IdentityOnly;
impl<const S: usize> Driver<S> for IdentityOnly {
    fn activation_plan(
        &self,
        identity: &core::IdentityRecord<S>,
    ) -> Result<runtime::ActivationPlan<S>, runtime::Fault> {
        if identity.services.iter().next().is_some() {
            return Err(runtime::Fault::UnsupportedService);
        }
        runtime::ActivationPlan::new(
            runtime::Resources {
                held_keys: 1,
                ..runtime::Resources::ZERO
            },
            &[],
        )
    }
    fn dependencies(
        &mut self,
        _: core::IdentityReference,
        services: core::ServiceSet<S>,
    ) -> Result<core::DependencyReport<S>, runtime::Fault> {
        if services.iter().next().is_some() {
            return Err(runtime::Fault::UnsupportedService);
        }
        Ok(core::DependencyReport::empty())
    }
    fn bind_key(&mut self, _: runtime::OwnerToken, _: KeyLease) {}
    fn classify_failure(&self, _: &runtime::Fault) -> core::ServiceFailure {
        core::ServiceFailure::Unavailable(0)
    }
}
impl runtime::configuration::ServiceInitializer for IdentityOnly {
    type Error = runtime::Fault;
    fn initialize_service(
        &mut self,
        _: core::EffectTicket,
        _: core::IdentityGuard,
        _: core::KeyReference,
        _: core::ServiceId,
    ) -> Result<core::InitializedService, Self::Error> {
        Err(runtime::Fault::UnsupportedService)
    }
}
impl<const S: usize> runtime::ScopedDriver<S> for IdentityOnly {
    type Error = runtime::Fault;
    fn preflight(
        &mut self,
        scope: Scope,
        plan: &runtime::ActivationPlan<S>,
    ) -> Result<Progress<Evidence<runtime::Resources>>, Self::Error> {
        Ok(Progress::Complete(Evidence {
            scope,
            value: plan.total()?,
        }))
    }
    fn prepare_service(
        &mut self,
        _: Scope,
        _: core::KeyReference,
        _: runtime::ServicePlan,
    ) -> Result<Progress<Evidence<runtime::PreparedService>>, Self::Error> {
        Err(runtime::Fault::UnsupportedService)
    }
    fn activate_service(
        &mut self,
        _: Scope,
        _: runtime::ServicePlan,
    ) -> Result<Progress<Evidence<()>>, Self::Error> {
        Err(runtime::Fault::UnsupportedService)
    }
    fn fence_previous_workers(
        &mut self,
        scope: Scope,
    ) -> Result<Progress<Evidence<bool>>, Self::Error> {
        Ok(Progress::Complete(Evidence { scope, value: true }))
    }
    fn quiesce_service(
        &mut self,
        _: Scope,
        _: runtime::ServicePlan,
    ) -> Result<Progress<Evidence<runtime::QuiescedService>>, Self::Error> {
        Err(runtime::Fault::UnsupportedService)
    }
    fn release_identity(
        &mut self,
        scope: Scope,
    ) -> Result<Progress<Evidence<runtime::ReleasedIdentity<S>>>, Self::Error> {
        Ok(Progress::Complete(Evidence {
            scope,
            value: runtime::ReleasedIdentity {
                owned_reservations_released: true,
                shared_owners_preserved: true,
                report: core::DependencyReport::empty(),
            },
        }))
    }
}
