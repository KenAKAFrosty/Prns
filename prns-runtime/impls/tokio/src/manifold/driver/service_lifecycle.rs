use std::sync::Arc;

use crate::engine::{EngineReaction, EngineState, InstantMillis, RetireDestinationOutcome};
use crate::identity::{IdentityHash, PrivateIdentityMaterial};
use crate::manifold::Host;
use crate::routing::announce::{derive_destination_hash, expand_name};
use crate::routing::links::LinkId;
use crate::runtime::node_facade::service_lifecycle::{
    ServiceCommand, ServiceLifecycleError as Error, ServicePause, ServiceRegistration,
};
use crate::storage::StorageLayout;
use crate::wire::{DestinationHash, WirePacketHeader};

use super::command_dispatch::{CommandDispatch, CommandEffect};
use super::egress::route_reaction;

const MAX_SERVICES: usize = 64;
#[derive(Default)]
pub(super) struct ServiceRegistry {
    entries: Vec<ServiceEntry>,
}
struct ServiceEntry {
    endpoint: u64,
    registration: ServiceRegistration,
    identity: IdentityHash,
    destination: DestinationHash,
    introduced_key: bool,
    pause: Option<ServicePause>,
    outgoing: Vec<LinkId>,
}

fn registration_matches(a: &ServiceRegistration, b: &ServiceRegistration) -> bool {
    a.initially_accepting == b.initially_accepting
        && a.secret.as_slice() == b.secret.as_slice()
        && a.app_name == b.app_name
        && a.aspects == b.aspects
        && a.app_data == b.app_data
        && a.proof_strategy == b.proof_strategy
        && a.link_request_policy == b.link_request_policy
        && a.ratchet_policy == b.ratchet_policy
        && a.resource_strategy == b.resource_strategy
        && a.maximum_request_bytes == b.maximum_request_bytes
        && a.request_handlers == b.request_handlers
        && a.restore_ratchets == b.restore_ratchets
}

impl ServiceRegistry {
    fn register<S: StorageLayout>(
        &mut self,
        engine: &mut EngineState<S>,
        endpoint: u64,
        registration: ServiceRegistration,
    ) -> Result<DestinationHash, Error> {
        registration.validate_bounds()?;
        if let Some(existing) = self.entries.iter().find(|entry| entry.endpoint == endpoint) {
            return if existing.pause.is_some() {
                Err(Error::WrongPhase)
            } else if registration_matches(&existing.registration, &registration) {
                Ok(existing.destination)
            } else {
                Err(Error::Conflict)
            };
        }
        if self.entries.len() == MAX_SERVICES {
            return Err(Error::Capacity);
        }
        let identity = PrivateIdentityMaterial::from(registration.secret.clone()).identity_hash();
        let aspects: Vec<_> = registration.aspects.iter().map(String::as_str).collect();
        let name = expand_name(&registration.app_name, &aspects).map_err(|_| Error::Conflict)?;
        let destination = derive_destination_hash(&identity, &name);
        if engine
            .upstream_app_destinations()
            .any(|entry| entry.destination == destination)
        {
            return Err(Error::Conflict);
        }
        // Validate before publishing any registration or held-key mutation.
        if let Some(bytes) = &registration.restore_ratchets {
            crate::persistence::read_self_ratchets_snapshot(bytes)
                .map_err(|_| Error::InvalidRatchets)?;
            if registration.ratchet_policy == crate::crypto::ratchets::RatchetPolicy::NoRatchets {
                return Err(Error::InvalidRatchets);
            }
        }
        let introduced_key = !engine.held_identity_hashes().contains(&identity);
        engine
            .hold_identity(registration.secret.clone())
            .map_err(|_| Error::Capacity)?;
        let registered = engine.register_single_destination(
            &identity,
            &registration.app_name,
            &aspects,
            &registration.app_data,
            registration.proof_strategy,
            registration.link_request_policy,
            registration.ratchet_policy,
        );
        let result = (|| {
            registered.map_err(|_| Error::Capacity)?;
            if !engine.set_single_ingress_enabled(&destination, registration.initially_accepting) {
                return Err(Error::Conflict);
            }
            if let Some(bytes) = &registration.restore_ratchets {
                let record = crate::persistence::read_self_ratchets_snapshot(bytes)
                    .map_err(|_| Error::InvalidRatchets)?;
                match engine.seed_self_ratchets(
                    &destination,
                    record.last_rotated,
                    record.secrets_newest_first(),
                ) {
                    crate::crypto::ratchets::SeedSelfRatchetsOutcome::Seeded => {}
                    _ => return Err(Error::InvalidRatchets),
                }
            }
            if !engine.set_default_resource_strategy(&destination, registration.resource_strategy) {
                return Err(Error::Conflict);
            }
            if !engine.set_maximum_request_bytes(&destination, registration.maximum_request_bytes) {
                return Err(Error::Conflict);
            }
            for &(path, policy) in &registration.request_handlers {
                engine
                    .register_request_handler_hash(&destination, path, policy)
                    .map_err(|_| Error::Capacity)?;
            }
            Ok(destination)
        })();
        if result.is_err() {
            let _ = engine.unregister_destination(&destination);
            if introduced_key {
                let _ = engine.release_unregistered_service_identity(&identity);
            }
            return result;
        }
        self.entries.push(ServiceEntry {
            endpoint,
            registration,
            identity,
            destination,
            introduced_key,
            pause: None,
            outgoing: Vec::new(),
        });
        result
    }
}

impl<S, H, J> CommandDispatch<'_, S, H, J>
where
    S: StorageLayout,
    H: Host,
    J: for<'a> FnMut(crate::engine::Journaled<'a>),
{
    pub(super) fn service_lifecycle(
        self,
        command: ServiceCommand,
        now: InstantMillis,
    ) -> CommandEffect {
        let Self {
            engine,
            host,
            topology,
            wire_scratch,
            journal,
            ..
        } = self;
        match command {
            ServiceCommand::ActivateIngress { endpoint, reply } => {
                let result = match topology
                    .services
                    .entries
                    .iter()
                    .find(|entry| entry.endpoint == endpoint)
                {
                    Some(entry) if entry.pause.is_some() => Err(Error::WrongPhase),
                    Some(entry) if engine.set_single_ingress_enabled(&entry.destination, true) => {
                        Ok(())
                    }
                    Some(_) => Err(Error::Conflict),
                    None => Err(Error::NotRegistered),
                };
                let _ = reply.send(result);
            }
            ServiceCommand::Register {
                endpoint,
                registration,
                reply,
            } => {
                let result = topology.services.register(engine, endpoint, *registration);
                let _ = reply.send(result);
            }
            ServiceCommand::PreparePause {
                endpoint,
                outgoing_links,
                reply,
            } => {
                if outgoing_links.len() > 256 {
                    let _ = reply.send(Err(Error::Bounds));
                    return CommandEffect::RecomputeWakeSchedules;
                }
                let Some(index) = topology
                    .services
                    .entries
                    .iter()
                    .position(|entry| entry.endpoint == endpoint)
                else {
                    let _ = reply.send(Err(Error::NotRegistered));
                    return CommandEffect::RecomputeWakeSchedules;
                };
                if let Some(pause) = &topology.services.entries[index].pause {
                    let result = if topology.services.entries[index].outgoing == outgoing_links {
                        Ok(pause.clone())
                    } else {
                        Err(Error::Conflict)
                    };
                    let _ = reply.send(result);
                    return CommandEffect::RecomputeWakeSchedules;
                }
                let destination = topology.services.entries[index].destination;
                let ratchets = prns_runtime::runtime::persistence_snapshots::snapshot_self_ratchet(
                    engine,
                    destination,
                )
                .map(|snapshot| snapshot.sealed);
                if topology.services.entries[index].registration.ratchet_policy
                    != crate::crypto::ratchets::RatchetPolicy::NoRatchets
                    && ratchets.is_none()
                {
                    let _ = reply.send(Err(Error::InvalidRatchets));
                    return CommandEffect::RecomputeWakeSchedules;
                }
                // Drained producer tasks supply initiator ownership. Inbound Links
                // are discovered by the engine from this exact local destination.
                for link_id in &outgoing_links {
                    engine.retire_service_link(
                        link_id,
                        topology.interfaces.view(),
                        &mut |bytes| host.fill_random(bytes),
                        &mut |reaction| {
                            route_reaction(
                                reaction,
                                &mut topology.egress,
                                &topology.ifacs,
                                &mut topology.pacers,
                                wire_scratch,
                                now,
                                &mut |event| journal.route(event),
                            );
                        },
                    );
                }
                let result = engine.retire_destination(
                    &destination,
                    topology.interfaces.view(),
                    &mut |bytes| host.fill_random(bytes),
                    &mut |reaction: EngineReaction<'_>| {
                        route_reaction(
                            reaction,
                            &mut topology.egress,
                            &topology.ifacs,
                            &mut topology.pacers,
                            wire_scratch,
                            now,
                            &mut |event| journal.route(event),
                        );
                    },
                );
                if !matches!(result, RetireDestinationOutcome::Retired { .. }) {
                    let _ = reply.send(Err(Error::RetirementIncomplete));
                    return CommandEffect::RecomputeWakeSchedules;
                }
                for pacer in topology.pacers.iter_mut() {
                    pacer.pacer.remove_matching(|bytes| {
                        WirePacketHeader::parse(bytes).is_ok_and(|(header, _)| {
                            header.address.as_bytes() == destination.as_bytes()
                        })
                    });
                }
                let pause = ServicePause {
                    endpoint,
                    token: Arc::new(()),
                    destination,
                    ratchets,
                    fences: topology.egress.drain_fences(),
                };
                topology.services.entries[index].outgoing = outgoing_links;
                topology.services.entries[index].pause = Some(pause.clone());
                let _ = reply.send(Ok(pause));
            }
            ServiceCommand::FinishPause {
                endpoint,
                token,
                reply,
            } => {
                let Some(index) = topology
                    .services
                    .entries
                    .iter()
                    .position(|entry| entry.endpoint == endpoint)
                else {
                    // A lost final acknowledgement is idempotent. The opaque
                    // endpoint id is never reused by this node's handle issuer.
                    let _ = reply.send(Ok(()));
                    return CommandEffect::RecomputeWakeSchedules;
                };
                let result = match &topology.services.entries[index].pause {
                    Some(pause) if !Arc::ptr_eq(&pause.token, &token) => Err(Error::Conflict),
                    Some(pause) if !pause.is_drained() => Err(Error::NotDrained),
                    Some(_) => Ok(()),
                    None => Err(Error::WrongPhase),
                };
                if result.is_ok() {
                    let removed = topology.services.entries.swap_remove(index);
                    if removed.introduced_key {
                        if let Some(other) = topology
                            .services
                            .entries
                            .iter_mut()
                            .find(|entry| entry.identity == removed.identity)
                        {
                            other.introduced_key = true;
                        } else {
                            let _ = engine.release_unregistered_service_identity(&removed.identity);
                        }
                    }
                }
                let _ = reply.send(result);
            }
        }
        CommandEffect::RecomputeWakeSchedules
    }
}
