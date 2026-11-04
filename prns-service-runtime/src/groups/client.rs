use super::*;

/// Shared native state for one physical group owner. Construct once and clone
/// into Local and Owner; the identity bound is the enclosing owner's capacity.
#[derive(Clone)]
pub struct SubmissionState {
    pub(super) input: groups::InputBudget,
    #[cfg(feature = "groups-network")]
    observations: Arc<super::observations::Registry>,
}
impl SubmissionState {
    pub fn new<const I: usize>(input: groups::InputBudget) -> Result<Self, Error> {
        Ok(Self {
            input,
            #[cfg(feature = "groups-network")]
            observations: Arc::new(super::observations::Registry::new(I)?),
        })
    }
}

/// The existing owner lane and its single shared group-input allowance. Keep
/// one instance per composed lane and clone it for every ordinary/network
/// client. Cloning never creates another queue or resets input accounting.
pub struct Submission<J> {
    commands: owned::Commands<J>,
    state: SubmissionState,
}
impl<J> Clone for Submission<J> {
    fn clone(&self) -> Self {
        Self {
            commands: self.commands.clone(),
            state: self.state.clone(),
        }
    }
}
impl<J: Send + 'static> Submission<J> {
    /// Custom assemblies pass a clone of the exact SubmissionState supplied to
    /// Local; wrapping the same InputBudget in another state splits observations.
    pub fn new(commands: owned::Commands<J>, state: SubmissionState) -> Self {
        Self { commands, state }
    }
    #[cfg(feature = "groups-network")]
    pub(super) fn observe_runner(
        &self,
        identity: core::IdentityReference,
        owner: runtime::OwnerToken,
        client: &groups::Client,
    ) -> Result<super::observations::Registration, groups::Error> {
        self.state
            .observations
            .register(identity, owner, client.synchronization_source())
    }
}

/// Existing opaque group job plus its immutable catalog-local binding. The
/// group manager, not this wrapper or its caller, derives the access class.
pub struct Command {
    pub identity: core::IdentityReference,
    pub job: groups::Job,
    // Captured at canonical activation. Ordinary clients cannot manufacture
    // this native capability or change its owner/service/generation.
    pub(super) network: Option<runtime::NetworkPermit>,
    // Only the private retained-runner constructor can capture this exact
    // admission origin. Public callers keep the normal closed-owner result.
    runner_dispatch: Option<owned::DispatchLease>,
}
impl Command {
    pub(super) fn reject(self) {
        let stopped_runner = self.runner_dispatch.as_ref().is_some_and(|lease| {
            self.network.is_some_and(|permit| {
                permit.owner() == lease.owner()
                    && permit.service() == SERVICE
                    && reference(permit.admission().identity) == self.identity
            }) && !lease.is_open()
        });
        self.job.fail(if stopped_runner {
            // This job was queued but never executed. Its runner is paired with
            // the retained Stop receiver; canonical shutdown ends the retry.
            groups::Error::Busy
        } else {
            groups::Error::Unavailable
        });
    }
}
struct Submit<J> {
    submission: Submission<J>,
    identity: core::IdentityReference,
    route: fn(Command) -> J,
    network: Option<runtime::NetworkPermit>,
    runner_dispatch: Option<owned::DispatchLease>,
}
impl<J: Send + 'static> groups::Submit for Submit<J> {
    #[cfg(feature = "groups-network")]
    fn synchronization_progress(
        &self,
        group: prns_groups_store::GroupId,
    ) -> Option<groups::GroupSyncProgress> {
        self.network.is_none().then(|| {
            self.submission
                .state
                .observations
                .snapshot(self.identity, group)
        })
    }
    fn input_budget(&self) -> &groups::InputBudget {
        &self.submission.state.input
    }
    fn submit(&self, job: groups::Job) -> Result<(), groups::Error> {
        self.submission
            .commands
            .submit((self.route)(Command {
                identity: self.identity,
                job,
                network: self.network,
                runner_dispatch: self.runner_dispatch.clone(),
            }))
            .map_err(|error| match error {
                owned::SubmitFailure::Full => groups::Error::Busy,
                owned::SubmitFailure::Bounds => groups::Error::InvalidInput,
                owned::SubmitFailure::Closed
                    if self
                        .runner_dispatch
                        .as_ref()
                        .is_some_and(|lease| !lease.is_open()) =>
                {
                    // Only this private runner adapter has a retained Stop
                    // receiver. Its retry wait ends through canonical shutdown;
                    // no admitted failure reply is reclassified here.
                    groups::Error::Busy
                }
                owned::SubmitFailure::Closed | owned::SubmitFailure::Reentrant => {
                    groups::Error::Unavailable
                }
            })
    }
}
/// Construct the ordinary canonical group client. `route` only embeds this one
/// domain job in the selected assembly's typed sum; it is not a command facade.
pub fn client<J: Send + 'static>(
    submission: Submission<J>,
    identity: core::IdentityReference,
    route: fn(Command) -> J,
) -> groups::Client {
    groups::Client::new(Arc::new(Submit {
        submission,
        identity,
        route,
        network: None,
        runner_dispatch: None,
    }))
}

/// The same ordinary domain client, bound to a native runner's exact activation.
/// Obtain `permit` from the identity driver's post-confirmation notification.
/// Capturing it does not grant dispatch: execution validates it again on the
/// identity owner lane, and the host's restrictive fence still applies.
pub fn network_client<J: Send + 'static>(
    submission: Submission<J>,
    identity: core::IdentityReference,
    permit: runtime::NetworkPermit,
    route: fn(Command) -> J,
) -> Result<groups::Client, groups::Error> {
    network_client_with_dispatch(submission, identity, permit, route, None)
}

/// Internal retained runner only: a synchronous closed admission waits for the
/// driver's paired Stop receiver instead of becoming a spurious failure. Public
/// clients keep their immediate Unavailable result. The immutable owner must
/// match the permit; a later identity generation cannot cancel this runner.
#[cfg(feature = "groups-network")]
pub(super) fn runner_client<J: Send + 'static>(
    submission: Submission<J>,
    identity: core::IdentityReference,
    permit: runtime::NetworkPermit,
    route: fn(Command) -> J,
    dispatch: owned::DispatchLease,
) -> Result<groups::Client, groups::Error> {
    if dispatch.owner() != permit.owner() {
        return Err(groups::Error::Configuration);
    }
    network_client_with_dispatch(submission, identity, permit, route, Some(dispatch))
}

fn network_client_with_dispatch<J: Send + 'static>(
    submission: Submission<J>,
    identity: core::IdentityReference,
    permit: runtime::NetworkPermit,
    route: fn(Command) -> J,
    runner_dispatch: Option<owned::DispatchLease>,
) -> Result<groups::Client, groups::Error> {
    if permit.service() != SERVICE || reference(permit.admission().identity) != identity {
        return Err(groups::Error::Configuration);
    }
    Ok(groups::Client::new(Arc::new(Submit {
        submission,
        identity,
        route,
        network: Some(permit),
        runner_dispatch,
    })))
}

#[cfg(test)]
mod tests;
