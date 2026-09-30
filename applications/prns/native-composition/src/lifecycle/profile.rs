use super::*;

pub async fn messaging_profile(name: Option<String>) -> LocalMessagingProfileOutcome {
    messaging_profile_with_supervisor(supervisor(), name).await
}

pub(super) async fn messaging_profile_with_supervisor(
    supervisor: &Supervisor,
    name: Option<String>,
) -> LocalMessagingProfileOutcome {
    let name = match name
        .map(|name| crate::messaging_profile::normalize_name(&name))
        .transpose()
    {
        Ok(name) => name,
        Err(detail) => return LocalMessagingProfileOutcome::InvalidInput { detail },
    };
    let receive = {
        let state = match admission::try_state(supervisor) {
            Ok(state) => state,
            Err(_) => return LocalMessagingProfileOutcome::Busy,
        };
        let snapshot = supervisor.snapshots.read();
        match &snapshot.primary_identity {
            PrimaryIdentityState::Unavailable { detail } => {
                return LocalMessagingProfileOutcome::Unavailable {
                    detail: detail.clone(),
                };
            }
            PrimaryIdentityState::DevelopmentResetRequired { reason } => {
                return LocalMessagingProfileOutcome::DevelopmentResetRequired {
                    reason: reason.clone(),
                };
            }
            PrimaryIdentityState::Missing | PrimaryIdentityState::Present { .. } => {}
        }
        if matches!(
            snapshot.runtime,
            DevelopmentNodeRuntime::Starting | DevelopmentNodeRuntime::Stopping
        ) || supervisor.snapshots.is_explicit_stop_in_progress()
        {
            return LocalMessagingProfileOutcome::Busy;
        }
        let Some(owner) = state.application_owner.as_ref() else {
            return LocalMessagingProfileOutcome::Unavailable {
                detail: "Prepare native storage before reading messaging settings.".into(),
            };
        };
        let commands = if name.is_some() && snapshot.runtime == DevelopmentNodeRuntime::Running {
            state.worker.as_ref().map(|worker| worker.commands.clone())
        } else {
            None
        };
        let (response, receive) = oneshot::channel();
        if let Err(failure) = owner.admit_messaging_name(name, move |result| {
            match result {
                Ok(name) => {
                    let profile =
                        crate::messaging_profile::profile(name, &snapshot.primary_identity);
                    if let Some(commands) = commands {
                        // The serial database owner preserves commit/application order.
                        // A dropped/closed actor command reports the saved-but-not-applied state.
                        let _ =
                            commands.try_send(Command::SetMessagingProfile(ProfileApplyReply {
                                profile,
                                response: Some(response),
                            }));
                    } else {
                        let _ = response.send(LocalMessagingProfileOutcome::Ready { profile });
                    }
                }
                Err(failure) => {
                    let _ = response.send(profile_failure(failure));
                }
            }
        }) {
            return profile_failure(failure);
        }
        receive
    };
    receive
        .await
        .unwrap_or_else(|_| LocalMessagingProfileOutcome::Unavailable {
            detail: "The database owner stopped before confirming messaging settings.".into(),
        })
}

fn profile_failure(failure: DevelopmentStoreFailure) -> LocalMessagingProfileOutcome {
    match failure {
        DevelopmentStoreFailure::Unavailable(detail) => {
            LocalMessagingProfileOutcome::Unavailable { detail }
        }
        DevelopmentStoreFailure::ResetRequired(reason) => {
            LocalMessagingProfileOutcome::DevelopmentResetRequired { reason }
        }
    }
}

pub(super) struct ProfileApplyReply {
    profile: LocalMessagingProfile,
    response: Option<Reply<LocalMessagingProfileOutcome>>,
}

impl Drop for ProfileApplyReply {
    fn drop(&mut self) {
        if let Some(response) = self.response.take() {
            let _ = response.send(LocalMessagingProfileOutcome::SavedButNotApplied {
                profile: self.profile.clone(),
                detail: "The name was saved, but the node stopped or its command lane was full before the live name update. Restart the node to apply it.".into(),
            });
        }
    }
}

pub(super) async fn apply_profile(
    handle: &PrnsNodeHandle,
    local_destination: [u8; 16],
    mut reply: ProfileApplyReply,
) {
    let update = async {
        let encoded = crate::messaging_profile::announce_data(&reply.profile.display_name)?;
        let app_data =
            personal_rns::routing::announce::emit::AnnounceAppDataBytes::from_slice(&encoded)
                .map_err(|_| "The encoded messaging name exceeds announce capacity.".to_owned())?;
        handle
            .set_registered_announce_app_data(personal_rns::engine::SetRegisteredAnnounceAppData {
                destination: DestinationHash::new(local_destination),
                app_data,
            })
            .await
            .map_err(|error| {
                format!(
                    "The saved messaging name could not be applied to the running node: {error:?}"
                )
            })
    };
    let result = tokio::time::timeout(LXMF_QUERY_TIMEOUT, update).await;
    let outcome = match result {
        Ok(Ok(())) => LocalMessagingProfileOutcome::Ready { profile: reply.profile.clone() },
        Ok(Err(detail)) => LocalMessagingProfileOutcome::SavedButNotApplied { profile: reply.profile.clone(), detail },
        Err(_) => LocalMessagingProfileOutcome::SavedButNotApplied {
            profile: reply.profile.clone(),
            detail: "The name was saved, but its live update did not finish within the bounded wait. Restart the node to apply it.".into(),
        },
    };
    if let Some(response) = reply.response.take() {
        let _ = response.send(outcome);
    }
}
