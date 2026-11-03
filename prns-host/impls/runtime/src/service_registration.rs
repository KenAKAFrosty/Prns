//! Construct staged native services from the host's canonical destination policy.
//! This conversion performs no file I/O, registration, key generation or dispatch.
use super::{
    engine_link_policy, engine_proof_strategy, engine_ratchet_policy, engine_request_policy,
    engine_resource_strategy, is_optional_safe_uint, HostStartError,
};
pub use personal_rns::identity::vault::IdentitySecretKey;
pub use personal_rns::runtime::{
    ServiceEndpoint, ServiceLifecycleError, ServicePause, ServiceRegistration,
};
use personal_rns::{routing::request_handlers::RequestPathHash, units::ByteLimit};
use prns_host::{DestinationIdentityConfig, SingleDestinationConfig};

/// Interpret `HostIdentity` as the caller's explicitly supplied service key.
/// A dedicated identity selector is rejected: resolve/retain its ownership before
/// calling this function instead of hiding initialization inside conversion.
/// Ingress always starts closed; the owning lifecycle activates it separately.
pub fn staged_service_registration(
    config: SingleDestinationConfig,
    secret: IdentitySecretKey,
) -> Result<ServiceRegistration, HostStartError> {
    if !matches!(config.identity, DestinationIdentityConfig::HostIdentity) {
        return Err(HostStartError::Destination(
            "staged registration requires an explicitly supplied service identity".into(),
        ));
    }
    if !is_optional_safe_uint(config.maximum_request_bytes) {
        return Err(HostStartError::Destination(
            "maximum request bytes must be an unsigned safe integer".into(),
        ));
    }
    let registration = ServiceRegistration {
        initially_accepting: false,
        secret,
        app_name: config.name.app_name().to_owned(),
        aspects: config.name.aspects().to_vec(),
        app_data: config.announce_app_data,
        proof_strategy: engine_proof_strategy(config.proof),
        link_request_policy: engine_link_policy(config.link_requests),
        ratchet_policy: engine_ratchet_policy(config.ratchet),
        resource_strategy: engine_resource_strategy(config.resource_strategy),
        maximum_request_bytes: config
            .maximum_request_bytes
            .map(ByteLimit::Maximum)
            .unwrap_or_default(),
        request_handlers: config
            .request_handlers
            .into_iter()
            .map(|handler| {
                (
                    RequestPathHash::of(&handler.path),
                    engine_request_policy(handler.policy),
                )
            })
            .collect(),
        restore_ratchets: None,
    };
    registration
        .validate_bounds()
        .map_err(|error| HostStartError::Destination(format!("service registration: {error}")))?;
    Ok(registration)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use prns_host::{
        DestinationLinkRequestPolicy, DestinationName, DestinationProofStrategy,
        DestinationRatchetPolicy, IdentityConfig, RequestHandlerConfig, RequestPolicy,
        ResourceStrategy,
    };
    fn config() -> SingleDestinationConfig {
        let mut config = SingleDestinationConfig::new(
            DestinationName::try_new("example", ["service".into()]).unwrap(),
            DestinationIdentityConfig::HostIdentity,
        );
        config.proof = DestinationProofStrategy::ProveIf;
        config.link_requests = DestinationLinkRequestPolicy::AcceptNone;
        config.ratchet = DestinationRatchetPolicy::RatchetsRequired;
        config.maximum_request_bytes = Some(4096);
        config.resource_strategy = ResourceStrategy::Accept {
            maximum_uncompressed_bytes: 8192,
            accept_compressed: true,
        };
        config.request_handlers.push(RequestHandlerConfig {
            path: "/example/query".into(),
            policy: RequestPolicy::AllowList,
        });
        config
    }
    #[test]
    fn staged_registration_preserves_canonical_policy_and_keeps_ingress_closed() {
        let registration =
            staged_service_registration(config(), IdentitySecretKey::new([7; 64])).unwrap();
        assert!(!registration.initially_accepting);
        assert_eq!(registration.app_name, "example");
        assert_eq!(registration.aspects, ["service"]);
        assert_eq!(
            registration.proof_strategy,
            personal_rns::ProofStrategy::ProveIf
        );
        assert_eq!(
            registration.link_request_policy,
            personal_rns::routing::LinkRequestPolicy::AcceptNone
        );
        assert_eq!(
            registration.ratchet_policy,
            personal_rns::RatchetPolicy::RatchetsRequired
        );
        assert_eq!(registration.maximum_request_bytes, ByteLimit::Maximum(4096));
        assert_eq!(
            registration.resource_strategy,
            personal_rns::ResourceStrategy::Accept {
                max_uncompressed_bytes: 8192,
                accept_compressed: true,
            }
        );
        assert_eq!(
            registration.request_handlers,
            [(
                RequestPathHash::of("/example/query"),
                personal_rns::routing::request_handlers::RequestPolicy::AllowList
            )]
        );
        assert!(registration.restore_ratchets.is_none());
    }
    #[test]
    fn supplied_identity_and_canonical_bounds_are_checked_before_registration() {
        let mut dedicated = config();
        dedicated.identity =
            DestinationIdentityConfig::Dedicated(IdentityConfig::GenerateEphemeral);
        assert!(staged_service_registration(dedicated, IdentitySecretKey::new([7; 64])).is_err());
        let mut unsafe_size = config();
        unsafe_size.maximum_request_bytes = Some(prns_host::SAFE_UINT_MAX + 1);
        assert!(staged_service_registration(unsafe_size, IdentitySecretKey::new([7; 64])).is_err());
        let mut too_many = config();
        too_many.request_handlers.resize(
            33,
            RequestHandlerConfig {
                path: "/example/query".into(),
                policy: RequestPolicy::AllowAll,
            },
        );
        assert!(staged_service_registration(too_many, IdentitySecretKey::new([7; 64])).is_err());
    }
}
