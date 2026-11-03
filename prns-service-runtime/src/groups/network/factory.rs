use super::*;
use crate::groups as assembly;
use prns_identities::owned;

/// Explicit storage selection, resource budget and carrier policy. No product
/// profile or app state is introduced by the network constructor.
#[derive(Clone, Copy)]
pub struct OwnerOptions {
    pub storage: assembly::Options,
    pub service: RunOptions,
}
impl OwnerOptions {
    /// Validate construction budgets before host or filesystem effects.
    pub fn validate<const I: usize, const O: usize, const S: usize, const L: usize>(
        self,
    ) -> Result<Self, crate::local::Error<DriverError>> {
        let storage = self.storage.validate().map_err(|error| {
            crate::local::Error::Identity(owned::Error::Driver(DriverError::Local(
                assembly::Error::Schema(assembly::SchemaError::Group(error)),
            )))
        })?;
        storage
            .local
            .identities
            .validate::<I, O, S, L>()
            .map_err(crate::local::Error::InvalidOptions)?;
        storage
            .local
            .stores
            .validate::<I>()
            .map_err(crate::local::Error::Stores)?;
        Driver::<I, S, assembly::Command>::validate_options(self.service)
            .map_err(|error| crate::local::Error::Identity(owned::Error::Driver(error)))?;
        Driver::<I, S, assembly::Command>::validate_client_limits(self.service, storage.client)
            .map_err(|error| crate::local::Error::Identity(owned::Error::Driver(error)))?;
        Ok(Self {
            storage,
            service: self.service,
        })
    }
}
pub type Owner<const I: usize, const O: usize, const S: usize, const L: usize> =
    assembly::Owner<I, O, S, L, Driver<I, S, Command>, Command>;

pub fn create_new<const I: usize, const O: usize, const S: usize, const L: usize>(
    path: impl AsRef<std::path::Path>,
    options: OwnerOptions,
    context: Context,
) -> Result<Owner<I, O, S, L>, crate::local::Error<DriverError>> {
    start(path.as_ref(), true, options, Some(context))
}
/// Strictly reopen configured schemas. Saved enablement is preserved but must
/// be explicitly recovered; no endpoint or runner starts during construction.
pub fn open_existing<const I: usize, const O: usize, const S: usize, const L: usize>(
    path: impl AsRef<std::path::Path>,
    options: OwnerOptions,
    context: Context,
) -> Result<Owner<I, O, S, L>, crate::local::Error<DriverError>> {
    start(path.as_ref(), false, options, Some(context))
}
pub fn create_new_detached<const I: usize, const O: usize, const S: usize, const L: usize>(
    path: impl AsRef<std::path::Path>,
    options: OwnerOptions,
) -> Result<Owner<I, O, S, L>, crate::local::Error<DriverError>> {
    start(path.as_ref(), true, options, None)
}
pub fn open_existing_detached<const I: usize, const O: usize, const S: usize, const L: usize>(
    path: impl AsRef<std::path::Path>,
    options: OwnerOptions,
) -> Result<Owner<I, O, S, L>, crate::local::Error<DriverError>> {
    start(path.as_ref(), false, options, None)
}
fn start<const I: usize, const O: usize, const S: usize, const L: usize>(
    path: &std::path::Path,
    create: bool,
    options: OwnerOptions,
    context: Option<Context>,
) -> Result<Owner<I, O, S, L>, crate::local::Error<DriverError>> {
    let options = options.validate::<I, O, S, L>()?;
    let storage = options.storage;
    let input = prns_groups::InputBudget::new(storage.client).map_err(|error| {
        crate::local::Error::Identity(owned::Error::Driver(DriverError::Local(
            assembly::Error::Schema(assembly::SchemaError::Group(prns_groups::Error::Api(error))),
        )))
    })?;
    let state = assembly::SubmissionState::new::<I>(input).map_err(|error| {
        crate::local::Error::Identity(owned::Error::Driver(DriverError::Local(error)))
    })?;
    let worker_input = state.clone();
    let factory = move |stores,
                        catalog: &prns_identities_core::Catalog<I, O, S>,
                        wake: std::task::Waker,
                        commands| {
        let local =
            assembly::factory::configure(stores, catalog, wake.clone(), storage, worker_input)?;
        match context {
            Some(context) => Driver::new(
                local,
                context,
                commands,
                Command::Group,
                options.service,
                wake,
            ),
            None => Driver::new_detached(local, commands, Command::Group, options.service, wake),
        }
    };
    let owner = if create {
        crate::local::Owner::create_new(path, storage.local, factory)
    } else {
        crate::local::Owner::open_existing(path, storage.local, factory)
    }?;
    Ok(assembly::Owner::from_local(owner, state))
}
