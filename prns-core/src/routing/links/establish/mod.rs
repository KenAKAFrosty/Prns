use crate::crypto::{
    x25519_diffie_hellman, x25519_public_key, Ed25519SecretKey, Ed25519Signature, X25519PublicKey,
    X25519SecretKey, X25519SharedSecret,
};
use crate::engine::{CommandId, CommandOutcome, EstablishLink, EstablishLinkRejection};
use crate::engine::{EngineState, InstantMillis};
use crate::identity::in_memory::InMemoryNodeIdentity;
use crate::identity::{IdentitySigner, IDENTITY_SECRET_KEY_LEN};
use crate::interfaces::{AttachedInterfaces, InterfaceId};
use crate::routing::links::handshake::{
    negotiated_link_mtu, write_link_proof, write_link_proof_from_parts, write_link_request,
    write_link_rtt, write_unsignalled_link_request, AcceptedLinkRequest, LinkProofSignOwed,
};
use crate::routing::links::table::{
    InitiatedLink, LinkActivation, LinkPhase, OverdueLink, RespondingLink, TrackLinkError,
};
use crate::routing::links::{LinkId, LinkKey, LinkMode, MAX_LINK_MTU};
use crate::routing::timing::{
    link_establishment_timeout_ms, FirstHopTiming, DEFAULT_PER_HOP_TIMEOUT_MS,
};
use crate::routing::NextHop;
use crate::storage::StorageLayout;
use crate::wire::BROADCAST_MTU;

pub const ESTABLISH_LINK_ENTROPY_LEN: usize = IDENTITY_SECRET_KEY_LEN;

pub fn link_mtu_ceiling(interfaces: AttachedInterfaces<'_>, interface_id: InterfaceId) -> usize {
    interfaces
        .descriptor_for(interface_id)
        .and_then(|descriptor| descriptor.hardware_mtu)
        .unwrap_or(BROADCAST_MTU)
        .min(MAX_LINK_MTU)
}

/// RNS 1.4.2 `Link.KEEPALIVE` (360s); the responder's establishment timeout rides on it.
pub const LINK_KEEPALIVE_MS: u64 = 360_000;

/// A fresh X25519 ‖ Ed25519 pair, the same layout an identity persists.
/// Move-only and never shown; consuming it keys exactly one link request, so one draw can never key two.
pub struct EstablishLinkEntropy([u8; ESTABLISH_LINK_ENTROPY_LEN]);

impl EstablishLinkEntropy {
    pub const LEN: usize = ESTABLISH_LINK_ENTROPY_LEN;

    pub const fn new(bytes: [u8; ESTABLISH_LINK_ENTROPY_LEN]) -> Self {
        Self(bytes)
    }

    fn into_parts(self) -> (X25519SecretKey, Ed25519SecretKey, InMemoryNodeIdentity) {
        let ephemeral = InMemoryNodeIdentity::from_secret_key_bytes(&self.0);
        let mut scalar = [0u8; 32];
        scalar.copy_from_slice(&self.0[..32]);
        let mut signing = [0u8; 32];
        signing.copy_from_slice(&self.0[32..]);
        (
            X25519SecretKey::new(scalar),
            Ed25519SecretKey::new(signing),
            ephemeral,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkRequestDispatch {
    pub wire_bytes: usize,
    pub fire_on: InterfaceId,
    pub link_id: LinkId,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct LinkRttTimes {
    pub activated_at: InstantMillis,
    pub evidence_observed_at: InstantMillis,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteEstablishLinkRejection {
    RouteVanished,
    Serialize,
    LinkTableFull,
    DuplicateLinkId,
}

impl From<TrackLinkError> for WriteEstablishLinkRejection {
    fn from(error: TrackLinkError) -> Self {
        match error {
            TrackLinkError::TableFull => Self::LinkTableFull,
            TrackLinkError::AlreadyTracked => Self::DuplicateLinkId,
        }
    }
}

#[must_use]
pub enum EstablishLinkWriteOutcome {
    Written(LinkRequestDispatch),
    Rejected {
        rejection: WriteEstablishLinkRejection,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteLinkProofError {
    IdentityNotHeld,
    Serialize,
    LinkTableFull,
    DuplicateLinkId,
}

impl From<TrackLinkError> for WriteLinkProofError {
    fn from(error: TrackLinkError) -> Self {
        match error {
            TrackLinkError::TableFull => Self::LinkTableFull,
            TrackLinkError::AlreadyTracked => Self::DuplicateLinkId,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteLinkRttError {
    NotPending,
    Serialize,
}

impl<S: StorageLayout> EngineState<S> {
    pub fn ingest_establish_link(&self, id: CommandId, establish: EstablishLink) -> CommandOutcome {
        if self
            .routing_table
            .stored_announce_for(&establish.destination)
            .is_none()
        {
            return CommandOutcome::EstablishLinkRejected {
                id,
                rejection: EstablishLinkRejection::NoRouteToDestination,
            };
        }
        CommandOutcome::OwesLinkRequest { id, establish }
    }

    /// RNS 1.4.2 `Link.__init__`, which always signals the default MTU and mode.
    pub fn write_commanded_link_request(
        &mut self,
        id: CommandId,
        establish: &EstablishLink,
        now: InstantMillis,
        entropy: EstablishLinkEntropy,
        interfaces: AttachedInterfaces<'_>,
        buf: &mut [u8],
    ) -> EstablishLinkWriteOutcome {
        self.write_commanded_link_request_with_timing(
            id,
            establish,
            now,
            entropy,
            FirstHopTiming {
                interfaces,
                shared_instance_floor_ms: None,
            },
            buf,
        )
    }

    pub fn write_commanded_link_request_with_timing(
        &mut self,
        id: CommandId,
        establish: &EstablishLink,
        now: InstantMillis,
        entropy: EstablishLinkEntropy,
        timing: FirstHopTiming<'_>,
        buf: &mut [u8],
    ) -> EstablishLinkWriteOutcome {
        use EstablishLinkWriteOutcome::{Rejected, Written};

        let Some(stored) = self
            .routing_table
            .stored_announce_for(&establish.destination)
        else {
            return Rejected {
                rejection: WriteEstablishLinkRejection::RouteVanished,
            };
        };
        let hops = stored.hops;
        let fire_on = stored.receiving_interface;
        let Some(route_evidence) = self
            .routing_table
            .route_evidence_handle_for(&establish.destination)
        else {
            return Rejected {
                rejection: WriteEstablishLinkRejection::RouteVanished,
            };
        };

        let (initiator_secret, link_signing, ephemeral) = entropy.into_parts();
        let encryption_public = *ephemeral.encryption_public_key().as_x25519();
        let signing_public = *ephemeral.signing_public_key().as_ed25519();
        let link_id = LinkId::derive(&establish.destination, &encryption_public, &signing_public);

        let via = match stored.next_hop {
            NextHop::Via(next) => Some(next),
            NextHop::Direct => None,
        };
        let mode = LinkMode::Aes256Cbc;
        let request = match self.protocol.link_mtu_discovery {
            crate::engine::LinkMtuDiscovery::Enabled => write_link_request(
                &establish.destination,
                via,
                &encryption_public,
                &signing_public,
                link_mtu_ceiling(timing.interfaces, fire_on),
                mode,
                buf,
            ),
            crate::engine::LinkMtuDiscovery::Disabled => write_unsignalled_link_request(
                &establish.destination,
                via,
                &encryption_public,
                &signing_public,
                buf,
            ),
        };
        let Ok(wire_bytes) = request else {
            return Rejected {
                rejection: WriteEstablishLinkRejection::Serialize,
            };
        };

        let bitrate = timing
            .interfaces
            .descriptor_for(fire_on)
            .filter(|descriptor| descriptor.capabilities.allows_transmit())
            .map(|descriptor| descriptor.bitrate);
        let computed = link_establishment_timeout_ms(hops, bitrate);
        let floor = timing.shared_instance_floor_ms.map(|first_hop| {
            first_hop
                .saturating_add(DEFAULT_PER_HOP_TIMEOUT_MS.saturating_mul(u64::from(hops.max(1))))
        });
        let timeout_at = InstantMillis(
            now.0
                .saturating_add(floor.map_or(computed, |floor| computed.max(floor))),
        );
        match self.links.track_initiated(InitiatedLink {
            link_id,
            destination: establish.destination,
            route_evidence,
            expected_hops: hops,
            mode,
            initiator_secret,
            link_signing,
            requested_at: now,
            timeout_at,
            command_id: id,
        }) {
            Ok(()) => Written(LinkRequestDispatch {
                wire_bytes,
                fire_on,
                link_id,
            }),
            Err(error) => Rejected {
                rejection: error.into(),
            },
        }
    }

    /// RNS 1.4.2 `Link.validate_request`, echoing the negotiated MTU and mode.
    pub fn write_owed_link_proof(
        &mut self,
        accepted: &AcceptedLinkRequest,
        ephemeral_secret: X25519SecretKey,
        mtu_ceiling: usize,
        buf: &mut [u8],
    ) -> Result<usize, WriteLinkProofError> {
        let request = &accepted.request;
        let held = self
            .held_identities
            .get(&accepted.identity)
            .ok_or(WriteLinkProofError::IdentityNotHeld)?;
        let responder_encryption = x25519_public_key(&ephemeral_secret);
        let shared = x25519_diffie_hellman(&ephemeral_secret, &request.initiator_encryption);
        let key = LinkKey::derive(&request.link_id, &shared);

        let mtu = negotiated_link_mtu(request.mtu, mtu_ceiling);
        let written = write_link_proof(
            &request.link_id,
            &responder_encryption,
            &held,
            mtu,
            request.mode,
            buf,
        )
        .map_err(|_| WriteLinkProofError::Serialize)?;
        self.track_responding_link(accepted, key, mtu)?;
        Ok(written)
    }

    /// The crypto-pool-friendly twin of [`Self::write_owed_link_proof`]; same bytes either way.
    pub fn write_owed_link_proof_with_parts(
        &mut self,
        owed: &LinkProofSignOwed,
        responder_encryption: &X25519PublicKey,
        shared: &X25519SharedSecret,
        signature: &Ed25519Signature,
        buf: &mut [u8],
    ) -> Result<usize, WriteLinkProofError> {
        let key = LinkKey::derive(&owed.request.link_id, shared);
        let written = write_link_proof_from_parts(
            &owed.request.link_id,
            responder_encryption,
            signature,
            owed.mtu,
            owed.request.mode,
            buf,
        )
        .map_err(|_| WriteLinkProofError::Serialize)?;
        self.track_responding_link(
            &AcceptedLinkRequest {
                request: owed.request,
                identity: owed.identity,
                proof_strategy: owed.proof_strategy,
                received_hops: owed.received_hops,
                arrived_at: owed.arrived_at,
            },
            key,
            owed.mtu,
        )?;
        Ok(written)
    }

    fn track_responding_link(
        &mut self,
        accepted: &AcceptedLinkRequest,
        key: LinkKey,
        mtu: usize,
    ) -> Result<(), WriteLinkProofError> {
        let &AcceptedLinkRequest {
            ref request,
            identity,
            proof_strategy,
            received_hops,
            arrived_at: requested_at,
            ..
        } = accepted;
        let timeout_at = InstantMillis(
            requested_at
                .0
                .saturating_add(
                    DEFAULT_PER_HOP_TIMEOUT_MS.saturating_mul(u64::from(received_hops.max(1))),
                )
                .saturating_add(LINK_KEEPALIVE_MS),
        );
        self.links
            .track_responding(RespondingLink {
                link_id: request.link_id,
                key,
                requested_at,
                timeout_at,
                mtu,
                initiator_signing: request.initiator_signing,
                destination: request.destination,
                identity,
                proof_strategy,
            })
            .map_err(Into::into)
    }

    /// RNS 1.4.2 `Link.validate_proof`
    pub fn write_owed_link_rtt(
        &mut self,
        link_id: &LinkId,
        responder_encryption: &X25519PublicKey,
        activation: &LinkActivation,
        now: InstantMillis,
        iv: &[u8; 16],
        buf: &mut [u8],
    ) -> Result<usize, WriteLinkRttError> {
        let shared = {
            let Some(LinkPhase::Pending {
                initiator_secret, ..
            }) = self.links.phase_for(link_id)
            else {
                return Err(WriteLinkRttError::NotPending);
            };
            x25519_diffie_hellman(initiator_secret, responder_encryption)
        };
        self.write_owed_link_rtt_with_shared(link_id, &shared, activation, now, iv, buf)
    }

    /// The crypto-pool-friendly twin of [`Self::write_owed_link_rtt`]; same bytes either way.
    pub fn write_owed_link_rtt_with_shared(
        &mut self,
        link_id: &LinkId,
        shared: &X25519SharedSecret,
        activation: &LinkActivation,
        now: InstantMillis,
        iv: &[u8; 16],
        buf: &mut [u8],
    ) -> Result<usize, WriteLinkRttError> {
        self.write_owed_link_rtt_with_shared_observed(
            link_id,
            shared,
            activation,
            LinkRttTimes {
                activated_at: now,
                evidence_observed_at: now,
            },
            iv,
            buf,
        )
    }

    pub(crate) fn write_owed_link_rtt_with_shared_observed(
        &mut self,
        link_id: &LinkId,
        shared: &X25519SharedSecret,
        activation: &LinkActivation,
        times: LinkRttTimes,
        iv: &[u8; 16],
        buf: &mut [u8],
    ) -> Result<usize, WriteLinkRttError> {
        let Some(LinkPhase::Pending {
            destination,
            route_evidence,
            expected_hops,
            ..
        }) = self.links.phase_for(link_id)
        else {
            return Err(WriteLinkRttError::NotPending);
        };
        let destination = *destination;
        let mut route_evidence = *route_evidence;
        let expected_hops = *expected_hops;
        let key = LinkKey::derive(link_id, shared);
        let written = write_link_rtt(link_id, &key, activation.rtt, iv, buf)
            .map_err(|_| WriteLinkRttError::Serialize)?;
        self.links
            .activate_initiated(link_id, key, activation, times.activated_at)
            .map_err(|_| WriteLinkRttError::NotPending)?;
        if activation.received_hops != expected_hops
            && self
                .routing_table
                .resolve_route_evidence(&mut route_evidence)
                .is_some()
        {
            self.routing_table
                .rebalance_hops(&destination, activation.received_hops);
        }
        self.mark_interface_dirty(activation.attached_interface);
        self.routing_table
            .apply_route_evidence(&mut route_evidence, times.evidence_observed_at);
        Ok(written)
    }

    pub fn pop_timed_out_link(&mut self, now: InstantMillis) -> Option<OverdueLink> {
        self.links.pop_overdue(now)
    }
}

#[cfg(test)]
mod tests;
