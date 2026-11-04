use super::*;

#[derive(Clone, Copy, Debug)]
pub struct Options {
    pub local: crate::local::Options,
    pub retention: groups::retained::Limits,
    catalog: groups::enrollment::CatalogLimits,
    pub(super) client: groups::ClientLimits,
    validation_cache_entries: std::num::NonZeroUsize,
    #[cfg(feature = "archive")]
    pub(super) archive: Option<groups::archive::Limits>,
}
impl Options {
    /// Catalog policy governs fresh service initialization; reopening retains
    /// its committed selection. Client limits apply to this owner's input lane.
    pub fn new(
        local: crate::local::Options,
        retention: groups::retained::Limits,
        catalog: groups::enrollment::CatalogLimits,
        client: groups::ClientLimits,
    ) -> Self {
        Self {
            local,
            retention,
            catalog,
            client,
            validation_cache_entries: std::num::NonZeroUsize::MIN,
            #[cfg(feature = "archive")]
            archive: None,
        }
    }
    /// One input allowance shared by every group client on this owner lane.
    pub fn with_client_limits(mut self, limits: groups::ClientLimits) -> Self {
        self.client = limits;
        self
    }
    /// Resident validation evidence per attached identity. This is a work/cache
    /// policy, not a bound on stored conversations or group membership.
    pub fn with_validation_cache_entries(mut self, entries: std::num::NonZeroUsize) -> Self {
        self.validation_cache_entries = entries;
        self
    }
    /// Select the owning group's catalog policy for fresh service initialization.
    /// Reopening an existing service keeps its committed limits.
    pub fn with_catalog_limits(mut self, limits: groups::enrollment::CatalogLimits) -> Self {
        self.catalog = limits;
        self
    }
    /// Select durable history for newly initialized group services. Reopening
    /// an existing service honors its persisted selection and budgets.
    #[cfg(feature = "archive")]
    pub fn with_archive(mut self, limits: groups::archive::Limits) -> Self {
        self.archive = Some(limits);
        self
    }
    pub(super) fn validate(self) -> Result<Self, groups::Error> {
        self.client.validate().map_err(groups::Error::Api)?;
        self.retention.validate()?;
        self.catalog.validate().map_err(groups::Error::Enrollment)?;
        #[cfg(feature = "archive")]
        if let Some(limits) = self.archive {
            limits.validate().map_err(groups::Error::Storage)?;
        }
        Ok(self)
    }
}
/// Selected group ownership over the existing local owner. Lifecycle and
/// identity methods remain on that owner. Submission state shares one input
/// allowance and the bounded observations published by its retained runners.
pub struct Owner<
    const I: usize,
    const O: usize,
    const S: usize,
    const L: usize,
    D = Local<I>,
    J = Command,
> where
    D: owned::CommandHandler<S, J>,
    <D as runtime::ScopedDriver<S>>::Error: Send + Sync + 'static,
{
    inner: crate::local::Owner<I, O, S, L, D, J>,
    submission: Submission<J>,
}
impl<const I: usize, const O: usize, const S: usize, const L: usize, D, J> Owner<I, O, S, L, D, J>
where
    D: owned::CommandHandler<S, J>,
    <D as runtime::ScopedDriver<S>>::Error: Send + Sync + 'static,
    J: Send + 'static,
{
    /// Custom assemblies must pass a clone of the exact SubmissionState supplied to Local.
    /// The built-in factories establish this relationship before construction.
    pub fn from_local(
        inner: crate::local::Owner<I, O, S, L, D, J>,
        state: SubmissionState,
    ) -> Self {
        let submission = Submission::new(inner.commands(), state);
        Self { inner, submission }
    }
    pub fn submission(&self) -> Submission<J> {
        self.submission.clone()
    }
}
impl<const I: usize, const O: usize, const S: usize, const L: usize, D, J> std::ops::Deref
    for Owner<I, O, S, L, D, J>
where
    D: owned::CommandHandler<S, J>,
    <D as runtime::ScopedDriver<S>>::Error: Send + Sync + 'static,
{
    type Target = crate::local::Owner<I, O, S, L, D, J>;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}
/// Create an empty identity authority and domain directory. Every service schema
/// is subsequently selected by an explicit retained AddService operation.
pub fn create_new<const I: usize, const O: usize, const S: usize, const L: usize>(
    path: impl AsRef<std::path::Path>,
    options: Options,
) -> Result<Owner<I, O, S, L>, crate::local::Error<Error>> {
    let options = options.validate().map_err(|error| {
        crate::local::Error::Identity(owned::Error::Driver(Error::Schema(SchemaError::Group(
            error,
        ))))
    })?;
    let input = groups::InputBudget::new(options.client).map_err(|error| {
        crate::local::Error::Identity(owned::Error::Driver(Error::Schema(SchemaError::Group(
            groups::Error::Api(error),
        ))))
    })?;
    let state = SubmissionState::new::<I>(input)
        .map_err(|error| crate::local::Error::Identity(owned::Error::Driver(error)))?;
    let worker_input = state.clone();
    crate::local::Owner::create_new(
        path,
        options.local,
        move |stores, catalog, wake, _commands| {
            configure(stores, catalog, wake, options, worker_input)
        },
    )
    .map(|inner| Owner::from_local(inner, state))
}
/// Strictly reopen every configured group schema before publishing readiness.
/// Enabled durable intent is preserved; recovery/activation remains explicit.
pub fn open_existing<const I: usize, const O: usize, const S: usize, const L: usize>(
    path: impl AsRef<std::path::Path>,
    options: Options,
) -> Result<Owner<I, O, S, L>, crate::local::Error<Error>> {
    let options = options.validate().map_err(|error| {
        crate::local::Error::Identity(owned::Error::Driver(Error::Schema(SchemaError::Group(
            error,
        ))))
    })?;
    let input = groups::InputBudget::new(options.client).map_err(|error| {
        crate::local::Error::Identity(owned::Error::Driver(Error::Schema(SchemaError::Group(
            groups::Error::Api(error),
        ))))
    })?;
    let state = SubmissionState::new::<I>(input)
        .map_err(|error| crate::local::Error::Identity(owned::Error::Driver(error)))?;
    let worker_input = state.clone();
    crate::local::Owner::open_existing(
        path,
        options.local,
        move |stores, catalog, wake, _commands| {
            configure(stores, catalog, wake, options, worker_input)
        },
    )
    .map(|inner| Owner::from_local(inner, state))
}
pub(super) fn configure<const I: usize, const O: usize, const S: usize>(
    stores: Stores<I>,
    catalog: &core::Catalog<I, O, S>,
    wake: Waker,
    options: Options,
    state: SubmissionState,
) -> Result<Local<I>, Error> {
    let mut driver = Local::new(stores, options.retention, wake, state)?
        .with_catalog_limits(options.catalog)?
        .with_validation_cache_entries(options.validation_cache_entries);
    #[cfg(feature = "archive")]
    if let Some(limits) = options.archive {
        driver = driver.with_archive(limits)?;
    }
    for identity in catalog.identities() {
        for service in identity.services.iter() {
            if service != SERVICE {
                return Err(Error::UnsupportedService(service));
            }
            driver.attach(identity.reference())?;
        }
    }
    Ok(driver)
}
