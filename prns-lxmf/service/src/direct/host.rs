//! Concrete host adapter; the injected runtime engine does not select this module.
use super::*;
use personal_rns::identity::Zeroizing;
use personal_rns::runtime::DestinationIdentityRetentionControl;
use prns_host::{CommandFailure, CommandOutcome, HostCommand};
use prns_host_runtime::owner::{HostClient, SessionError};

/// Direct network adapter over one borrowed canonical host client.
/// The service neither owns nor stops the host session.
#[derive(Clone)]
pub struct PrnsDirectNetwork {
    host: HostClient,
    scope: Option<prns_host_runtime::service_scope::HostServiceScope>,
}

// Ownership starts before admission: an overloaded or fenced operation lane must
// also close an already-acquired Link. The host retains this guard through the
// actual send even if its caller abandons the waiter.
struct LinkCleanup {
    host: HostClient,
    link: Option<prns_host::LinkId>,
}
impl Drop for LinkCleanup {
    fn drop(&mut self) {
        let Some(link) = self.link.take() else {
            return;
        };
        if let Ok(services) = self.host.service_client() {
            let _queued = services
                .protocols()
                .close_link(personal_rns::routing::links::LinkId::new(link.into_bytes()));
        }
    }
}

impl PrnsDirectNetwork {
    /// Borrow a host without an identity-generation fence. Each sent Link is
    /// transferred to the adapter and closed after settlement; unrelated Links
    /// and the host remain owned by the caller. Prefer [`Self::scoped`] when an
    /// embedding must fence one identity independently of the host.
    #[must_use]
    pub fn new(host: HostClient) -> Self {
        Self { host, scope: None }
    }

    /// Bind initiator Links and network admission to one local identity generation.
    /// The embedding forwards lifecycle events, fences this scope, retires the
    /// destination ingress, and drains the durable service before acknowledging
    /// pause. Keep accepted receive callbacks alive until ingress retires.
    pub fn scoped(scope: prns_host_runtime::service_scope::HostServiceScope) -> Self {
        Self {
            host: scope.host().clone(),
            scope: Some(scope),
        }
    }

    async fn execute(
        &self,
        command: HostCommand,
    ) -> Result<Result<CommandOutcome, CommandFailure>, SessionError> {
        match &self.scope {
            Some(scope) => {
                use prns_host_runtime::service_scope::HostServiceScopeError;
                let result = if let HostCommand::SendLinkPacket { link_id, .. } = &command {
                    let cleanup = scope.owns_link(*link_id).then(|| LinkCleanup {
                        host: scope.host().clone(),
                        link: Some(*link_id),
                    });
                    let scoped = scope.clone();
                    // The host-owned operation keeps this cleanup alive if the
                    // send waiter is cancelled. Closure submission alone never
                    // releases the scope's tracked Link slot.
                    scope
                        .protocol_operation(move |_| async move {
                            let _cleanup = cleanup;
                            scoped.execute(command).await
                        })
                        .await
                        .and_then(|result| result)
                } else {
                    scope.execute(command).await
                };
                result.map_err(|error| match error {
                    HostServiceScopeError::Session(error) => error,
                    HostServiceScopeError::Capacity => SessionError::Busy,
                    _ => SessionError::Stopped,
                })
            }
            None => match command {
                HostCommand::EstablishLink { .. } | HostCommand::SendLinkPacket { .. } => {
                    self.execute_link(command).await
                }
                _ => self.host.execute(command).await,
            },
        }
    }

    async fn execute_link(
        &self,
        command: HostCommand,
    ) -> Result<Result<CommandOutcome, CommandFailure>, SessionError> {
        let cleanup = match &command {
            HostCommand::SendLinkPacket { link_id, .. } => Some(LinkCleanup {
                host: self.host.clone(),
                link: Some(*link_id),
            }),
            _ => None,
        };
        let host = self.host.clone();
        let (result, mut established) = self
            .host
            .protocol_operation(move |_| async move {
                let _cleanup = cleanup;
                let result = host.execute(command).await?;
                let established = match &result {
                    Ok(CommandOutcome::LinkEstablished { link_id, .. }) => Some(LinkCleanup {
                        host,
                        link: Some(*link_id),
                    }),
                    _ => None,
                };
                Ok::<_, SessionError>((result, established))
            })
            .await??;
        // The host retains this guard until the establishment waiter receives
        // success. Caller cancellation must close a late, otherwise lost Link.
        if let Some(established) = &mut established {
            established.link = None;
        }
        Ok(result)
    }
}

impl DirectNetwork for PrnsDirectNetwork {
    fn has_route(&self, destination: [u8; 16]) -> DirectNetworkFuture<'_, bool> {
        Box::pin(async move {
            self.host.snapshot().await.is_ok_and(|snapshot| {
                snapshot
                    .routes
                    .iter()
                    .any(|route| route.destination.as_bytes() == &destination)
            })
        })
    }

    fn request_path(
        &self,
        destination: [u8; 16],
    ) -> DirectNetworkFuture<'_, Result<(), DirectSendFailure>> {
        Box::pin(async move {
            match self
                .execute(HostCommand::RequestPath {
                    destination: prns_host::DestinationHash::new(destination),
                })
                .await
            {
                Ok(Ok(CommandOutcome::PathDiscovered { .. })) => Ok(()),
                Err(SessionError::Stopped) | Ok(Err(CommandFailure::NodeStopped)) => {
                    Err(DirectSendFailure::LocalNodeStopped)
                }
                _ => Err(DirectSendFailure::NoRoute),
            }
        })
    }

    fn establish_link(
        &self,
        destination: [u8; 16],
    ) -> DirectNetworkFuture<'_, Result<[u8; 16], DirectSendFailure>> {
        Box::pin(async move {
            match self
                .execute(HostCommand::EstablishLink {
                    destination: prns_host::DestinationHash::new(destination),
                })
                .await
            {
                Ok(Ok(CommandOutcome::LinkEstablished { link_id, .. })) => Ok(link_id.into_bytes()),
                Ok(Err(failure)) => Err(classify_establish_link_failure(failure)),
                Err(SessionError::Stopped) => Err(DirectSendFailure::LocalNodeStopped),
                _ => Err(DirectSendFailure::LinkFailed),
            }
        })
    }

    fn send_link_packet(
        &self,
        link: [u8; 16],
        complete_wire: Vec<u8>,
    ) -> DirectNetworkFuture<'_, Result<DirectDeliveryReceipt, DirectSendFailure>> {
        Box::pin(async move {
            match self
                .execute(HostCommand::SendLinkPacket {
                    link_id: prns_host::LinkId::new(link),
                    payload: complete_wire,
                })
                .await
            {
                Ok(Ok(CommandOutcome::PacketDelivered {
                    rtt_millis,
                    evidence:
                        prns_host::DeliveryEvidence::ExplicitProof(_)
                        | prns_host::DeliveryEvidence::ImplicitProof(_),
                })) => Ok(DirectDeliveryReceipt { rtt_millis }),
                Ok(Err(CommandFailure::DeliveryTimedOut)) => {
                    Err(DirectSendFailure::DeliveryTimedOut)
                }
                Err(SessionError::Stopped) | Ok(Err(CommandFailure::NodeStopped)) => {
                    Err(DirectSendFailure::LocalNodeStopped)
                }
                _ => Err(DirectSendFailure::LinkFailed),
            }
        })
    }

    fn destination_public_key(
        &self,
        destination: [u8; 16],
    ) -> DirectNetworkFuture<'_, Option<[u8; 64]>> {
        Box::pin(async move {
            self.host
                .destination_public_key(prns_host::DestinationHash::new(destination))
                .await
                .ok()
                .flatten()
        })
    }

    fn mark_destination_used(&self, destination: [u8; 16]) -> DirectNetworkFuture<'_, ()> {
        Box::pin(async move {
            let Ok(services) = self.host.service_client() else {
                return;
            };
            let _retention = services
                .protocols()
                .mark_destination_used(personal_rns::wire::DestinationHash::new(destination))
                .await;
        })
    }

    fn announce(
        &self,
        destination: [u8; 16],
    ) -> DirectNetworkFuture<'_, Result<(), DirectAnnounceFailure>> {
        Box::pin(async move {
            match self
                .execute(HostCommand::Announce {
                    destination: prns_host::DestinationHash::new(destination),
                    interface: None,
                })
                .await
            {
                Ok(Ok(CommandOutcome::Announced)) => Ok(()),
                _ => Err(DirectAnnounceFailure::NodeRejected),
            }
        })
    }
}

pub(super) fn classify_establish_link_failure(failure: CommandFailure) -> DirectSendFailure {
    match failure {
        CommandFailure::NoRouteToDestination => DirectSendFailure::NoRoute,
        CommandFailure::NodeStopped => DirectSendFailure::LocalNodeStopped,
        _ => DirectSendFailure::LinkFailed,
    }
}

/// Failure to construct the fixed LXMF destination in the canonical host contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LxmfDestinationError {
    IdentityName(ExpandNameError),
    HostName(prns_host::DestinationNameError),
}

/// Retain a zeroizing signer and move a separate zeroizing copy into the exact
/// direct-only canonical host destination configuration.
pub fn prepare_local_lxmf_destination(
    identity: Zeroizing<[u8; IDENTITY_SECRET_KEY_LEN]>,
    announce_app_data: &[u8],
) -> Result<(LocalLxmfIdentity, prns_host::DestinationConfig), LxmfDestinationError> {
    prepare_lxmf_destination(
        &identity,
        prns_host::DestinationIdentityConfig::Dedicated(prns_host::IdentityConfig::Existing(
            prns_host::IdentitySecret::new(*identity),
        )),
        announce_app_data,
    )
}

/// Retain the LXMF signer while using the host's identity for its destination.
/// The caller must configure the host with this same identity. Neither the
/// borrowed secret nor a separate destination identity is retained in the config.
pub fn prepare_host_lxmf_destination(
    host_identity: &[u8; IDENTITY_SECRET_KEY_LEN],
    announce_app_data: &[u8],
) -> Result<(LocalLxmfIdentity, prns_host::DestinationConfig), LxmfDestinationError> {
    prepare_lxmf_destination(
        host_identity,
        prns_host::DestinationIdentityConfig::HostIdentity,
        announce_app_data,
    )
}

fn prepare_lxmf_destination(
    secret: &[u8; IDENTITY_SECRET_KEY_LEN],
    identity: prns_host::DestinationIdentityConfig,
    announce_app_data: &[u8],
) -> Result<(LocalLxmfIdentity, prns_host::DestinationConfig), LxmfDestinationError> {
    let retained =
        LocalLxmfIdentity::from_secret_bytes(secret).map_err(LxmfDestinationError::IdentityName)?;
    let name = prns_host::DestinationName::try_new(
        prns_lxmf_wire::LXMF_APP_NAME,
        prns_lxmf_wire::LXMF_DELIVERY_ASPECTS
            .iter()
            .map(|aspect| (*aspect).to_owned()),
    )
    .map_err(LxmfDestinationError::HostName)?;
    let mut destination = prns_host::SingleDestinationConfig::new(name, identity);
    destination.announce_app_data = announce_app_data.to_vec();
    destination.maximum_request_bytes = Some(0);
    Ok((retained, prns_host::DestinationConfig::Single(destination)))
}
