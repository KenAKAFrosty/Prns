use crate::engine::{
    BeginRemoteControlControllerPairing, EstablishLinkFailure,
    RemoteControlControllerPairingResponseEffect, RemoteControlControllerPairingResponseReceived,
};
use crate::remote_control::{
    RemoteControlPairingAvailabilityObservation, RemoteControlPairingContext,
    RemoteControlPairingEndpoint, RemoteControlPairingInvitationCode,
};
use crate::routing::links::LinkId;
use crate::units::InstantMillis;
use crate::wire::DestinationHash;

use super::{
    BeginRemoteControlControllerPairingControlFailure, RemoteControlPairingControl,
    RemoteControlPairingControlError, RemoteControlPairingLinkCleanupOutcome, SendError,
};

#[derive(Debug, PartialEq, Eq)]
pub struct InitiateRemoteControlControllerPairing {
    pub endpoint: RemoteControlPairingEndpoint,
    pub invitation_code: RemoteControlPairingInvitationCode,
    pub expires_at: InstantMillis,
}

impl InitiateRemoteControlControllerPairing {
    #[must_use]
    pub fn from_observation(
        observation: &RemoteControlPairingAvailabilityObservation<'_>,
        invitation_code: RemoteControlPairingInvitationCode,
    ) -> Self {
        Self {
            endpoint: observation.endpoint(),
            invitation_code,
            expires_at: observation.expires_at(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InitiateRemoteControlControllerPairingError {
    EstablishLink(SendError<EstablishLinkFailure>),
    NodeStopped {
        cleanup: RemoteControlPairingLinkCleanupOutcome,
    },
    Busy {
        cleanup: RemoteControlPairingLinkCleanupOutcome,
    },
    Begin {
        failure: crate::engine::BeginRemoteControlControllerPairingFailure,
        cleanup: RemoteControlPairingLinkCleanupOutcome,
    },
    Identify {
        failure: SendError<crate::engine::IdentifyFailure>,
        cleanup: RemoteControlPairingLinkCleanupOutcome,
    },
    Request {
        failure: crate::engine::RemoteControlControllerPairingRequestFailure,
        cleanup: RemoteControlPairingLinkCleanupOutcome,
    },
    ResponseNotAdvanced {
        response: RemoteControlControllerPairingResponseReceived,
        cleanup: RemoteControlPairingLinkCleanupOutcome,
    },
    ResponseExpired {
        response: RemoteControlControllerPairingResponseReceived,
    },
}

pub trait RemoteControlControllerPairingInitiationControl:
    RemoteControlControllerPairingInitiationTransport
{
    fn initiate_remote_control_controller_pairing(
        &self,
        initiate: InitiateRemoteControlControllerPairing,
    ) -> impl core::future::Future<
        Output = Result<
            RemoteControlControllerPairingResponseReceived,
            InitiateRemoteControlControllerPairingError,
        >,
    > + Send {
        async move {
            let link_id = self
                .establish_remote_control_pairing_link(initiate.endpoint.destination_hash())
                .await
                .map_err(InitiateRemoteControlControllerPairingError::EstablishLink)?;
            let result = self
                .begin_remote_control_controller_pairing(BeginRemoteControlControllerPairing {
                    context: RemoteControlPairingContext::new(initiate.endpoint, link_id),
                    invitation_code: initiate.invitation_code,
                    pairing_expires_at: initiate.expires_at,
                })
                .await;
            match result {
                Ok(received) => match received.effect {
                    RemoteControlControllerPairingResponseEffect::Advanced => Ok(received),
                    RemoteControlControllerPairingResponseEffect::Expired { .. } => Err(
                        InitiateRemoteControlControllerPairingError::ResponseExpired {
                            response: received,
                        },
                    ),
                    RemoteControlControllerPairingResponseEffect::NotAdvanced(_) => Err(
                        InitiateRemoteControlControllerPairingError::ResponseNotAdvanced {
                            response: received,
                            cleanup: self.close_remote_control_pairing_link(link_id),
                        },
                    ),
                },
                Err(RemoteControlPairingControlError::NodeStopped) => {
                    Err(InitiateRemoteControlControllerPairingError::NodeStopped {
                        cleanup: self.close_remote_control_pairing_link(link_id),
                    })
                }
                Err(RemoteControlPairingControlError::Busy) => {
                    Err(InitiateRemoteControlControllerPairingError::Busy {
                        cleanup: self.close_remote_control_pairing_link(link_id),
                    })
                }
                Err(RemoteControlPairingControlError::Failed(
                    BeginRemoteControlControllerPairingControlFailure::Begin(failure),
                )) => Err(InitiateRemoteControlControllerPairingError::Begin {
                    failure,
                    cleanup: self.close_remote_control_pairing_link(link_id),
                }),
                Err(RemoteControlPairingControlError::Failed(
                    BeginRemoteControlControllerPairingControlFailure::Identify {
                        failure,
                        cleanup,
                    },
                )) => {
                    Err(InitiateRemoteControlControllerPairingError::Identify { failure, cleanup })
                }
                Err(RemoteControlPairingControlError::Failed(
                    BeginRemoteControlControllerPairingControlFailure::Request(failure),
                )) => Err(InitiateRemoteControlControllerPairingError::Request {
                    failure,
                    cleanup: self.close_remote_control_pairing_link(link_id),
                }),
            }
        }
    }
}

impl<T> RemoteControlControllerPairingInitiationControl for T where
    T: RemoteControlControllerPairingInitiationTransport
{
}

pub trait RemoteControlControllerPairingInitiationTransport:
    RemoteControlPairingControl + Sync
{
    fn establish_remote_control_pairing_link(
        &self,
        destination: DestinationHash,
    ) -> impl core::future::Future<Output = Result<LinkId, SendError<EstablishLinkFailure>>> + Send;

    fn close_remote_control_pairing_link(
        &self,
        link_id: LinkId,
    ) -> RemoteControlPairingLinkCleanupOutcome;
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use futures_executor::block_on;

    use super::*;
    use crate::engine::{
        AdmitRemoteControlControllerPairingResponseOutcome, ApproveRemoteControlControllerPairing,
        ApproveRemoteControlTargetPairing, BeginRemoteControlControllerPairingFailure,
        DeliveryEvidence, IdentifyFailure, PacketReceiptDelivered,
        RejectRemoteControlControllerPairing, RejectRemoteControlTargetPairing,
        RemoteControlControllerPairingRequestFailure,
        RemoteControlControllerPairingRequestFailureCause, RemoteControlTargetPairingApproval,
        RemoteControlTargetPairingRejection,
    };
    use crate::identity::IdentityHash;
    use crate::remote_control::{
        FailRemoteControlControllerPairingRequestOutcome, RemoteControlPairingIdentity,
    };
    use crate::runtime::{
        ApproveRemoteControlControllerPairingControlError,
        ApproveRemoteControlTargetPairingControlError,
        BeginRemoteControlControllerPairingControlError,
        RejectRemoteControlControllerPairingControlError,
        RejectRemoteControlTargetPairingControlError,
    };
    use crate::units::RttMillis;

    const LINK_ID: LinkId = LinkId::new([0x41; 16]);
    const PAIRING_EXPIRES_AT: InstantMillis = InstantMillis(10_000);

    #[derive(Debug, PartialEq, Eq)]
    enum Invocation {
        EstablishLink(DestinationHash),
        Begin(BeginRemoteControlControllerPairing),
        CloseLink(LinkId),
    }

    struct Harness {
        invocations: Mutex<std::vec::Vec<Invocation>>,
        establish: Result<LinkId, SendError<EstablishLinkFailure>>,
        begin: Result<
            RemoteControlControllerPairingResponseReceived,
            BeginRemoteControlControllerPairingControlError,
        >,
        cleanup: RemoteControlPairingLinkCleanupOutcome,
    }

    impl Harness {
        fn new(
            establish: Result<LinkId, SendError<EstablishLinkFailure>>,
            begin: Result<
                RemoteControlControllerPairingResponseReceived,
                BeginRemoteControlControllerPairingControlError,
            >,
            cleanup: RemoteControlPairingLinkCleanupOutcome,
        ) -> Self {
            Self {
                invocations: Mutex::new(std::vec::Vec::new()),
                establish,
                begin,
                cleanup,
            }
        }

        fn invocations(&self) -> std::sync::MutexGuard<'_, std::vec::Vec<Invocation>> {
            self.invocations.lock().unwrap()
        }
    }

    impl RemoteControlPairingControl for Harness {
        async fn begin_remote_control_controller_pairing(
            &self,
            begin: BeginRemoteControlControllerPairing,
        ) -> Result<
            RemoteControlControllerPairingResponseReceived,
            BeginRemoteControlControllerPairingControlError,
        > {
            self.invocations
                .lock()
                .unwrap()
                .push(Invocation::Begin(begin));
            self.begin
        }

        async fn approve_remote_control_controller_pairing(
            &self,
            _approve: ApproveRemoteControlControllerPairing,
        ) -> Result<
            RemoteControlControllerPairingResponseReceived,
            ApproveRemoteControlControllerPairingControlError,
        > {
            Err(RemoteControlPairingControlError::NodeStopped)
        }

        async fn reject_remote_control_controller_pairing(
            &self,
            _reject: RejectRemoteControlControllerPairing,
        ) -> Result<
            crate::engine::RemoteControlControllerPairingRejection,
            RejectRemoteControlControllerPairingControlError,
        > {
            Err(RemoteControlPairingControlError::NodeStopped)
        }

        async fn approve_remote_control_target_pairing(
            &self,
            _approve: ApproveRemoteControlTargetPairing,
        ) -> Result<RemoteControlTargetPairingApproval, ApproveRemoteControlTargetPairingControlError>
        {
            Err(RemoteControlPairingControlError::NodeStopped)
        }

        async fn reject_remote_control_target_pairing(
            &self,
            _reject: RejectRemoteControlTargetPairing,
        ) -> Result<RemoteControlTargetPairingRejection, RejectRemoteControlTargetPairingControlError>
        {
            Err(RemoteControlPairingControlError::NodeStopped)
        }
    }

    impl RemoteControlControllerPairingInitiationTransport for Harness {
        async fn establish_remote_control_pairing_link(
            &self,
            destination: DestinationHash,
        ) -> Result<LinkId, SendError<EstablishLinkFailure>> {
            self.invocations
                .lock()
                .unwrap()
                .push(Invocation::EstablishLink(destination));
            self.establish
        }

        fn close_remote_control_pairing_link(
            &self,
            link_id: LinkId,
        ) -> RemoteControlPairingLinkCleanupOutcome {
            self.invocations
                .lock()
                .unwrap()
                .push(Invocation::CloseLink(link_id));
            self.cleanup
        }
    }

    fn endpoint() -> RemoteControlPairingEndpoint {
        RemoteControlPairingIdentity::new(IdentityHash::new([0x51; 16])).endpoint()
    }

    fn initiate() -> InitiateRemoteControlControllerPairing {
        InitiateRemoteControlControllerPairing {
            endpoint: endpoint(),
            invitation_code: RemoteControlPairingInvitationCode::from_value(0x1234_ABCD),
            expires_at: PAIRING_EXPIRES_AT,
        }
    }

    fn expected_begin() -> BeginRemoteControlControllerPairing {
        BeginRemoteControlControllerPairing {
            context: RemoteControlPairingContext::new(endpoint(), LINK_ID),
            invitation_code: RemoteControlPairingInvitationCode::from_value(0x1234_ABCD),
            pairing_expires_at: PAIRING_EXPIRES_AT,
        }
    }

    fn received(
        effect: RemoteControlControllerPairingResponseEffect,
    ) -> RemoteControlControllerPairingResponseReceived {
        RemoteControlControllerPairingResponseReceived {
            delivered: PacketReceiptDelivered {
                rtt: RttMillis::new(7),
                evidence: DeliveryEvidence::Response,
            },
            admission: AdmitRemoteControlControllerPairingResponseOutcome::NoActivePairing,
            effect,
        }
    }

    fn advanced_received() -> RemoteControlControllerPairingResponseReceived {
        received(RemoteControlControllerPairingResponseEffect::Advanced)
    }

    fn not_advanced_received() -> RemoteControlControllerPairingResponseReceived {
        received(RemoteControlControllerPairingResponseEffect::NotAdvanced(
            FailRemoteControlControllerPairingRequestOutcome::NoActiveAttempt,
        ))
    }

    fn expired_received() -> RemoteControlControllerPairingResponseReceived {
        received(RemoteControlControllerPairingResponseEffect::Expired {
            retired_link: LINK_ID,
        })
    }

    #[test]
    fn successful_initiation_establishes_then_begins_without_closing() {
        let harness = Harness::new(
            Ok(LINK_ID),
            Ok(advanced_received()),
            RemoteControlPairingLinkCleanupOutcome::NotQueued,
        );

        assert_eq!(
            block_on(harness.initiate_remote_control_controller_pairing(initiate())),
            Ok(advanced_received()),
        );
        assert_eq!(
            harness.invocations().as_slice(),
            [
                Invocation::EstablishLink(endpoint().destination_hash()),
                Invocation::Begin(expected_begin()),
            ],
        );
    }

    #[test]
    fn settled_response_that_does_not_advance_closes_the_owned_link() {
        let response = not_advanced_received();
        let harness = Harness::new(
            Ok(LINK_ID),
            Ok(response),
            RemoteControlPairingLinkCleanupOutcome::Queued,
        );

        assert_eq!(
            block_on(harness.initiate_remote_control_controller_pairing(initiate())),
            Err(
                InitiateRemoteControlControllerPairingError::ResponseNotAdvanced {
                    response,
                    cleanup: RemoteControlPairingLinkCleanupOutcome::Queued,
                },
            ),
        );
        assert_eq!(
            harness.invocations().as_slice(),
            [
                Invocation::EstablishLink(endpoint().destination_hash()),
                Invocation::Begin(expected_begin()),
                Invocation::CloseLink(LINK_ID),
            ],
        );
    }

    #[test]
    fn expired_response_reports_the_engine_retirement_without_closing_again() {
        let response = expired_received();
        let harness = Harness::new(
            Ok(LINK_ID),
            Ok(response),
            RemoteControlPairingLinkCleanupOutcome::NotQueued,
        );

        assert_eq!(
            block_on(harness.initiate_remote_control_controller_pairing(initiate())),
            Err(InitiateRemoteControlControllerPairingError::ResponseExpired { response }),
        );
        assert_eq!(
            harness.invocations().as_slice(),
            [
                Invocation::EstablishLink(endpoint().destination_hash()),
                Invocation::Begin(expected_begin()),
            ],
        );
    }

    #[test]
    fn link_establishment_failure_is_exact_and_stops_initiation() {
        let error = SendError::Failed(EstablishLinkFailure::Timeout);
        let harness = Harness::new(
            Err(error),
            Ok(advanced_received()),
            RemoteControlPairingLinkCleanupOutcome::NotQueued,
        );

        assert_eq!(
            block_on(harness.initiate_remote_control_controller_pairing(initiate())),
            Err(InitiateRemoteControlControllerPairingError::EstablishLink(
                error
            )),
        );
        assert_eq!(
            harness.invocations().as_slice(),
            [Invocation::EstablishLink(endpoint().destination_hash())],
        );
    }

    #[test]
    fn failures_before_identification_cleanup_close_the_owned_link_once() {
        let cleanup = RemoteControlPairingLinkCleanupOutcome::Queued;
        let begin_failure =
            BeginRemoteControlControllerPairingFailure::ControllerIdentityUnavailable;
        let request_failure = RemoteControlControllerPairingRequestFailure {
            cause: RemoteControlControllerPairingRequestFailureCause::ResourceResponseUnsupported,
            exchange: FailRemoteControlControllerPairingRequestOutcome::NoActiveAttempt,
        };
        let failures = [
            (
                RemoteControlPairingControlError::NodeStopped,
                InitiateRemoteControlControllerPairingError::NodeStopped { cleanup },
            ),
            (
                RemoteControlPairingControlError::Busy,
                InitiateRemoteControlControllerPairingError::Busy { cleanup },
            ),
            (
                RemoteControlPairingControlError::Failed(
                    BeginRemoteControlControllerPairingControlFailure::Begin(begin_failure),
                ),
                InitiateRemoteControlControllerPairingError::Begin {
                    failure: begin_failure,
                    cleanup,
                },
            ),
            (
                RemoteControlPairingControlError::Failed(
                    BeginRemoteControlControllerPairingControlFailure::Request(request_failure),
                ),
                InitiateRemoteControlControllerPairingError::Request {
                    failure: request_failure,
                    cleanup,
                },
            ),
        ];

        for (error, expected) in failures {
            let harness = Harness::new(Ok(LINK_ID), Err(error), cleanup);

            assert_eq!(
                block_on(harness.initiate_remote_control_controller_pairing(initiate())),
                Err(expected),
            );
            assert_eq!(
                harness.invocations().as_slice(),
                [
                    Invocation::EstablishLink(endpoint().destination_hash()),
                    Invocation::Begin(expected_begin()),
                    Invocation::CloseLink(LINK_ID),
                ],
            );
        }
    }

    #[test]
    fn identification_failure_reuses_the_lower_operation_cleanup_without_closing_again() {
        let error = RemoteControlPairingControlError::Failed(
            BeginRemoteControlControllerPairingControlFailure::Identify {
                failure: SendError::Failed(IdentifyFailure::WriteFailed),
                cleanup: RemoteControlPairingLinkCleanupOutcome::NotQueued,
            },
        );
        let harness = Harness::new(
            Ok(LINK_ID),
            Err(error),
            RemoteControlPairingLinkCleanupOutcome::Queued,
        );

        assert_eq!(
            block_on(harness.initiate_remote_control_controller_pairing(initiate())),
            Err(InitiateRemoteControlControllerPairingError::Identify {
                failure: SendError::Failed(IdentifyFailure::WriteFailed),
                cleanup: RemoteControlPairingLinkCleanupOutcome::NotQueued,
            }),
        );
        assert_eq!(
            harness.invocations().as_slice(),
            [
                Invocation::EstablishLink(endpoint().destination_hash()),
                Invocation::Begin(expected_begin()),
            ],
        );
    }
}
