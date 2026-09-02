use crate::engine::{ConfigureRemoteControlPairingError, EngineState, RatchetPolicy};
use crate::identity::held::{HoldIdentityError, ReleaseHeldIdentityOutcome};
use crate::identity::IdentityHash;
use crate::remote_control::{
    RemoteControlEndpoint, RemoteControlNodeIdentities, RemoteControlNodeIdentitySecrets,
    RemoteControlPairingAvailabilityDestination, RemoteControlPairingView,
    REMOTE_CONTROL_APPLICATION_ASPECTS, REMOTE_CONTROL_APPLICATION_NAME,
    REMOTE_CONTROL_REQUEST_ENDPOINT_ID,
};
use crate::routing::request_handlers::{RequestPathHash, RequestPolicy};
use crate::routing::upstream_app_destinations::{
    LinkRequestPolicy, ProofStrategy, RegisterDestinationError, UnregisterRegistrationOutcome,
};
use crate::storage::{StorageLayout, TablePushError};
use crate::units::ByteLimit;

use super::{ConfigureRemoteControlIdentitiesError, RemoteControlControllerIdentityConfiguration};

pub struct RemoteControlServiceConfiguration {
    pub identity_secrets: RemoteControlNodeIdentitySecrets,
    pub maximum_request_bytes: ByteLimit,
}

#[derive(Debug, PartialEq, Eq)]
pub struct ConfiguredRemoteControlService {
    identities: RemoteControlNodeIdentities,
    target_endpoint: RemoteControlEndpoint,
    request_endpoint_id: RequestPathHash,
    pairing_availability_destination: RemoteControlPairingAvailabilityDestination,
}

impl ConfiguredRemoteControlService {
    #[must_use]
    pub fn into_parts(
        self,
    ) -> (
        RemoteControlNodeIdentities,
        RemoteControlEndpoint,
        RequestPathHash,
        RemoteControlPairingAvailabilityDestination,
    ) {
        (
            self.identities,
            self.target_endpoint,
            self.request_endpoint_id,
            self.pairing_availability_destination,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigureRemoteControlServiceError {
    ConfiguredIdentities(ConfigureRemoteControlIdentitiesError),
    ControllerIdentityAlreadyHeld(IdentityHash),
    TargetIdentityAlreadyHeld(IdentityHash),
    TargetDestinationAlreadyRegistered(RemoteControlEndpoint),
    TargetRequestHandlerAlreadyRegistered {
        destination: RemoteControlEndpoint,
        path: RequestPathHash,
    },
    TargetAndPairingAvailabilityDestinationCollision(RemoteControlEndpoint),
    PairingAvailabilityAlreadyRegistered(RemoteControlPairingAvailabilityDestination),
    RegisterTarget(RegisterDestinationError),
    ConfigureRequestLimit,
    RegisterRequestEndpoint(TablePushError),
    ConfigurePairing(ConfigureRemoteControlPairingError),
}

impl<S: StorageLayout> EngineState<S> {
    pub fn configure_remote_control_service(
        &mut self,
        configuration: RemoteControlServiceConfiguration,
    ) -> Result<ConfiguredRemoteControlService, ConfigureRemoteControlServiceError> {
        let identities = configuration.identity_secrets.identities();
        let controller_identity = identities.controller().identity_hash();
        let target_identity = identities.target().identity_hash();
        let target_endpoint = identities.target().endpoint();
        let request_endpoint_id = RequestPathHash::of(REMOTE_CONTROL_REQUEST_ENDPOINT_ID);
        let pairing_availability_destination =
            RemoteControlPairingAvailabilityDestination::canonical();

        self.validate_remote_control_service_configuration(
            controller_identity,
            target_identity,
            target_endpoint,
            pairing_availability_destination,
        )?;

        self.configure_remote_control_identities(configuration.identity_secrets)
            .map_err(ConfigureRemoteControlServiceError::ConfiguredIdentities)?;

        let configured_pairing_availability = self.configure_remote_control_registrations(
            target_identity,
            request_endpoint_id,
            configuration.maximum_request_bytes,
        );
        let configured_pairing_availability = match configured_pairing_availability {
            Ok(configured_pairing_availability) => configured_pairing_availability,
            Err(error) => {
                self.rollback_remote_control_service_configuration(
                    controller_identity,
                    target_identity,
                    target_endpoint,
                    request_endpoint_id,
                    pairing_availability_destination,
                );
                return Err(error);
            }
        };

        Ok(ConfiguredRemoteControlService {
            identities,
            target_endpoint,
            request_endpoint_id,
            pairing_availability_destination: configured_pairing_availability,
        })
    }

    fn validate_remote_control_service_configuration(
        &self,
        controller_identity: IdentityHash,
        target_identity: IdentityHash,
        target_endpoint: RemoteControlEndpoint,
        pairing_availability_destination: RemoteControlPairingAvailabilityDestination,
    ) -> Result<(), ConfigureRemoteControlServiceError> {
        match self.remote_control_controller_identity {
            RemoteControlControllerIdentityConfiguration::Unavailable => {}
            RemoteControlControllerIdentityConfiguration::Configured(_) => {
                return Err(ConfigureRemoteControlServiceError::ConfiguredIdentities(
                    ConfigureRemoteControlIdentitiesError::AlreadyConfigured,
                ))
            }
        }
        match self.remote_control_pairing.view() {
            RemoteControlPairingView::Unavailable => {}
            RemoteControlPairingView::Closed | RemoteControlPairingView::Open(_) => {
                return Err(ConfigureRemoteControlServiceError::ConfigurePairing(
                    ConfigureRemoteControlPairingError::AlreadyConfigured,
                ))
            }
        }
        if self.held_identities.contains(&controller_identity) {
            return Err(
                ConfigureRemoteControlServiceError::ControllerIdentityAlreadyHeld(
                    controller_identity,
                ),
            );
        }
        if self.held_identities.contains(&target_identity) {
            return Err(
                ConfigureRemoteControlServiceError::TargetIdentityAlreadyHeld(target_identity),
            );
        }
        if !self.held_identities.has_capacity_for(2) {
            return Err(ConfigureRemoteControlServiceError::ConfiguredIdentities(
                ConfigureRemoteControlIdentitiesError::Hold(HoldIdentityError::StoreFull),
            ));
        }
        if target_endpoint.destination_hash() == pairing_availability_destination.destination_hash()
        {
            return Err(
                ConfigureRemoteControlServiceError::TargetAndPairingAvailabilityDestinationCollision(
                    target_endpoint,
                ),
            );
        }
        if self
            .upstream_app_destinations
            .registration_for(&target_endpoint.destination_hash())
            .is_some()
        {
            return Err(
                ConfigureRemoteControlServiceError::TargetDestinationAlreadyRegistered(
                    target_endpoint,
                ),
            );
        }
        if let Some(path) = self
            .request_handlers
            .first_path_for_destination(&target_endpoint.destination_hash())
        {
            return Err(
                ConfigureRemoteControlServiceError::TargetRequestHandlerAlreadyRegistered {
                    destination: target_endpoint,
                    path,
                },
            );
        }
        if self
            .upstream_app_destinations
            .registration_for(&pairing_availability_destination.destination_hash())
            .is_some()
        {
            return Err(
                ConfigureRemoteControlServiceError::PairingAvailabilityAlreadyRegistered(
                    pairing_availability_destination,
                ),
            );
        }
        if !self.upstream_app_destinations.has_capacity_for(1) {
            return Err(ConfigureRemoteControlServiceError::RegisterTarget(
                RegisterDestinationError::RegistryFull,
            ));
        }
        if !self.upstream_app_destinations.has_capacity_for(2) {
            return Err(ConfigureRemoteControlServiceError::ConfigurePairing(
                ConfigureRemoteControlPairingError::RegisterAvailability(
                    RegisterDestinationError::RegistryFull,
                ),
            ));
        }
        if !self.request_handlers.has_capacity_for(1) {
            return Err(ConfigureRemoteControlServiceError::RegisterRequestEndpoint(
                TablePushError::TableFull,
            ));
        }
        Ok(())
    }

    fn configure_remote_control_registrations(
        &mut self,
        target_identity: IdentityHash,
        request_endpoint_id: RequestPathHash,
        maximum_request_bytes: ByteLimit,
    ) -> Result<RemoteControlPairingAvailabilityDestination, ConfigureRemoteControlServiceError>
    {
        let destination = self
            .register_single_destination(
                &target_identity,
                REMOTE_CONTROL_APPLICATION_NAME,
                REMOTE_CONTROL_APPLICATION_ASPECTS,
                b"",
                ProofStrategy::ProveAll,
                LinkRequestPolicy::AcceptAll,
                RatchetPolicy::NoRatchets,
            )
            .map_err(ConfigureRemoteControlServiceError::RegisterTarget)?;
        if !self.set_maximum_request_bytes(&destination, maximum_request_bytes) {
            return Err(ConfigureRemoteControlServiceError::ConfigureRequestLimit);
        }
        self.register_request_handler_hash(
            &destination,
            request_endpoint_id,
            RequestPolicy::RequireIdentified,
        )
        .map_err(ConfigureRemoteControlServiceError::RegisterRequestEndpoint)?;
        self.configure_remote_control_pairing(target_identity)
            .map_err(ConfigureRemoteControlServiceError::ConfigurePairing)
    }

    fn rollback_remote_control_service_configuration(
        &mut self,
        controller_identity: IdentityHash,
        target_identity: IdentityHash,
        target_endpoint: RemoteControlEndpoint,
        request_endpoint_id: RequestPathHash,
        pairing_availability_destination: RemoteControlPairingAvailabilityDestination,
    ) {
        self.request_handlers
            .unregister(&target_endpoint.destination_hash(), &request_endpoint_id);
        match self
            .upstream_app_destinations
            .unregister(&pairing_availability_destination.destination_hash())
        {
            UnregisterRegistrationOutcome::Unregistered { .. }
            | UnregisterRegistrationOutcome::NotRegistered => {}
        }
        match self
            .upstream_app_destinations
            .unregister(&target_endpoint.destination_hash())
        {
            UnregisterRegistrationOutcome::Unregistered { .. }
            | UnregisterRegistrationOutcome::NotRegistered => {}
        }
        self.remote_control_controller_identity =
            RemoteControlControllerIdentityConfiguration::Unavailable;
        match self.held_identities.release(&target_identity) {
            ReleaseHeldIdentityOutcome::Released | ReleaseHeldIdentityOutcome::NotHeld => {}
        }
        match self.held_identities.release(&controller_identity) {
            ReleaseHeldIdentityOutcome::Released | ReleaseHeldIdentityOutcome::NotHeld => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::vault::IdentitySecretKey;
    use crate::identity::IDENTITY_SECRET_KEY_LEN;
    use crate::remote_control::{
        RemoteControlControllerIdentitySecret, RemoteControlNodeIdentitySecrets,
        RemoteControlPairingView, RemoteControlTargetIdentitySecret,
    };
    use crate::storage::TestFixedStorage;

    type EnoughStorage = TestFixedStorage<4, 4, 128, 3, 3, 4, 2, 2, 2, 2, 2, 2>;
    type OneDestinationStorage = TestFixedStorage<4, 4, 128, 1, 3, 4, 2, 2, 2, 2, 2, 2>;

    fn identity_secrets() -> RemoteControlNodeIdentitySecrets {
        RemoteControlNodeIdentitySecrets::new(
            RemoteControlControllerIdentitySecret::from(IdentitySecretKey::new(
                [0x31; IDENTITY_SECRET_KEY_LEN],
            )),
            RemoteControlTargetIdentitySecret::from(IdentitySecretKey::new(
                [0x32; IDENTITY_SECRET_KEY_LEN],
            )),
        )
        .unwrap()
    }

    fn configuration() -> RemoteControlServiceConfiguration {
        RemoteControlServiceConfiguration {
            identity_secrets: identity_secrets(),
            maximum_request_bytes: ByteLimit::Maximum(512),
        }
    }

    #[test]
    fn configures_the_complete_remote_control_service_surface() {
        let mut engine = EngineState::<EnoughStorage>::default();
        let expected_identities = configuration().identity_secrets.identities();
        let expected_target = expected_identities.target().endpoint();
        let expected_pairing = RemoteControlPairingAvailabilityDestination::canonical();

        let configured = engine
            .configure_remote_control_service(configuration())
            .unwrap();

        assert_eq!(
            configured.into_parts(),
            (
                expected_identities,
                expected_target,
                RequestPathHash::of(REMOTE_CONTROL_REQUEST_ENDPOINT_ID),
                expected_pairing,
            ),
        );
        assert_eq!(engine.held_identity_hashes().len(), 2);
        assert_eq!(engine.upstream_app_destinations().count(), 2);
        assert_eq!(
            engine.remote_control_pairing_view(),
            RemoteControlPairingView::Closed
        );
    }

    #[test]
    fn capacity_rejection_leaves_the_engine_unconfigured() {
        let mut engine = EngineState::<OneDestinationStorage>::default();

        assert_eq!(
            engine.configure_remote_control_service(configuration()),
            Err(ConfigureRemoteControlServiceError::ConfigurePairing(
                ConfigureRemoteControlPairingError::RegisterAvailability(
                    RegisterDestinationError::RegistryFull,
                ),
            )),
        );
        assert!(engine.held_identity_hashes().is_empty());
        assert_eq!(engine.upstream_app_destinations().count(), 0);
        assert!(engine.request_handlers.is_empty());
        assert_eq!(
            engine.remote_control_pairing_view(),
            RemoteControlPairingView::Unavailable,
        );
        assert_eq!(
            engine.remote_control_controller_identity,
            RemoteControlControllerIdentityConfiguration::Unavailable,
        );
    }

    #[test]
    fn target_request_handler_collision_leaves_the_engine_unconfigured() {
        let mut engine = EngineState::<EnoughStorage>::default();
        let target_endpoint = configuration()
            .identity_secrets
            .identities()
            .target()
            .endpoint();
        let existing_path = RequestPathHash::of("/existing");
        engine
            .register_request_handler_hash(
                &target_endpoint.destination_hash(),
                existing_path,
                RequestPolicy::AllowAll,
            )
            .unwrap();

        assert_eq!(
            engine.configure_remote_control_service(configuration()),
            Err(
                ConfigureRemoteControlServiceError::TargetRequestHandlerAlreadyRegistered {
                    destination: target_endpoint,
                    path: existing_path,
                },
            ),
        );
        assert!(engine.held_identity_hashes().is_empty());
        assert_eq!(engine.upstream_app_destinations().count(), 0);
        assert!(engine.request_handlers.permits(
            &target_endpoint.destination_hash(),
            &existing_path,
            None,
        ));
        assert_eq!(
            engine.remote_control_pairing_view(),
            RemoteControlPairingView::Unavailable,
        );
        assert_eq!(
            engine.remote_control_controller_identity,
            RemoteControlControllerIdentityConfiguration::Unavailable,
        );
    }
}
