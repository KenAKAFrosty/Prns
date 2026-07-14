use crate::crypto::{Ed25519PublicKey, Ed25519SecretKey, X25519SecretKey};
use crate::engine::CommandId;
use crate::engine::InstantMillis;
use crate::identity::IdentityHash;
use crate::interfaces::InterfaceId;
use crate::routing::links::maintenance::{keepalive_ms_from, stale_ms_from, timeout_grace_ms_from};
use crate::routing::links::resources::ResourceStrategy;
use crate::routing::links::{LinkId, LinkKey};
use crate::routing::upstream_app_destinations::ProofStrategy;
use crate::units::RttMillis;
use crate::wire::DestinationHash;

pub enum LinkRole {
    Initiator {
        link_signing: Ed25519SecretKey,
    },
    Responder {
        destination: DestinationHash,
        identity: IdentityHash,
        proof_strategy: ProofStrategy,
    },
}

// Holds the initiator's per-link signing secret, so Debug can't be derived (Ed25519SecretKey deliberately has no Debug to leak).
impl core::fmt::Debug for LinkRole {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Initiator { .. } => f.debug_struct("Initiator").finish_non_exhaustive(),
            Self::Responder {
                destination,
                identity,
                proof_strategy,
            } => f
                .debug_struct("Responder")
                .field("destination", destination)
                .field("identity", identity)
                .field("proof_strategy", proof_strategy)
                .finish(),
        }
    }
}

pub enum LinkPhase {
    Pending {
        destination: DestinationHash,
        initiator_secret: X25519SecretKey,
        link_signing: Ed25519SecretKey,
        requested_at: InstantMillis,
        command_id: CommandId,
    },
    Handshake {
        key: LinkKey,
        requested_at: InstantMillis,
        mtu: usize,
        initiator_signing: Ed25519PublicKey,
        destination: DestinationHash,
        identity: IdentityHash,
        proof_strategy: ProofStrategy,
    },
    Active {
        key: LinkKey,
        role: LinkRole,
        rtt: RttMillis,
        mtu: usize,
        attached_interface: InterfaceId,
        last_inbound: InstantMillis,
        last_keepalive_sent: InstantMillis,
        keepalive_ms: u64,
        peer_signing: Ed25519PublicKey,
        remote_identity: Option<IdentityHash>,
        resource_strategy: ResourceStrategy,
        last_resource_window: Option<usize>,
        last_resource_eifr: Option<u64>,
    },
}

impl LinkPhase {
    pub fn vacant() -> Self {
        Self::Pending {
            destination: DestinationHash::new([0u8; 16]),
            initiator_secret: X25519SecretKey::new([0u8; 32]),
            link_signing: Ed25519SecretKey::new([0u8; 32]),
            requested_at: InstantMillis(0),
            command_id: CommandId(0),
        }
    }
}

// The Pending phase holds the initiator secret, so Debug can't be derived (X25519SecretKey deliberately has no Debug to leak).
impl core::fmt::Debug for LinkPhase {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Pending {
                destination,
                requested_at,
                command_id,
                ..
            } => f
                .debug_struct("Pending")
                .field("destination", destination)
                .field("requested_at", requested_at)
                .field("command_id", command_id)
                .finish_non_exhaustive(),
            Self::Handshake {
                key,
                requested_at,
                mtu,
                ..
            } => f
                .debug_struct("Handshake")
                .field("key", key)
                .field("requested_at", requested_at)
                .field("mtu", mtu)
                .finish(),
            Self::Active {
                key,
                role,
                rtt,
                mtu,
                attached_interface,
                last_inbound,
                last_keepalive_sent,
                keepalive_ms,
                ..
            } => f
                .debug_struct("Active")
                .field("key", key)
                .field("role", role)
                .field("rtt", rtt)
                .field("mtu", mtu)
                .field("attached_interface", attached_interface)
                .field("last_inbound", last_inbound)
                .field("last_keepalive_sent", last_keepalive_sent)
                .field("keepalive_ms", keepalive_ms)
                .finish(),
        }
    }
}

pub struct InitiatedLink {
    pub link_id: LinkId,
    pub destination: DestinationHash,
    pub initiator_secret: X25519SecretKey,
    pub link_signing: Ed25519SecretKey,
    pub requested_at: InstantMillis,
    pub timeout_at: InstantMillis,
    pub command_id: CommandId,
}

pub struct RespondingLink {
    pub link_id: LinkId,
    pub key: LinkKey,
    pub requested_at: InstantMillis,
    pub timeout_at: InstantMillis,
    pub mtu: usize,
    pub initiator_signing: Ed25519PublicKey,
    pub destination: DestinationHash,
    pub identity: IdentityHash,
    pub proof_strategy: ProofStrategy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DueKeepalive {
    pub link_id: LinkId,
    pub attached_interface: InterfaceId,
}

fn teardown_at(last_inbound: InstantMillis, keepalive_ms: u64, rtt: RttMillis) -> u64 {
    last_inbound
        .0
        .saturating_add(stale_ms_from(keepalive_ms))
        .saturating_add(timeout_grace_ms_from(rtt))
}

fn active_deadline(
    role: &LinkRole,
    last_inbound: InstantMillis,
    last_keepalive_sent: InstantMillis,
    keepalive_ms: u64,
    rtt: RttMillis,
) -> InstantMillis {
    let teardown_at = teardown_at(last_inbound, keepalive_ms, rtt);
    match role {
        LinkRole::Responder { .. } => InstantMillis(teardown_at),
        LinkRole::Initiator { .. } => {
            let stale_at = last_inbound.0.saturating_add(stale_ms_from(keepalive_ms));
            let send_at = last_inbound
                .0
                .max(last_keepalive_sent.0)
                .saturating_add(keepalive_ms);
            if send_at <= stale_at {
                InstantMillis(send_at)
            } else {
                InstantMillis(teardown_at)
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverdueLink {
    Initiated {
        link_id: LinkId,
        command_id: CommandId,
        destination: DestinationHash,
    },
    Responding {
        link_id: LinkId,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackLinkError {
    TableFull,
    AlreadyTracked,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkActivation {
    pub rtt: RttMillis,
    pub mtu: usize,
    pub attached_interface: InterfaceId,
    pub peer_signing: Ed25519PublicKey,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkActivationError {
    UnknownLink,
    WrongPhase,
}

pub trait LinkTable {
    fn capacity(&self) -> usize;
    fn len(&self) -> usize;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn link_ids(&self) -> &[LinkId];
    fn timeout_ats(&self) -> &[Option<InstantMillis>];
    fn phases(&self) -> &[LinkPhase];

    fn phase_mut(&mut self, index: usize) -> &mut LinkPhase;
    fn set_timeout_at(&mut self, index: usize, timeout_at: Option<InstantMillis>);
    fn push(
        &mut self,
        link_id: LinkId,
        phase: LinkPhase,
        timeout_at: Option<InstantMillis>,
    ) -> Result<usize, TrackLinkError>;
    fn swap_remove(&mut self, index: usize);

    fn index_of(&self, link_id: &LinkId) -> Option<usize> {
        self.link_ids()
            .iter()
            .position(|candidate| candidate == link_id)
    }

    fn earliest_indexed_timeout(&mut self) -> Option<InstantMillis> {
        self.timeout_ats().iter().flatten().min().copied()
    }

    fn first_due_timeout_matching<P>(
        &mut self,
        now: InstantMillis,
        mut predicate: P,
    ) -> Option<usize>
    where
        P: FnMut(usize, &LinkPhase) -> bool,
    {
        (0..self.len()).find(|&index| {
            self.timeout_ats()[index].is_some_and(|at| at <= now)
                && predicate(index, &self.phases()[index])
        })
    }
}

#[derive(Debug, Default)]
pub struct Links<C: LinkTable> {
    table: C,
    earliest_timeout: Option<InstantMillis>,
}

/// An active link's transport view: the fields a sender seals and fires with. `key` stays borrowed from the link column (it is [`ZeroizeOnDrop`](zeroize::ZeroizeOnDrop), so never copied out); the rest are `Copy`. Returned by [`Links::active_view`].
pub(crate) struct ActiveLinkView<'a> {
    pub key: &'a LinkKey,
    pub mtu: usize,
    pub attached_interface: InterfaceId,
    pub rtt: RttMillis,
}

/// What [`Links::active_view`] found. A bare `phase_for` collapses "no such link" and "link present but not yet active" into one `None`; a sender must tell them apart to reject with the right reason (`NoSuchLink` vs `LinkNotActive`).
pub(crate) enum ActiveLinkLookup<'a> {
    Active(ActiveLinkView<'a>),
    Inactive,
    Absent,
}

impl<C: LinkTable> Links<C> {
    pub fn track_initiated(&mut self, link: InitiatedLink) -> Result<(), TrackLinkError> {
        if self.index_of(&link.link_id).is_some() {
            return Err(TrackLinkError::AlreadyTracked);
        }
        self.table.push(
            link.link_id,
            LinkPhase::Pending {
                destination: link.destination,
                initiator_secret: link.initiator_secret,
                link_signing: link.link_signing,
                requested_at: link.requested_at,
                command_id: link.command_id,
            },
            Some(link.timeout_at),
        )?;
        self.refresh_earliest_timeout();
        Ok(())
    }

    pub fn track_responding(&mut self, link: RespondingLink) -> Result<(), TrackLinkError> {
        if self.index_of(&link.link_id).is_some() {
            return Err(TrackLinkError::AlreadyTracked);
        }
        self.table.push(
            link.link_id,
            LinkPhase::Handshake {
                key: link.key,
                requested_at: link.requested_at,
                mtu: link.mtu,
                initiator_signing: link.initiator_signing,
                destination: link.destination,
                identity: link.identity,
                proof_strategy: link.proof_strategy,
            },
            Some(link.timeout_at),
        )?;
        self.refresh_earliest_timeout();
        Ok(())
    }

    pub fn link_count_via(&self, interface: InterfaceId) -> usize {
        self.table
            .phases()
            .iter()
            .filter(|phase| {
                matches!(
                    phase,
                    LinkPhase::Active {
                        attached_interface,
                        ..
                    } if *attached_interface == interface
                )
            })
            .count()
    }

    pub fn phase_for(&self, link_id: &LinkId) -> Option<&LinkPhase> {
        let index = self.index_of(link_id)?;
        self.table.phases().get(index)
    }

    /// [`phase_for`](Self::phase_for) narrowed to an active link's transport view, keeping "absent" and "present but inactive" apart. Borrows only the `links` column, so a caller can hold the view's `key` while mutating a sibling field like `outgoing_resources`.
    pub(crate) fn active_view(&self, link_id: &LinkId) -> ActiveLinkLookup<'_> {
        match self.phase_for(link_id) {
            None => ActiveLinkLookup::Absent,
            Some(LinkPhase::Active {
                key,
                mtu,
                attached_interface,
                rtt,
                ..
            }) => ActiveLinkLookup::Active(ActiveLinkView {
                key,
                mtu: *mtu,
                attached_interface: *attached_interface,
                rtt: *rtt,
            }),
            Some(_) => ActiveLinkLookup::Inactive,
        }
    }

    pub fn has_local_link(&self, link_id: &LinkId) -> bool {
        self.index_of(link_id).is_some()
    }

    pub fn activate_initiated(
        &mut self,
        link_id: &LinkId,
        key: LinkKey,
        activation: &LinkActivation,
        now: InstantMillis,
    ) -> Result<(), LinkActivationError> {
        let &LinkActivation {
            rtt,
            mtu,
            attached_interface,
            peer_signing,
        } = activation;
        let index = self
            .index_of(link_id)
            .ok_or(LinkActivationError::UnknownLink)?;
        let phase = self.table.phase_mut(index);
        // Take the phase by value so the arm can move its non-Copy secret (link_signing) into the new role; a &mut can't yield ownership of a field.
        // The vacant placeholder is transient: every arm writes *phase back before returning.
        match core::mem::replace(phase, LinkPhase::vacant()) {
            LinkPhase::Pending { link_signing, .. } => {
                let keepalive_ms = keepalive_ms_from(rtt);
                let role = LinkRole::Initiator { link_signing };
                let deadline = active_deadline(&role, now, now, keepalive_ms, rtt);
                *phase = LinkPhase::Active {
                    remote_identity: None,
                    resource_strategy: ResourceStrategy::default(),
                    last_resource_window: None,
                    last_resource_eifr: None,
                    key,
                    role,
                    rtt,
                    mtu,
                    attached_interface,
                    last_inbound: now,
                    last_keepalive_sent: now,
                    keepalive_ms,
                    peer_signing,
                };
                self.table.set_timeout_at(index, Some(deadline));
                self.refresh_earliest_timeout();
                Ok(())
            }
            other => {
                *phase = other;
                Err(LinkActivationError::WrongPhase)
            }
        }
    }

    pub fn activate_responding(
        &mut self,
        link_id: &LinkId,
        rtt: RttMillis,
        attached_interface: InterfaceId,
        now: InstantMillis,
    ) -> Result<DestinationHash, LinkActivationError> {
        let index = self
            .index_of(link_id)
            .ok_or(LinkActivationError::UnknownLink)?;
        let phase = self.table.phase_mut(index);
        // Take the phase by value as in activate_initiated: the Handshake's key/secrets move into the new role.
        match core::mem::replace(phase, LinkPhase::vacant()) {
            LinkPhase::Handshake {
                key,
                mtu,
                initiator_signing,
                destination,
                identity,
                proof_strategy,
                ..
            } => {
                let keepalive_ms = keepalive_ms_from(rtt);
                let role = LinkRole::Responder {
                    destination,
                    identity,
                    proof_strategy,
                };
                let deadline = active_deadline(&role, now, now, keepalive_ms, rtt);
                *phase = LinkPhase::Active {
                    remote_identity: None,
                    resource_strategy: ResourceStrategy::default(),
                    last_resource_window: None,
                    last_resource_eifr: None,
                    key,
                    role,
                    rtt,
                    mtu,
                    attached_interface,
                    last_inbound: now,
                    last_keepalive_sent: now,
                    keepalive_ms,
                    peer_signing: initiator_signing,
                };
                self.table.set_timeout_at(index, Some(deadline));
                self.refresh_earliest_timeout();
                Ok(destination)
            }
            other => {
                *phase = other;
                Err(LinkActivationError::WrongPhase)
            }
        }
    }

    /// RNS 1.3.5 `Link.set_resource_strategy`: how this link answers inbound resource advertisements from now on.
    pub fn set_resource_strategy(
        &mut self,
        link_id: &LinkId,
        strategy: ResourceStrategy,
    ) -> Result<(), LinkActivationError> {
        let index = self
            .index_of(link_id)
            .ok_or(LinkActivationError::UnknownLink)?;
        let LinkPhase::Active {
            resource_strategy, ..
        } = self.table.phase_mut(index)
        else {
            return Err(LinkActivationError::WrongPhase);
        };
        *resource_strategy = strategy;
        Ok(())
    }

    /// RNS 1.3.5 `Link.resource_concluded`'s memory. The window and expected in-flight rate an incoming transfer ended with, inherited by the next transfer this link accepts.
    pub fn note_resource_concluded(&mut self, link_id: &LinkId, window: usize, eifr: u64) {
        let Some(index) = self.index_of(link_id) else {
            return;
        };
        if let LinkPhase::Active {
            last_resource_window,
            last_resource_eifr,
            ..
        } = self.table.phase_mut(index)
        {
            *last_resource_window = Some(window);
            *last_resource_eifr = Some(eifr);
        }
    }

    pub fn note_identified(&mut self, link_id: &LinkId, identity: IdentityHash) {
        let Some(index) = self.index_of(link_id) else {
            return;
        };
        if let LinkPhase::Active {
            remote_identity, ..
        } = self.table.phase_mut(index)
        {
            *remote_identity = Some(identity);
        }
    }

    pub fn note_inbound(&mut self, link_id: &LinkId, now: InstantMillis) {
        let Some(index) = self.index_of(link_id) else {
            return;
        };
        if let LinkPhase::Active {
            role,
            rtt,
            last_inbound,
            last_keepalive_sent,
            keepalive_ms,
            ..
        } = self.table.phase_mut(index)
        {
            *last_inbound = now;
            let deadline = active_deadline(role, now, *last_keepalive_sent, *keepalive_ms, *rtt);
            self.table.set_timeout_at(index, Some(deadline));
        }
        self.refresh_earliest_timeout();
    }

    pub fn remove(&mut self, link_id: &LinkId) -> bool {
        match self.index_of(link_id) {
            Some(index) => {
                self.table.swap_remove(index);
                self.refresh_earliest_timeout();
                true
            }
            None => false,
        }
    }

    pub fn pop_stale(&mut self, now: InstantMillis) -> Option<LinkId> {
        let index = self
            .table
            .first_due_timeout_matching(now, |_, phase| match phase {
                LinkPhase::Active {
                    last_inbound,
                    keepalive_ms,
                    rtt,
                    ..
                } => now.0 >= teardown_at(*last_inbound, *keepalive_ms, *rtt),
                LinkPhase::Pending { .. } | LinkPhase::Handshake { .. } => false,
            })?;
        Some(self.table.link_ids()[index])
    }

    pub fn pop_due_keepalive(&mut self, now: InstantMillis) -> Option<DueKeepalive> {
        let index = self
            .table
            .first_due_timeout_matching(now, |_, phase| match phase {
                LinkPhase::Active {
                    role: LinkRole::Initiator { .. },
                    last_inbound,
                    last_keepalive_sent,
                    keepalive_ms,
                    ..
                } => {
                    let stale_at = last_inbound.0.saturating_add(stale_ms_from(*keepalive_ms));
                    let send_at = last_inbound
                        .0
                        .max(last_keepalive_sent.0)
                        .saturating_add(*keepalive_ms);
                    now.0 >= send_at && send_at <= stale_at
                }
                LinkPhase::Active {
                    role: LinkRole::Responder { .. },
                    ..
                }
                | LinkPhase::Pending { .. }
                | LinkPhase::Handshake { .. } => false,
            })?;
        let link_id = self.table.link_ids()[index];
        if let LinkPhase::Active {
            role: role @ LinkRole::Initiator { .. },
            attached_interface,
            rtt,
            last_inbound,
            last_keepalive_sent,
            keepalive_ms,
            ..
        } = self.table.phase_mut(index)
        {
            *last_keepalive_sent = now;
            let attached_interface = *attached_interface;
            let deadline = active_deadline(role, *last_inbound, now, *keepalive_ms, *rtt);
            self.table.set_timeout_at(index, Some(deadline));
            self.refresh_earliest_timeout();
            return Some(DueKeepalive {
                link_id,
                attached_interface,
            });
        }
        None
    }

    pub fn pop_overdue(&mut self, now: InstantMillis) -> Option<OverdueLink> {
        let index = self.table.first_due_timeout_matching(now, |_, phase| {
            matches!(
                phase,
                LinkPhase::Pending { .. } | LinkPhase::Handshake { .. }
            )
        })?;
        let link_id = self.table.link_ids()[index];
        let overdue = match &self.table.phases()[index] {
            LinkPhase::Pending {
                command_id,
                destination,
                ..
            } => OverdueLink::Initiated {
                link_id,
                command_id: *command_id,
                destination: *destination,
            },
            LinkPhase::Handshake { .. } => OverdueLink::Responding { link_id },
            LinkPhase::Active { .. } => return None,
        };
        self.table.swap_remove(index);
        self.refresh_earliest_timeout();
        Some(overdue)
    }

    fn refresh_earliest_timeout(&mut self) {
        self.earliest_timeout = self.table.earliest_indexed_timeout();
    }

    pub fn earliest_timeout_at(&self) -> Option<InstantMillis> {
        debug_assert_eq!(
            self.earliest_timeout,
            self.table.timeout_ats().iter().flatten().min().copied(),
            "earliest_timeout cache desynced from the timeout_ats column"
        );
        self.earliest_timeout
    }

    pub fn len(&self) -> usize {
        self.table.len()
    }

    pub fn is_empty(&self) -> bool {
        self.table.is_empty()
    }

    fn index_of(&self, link_id: &LinkId) -> Option<usize> {
        self.table.index_of(link_id)
    }
}

#[cfg(test)]
mod tests {
    use super::super::*;
    use super::*;
    use crate::crypto::{x25519_diffie_hellman, x25519_public_key};

    type TestLinks = Links<FixedLinkTable<4>>;

    fn link_id(byte: u8) -> LinkId {
        LinkId::new([byte; 16])
    }

    fn dest(byte: u8) -> DestinationHash {
        DestinationHash::new([byte; 16])
    }

    fn secret(byte: u8) -> X25519SecretKey {
        X25519SecretKey::new([byte; 32])
    }

    fn key(id: u8, scalar: u8) -> LinkKey {
        let shared = x25519_diffie_hellman(
            &secret(scalar),
            &x25519_public_key(&secret(scalar.wrapping_add(1))),
        );
        LinkKey::derive(&link_id(id), &shared)
    }

    fn initiated(id: u8, timeout_at: u64) -> InitiatedLink {
        InitiatedLink {
            link_id: link_id(id),
            destination: dest(id),
            initiator_secret: secret(id),
            link_signing: Ed25519SecretKey::new([id; 32]),
            requested_at: InstantMillis(1_000),
            timeout_at: InstantMillis(timeout_at),
            command_id: CommandId(u64::from(id)),
        }
    }

    fn responding(id: u8, timeout_at: u64) -> RespondingLink {
        RespondingLink {
            link_id: link_id(id),
            key: key(id, id),
            requested_at: InstantMillis(1_000),
            timeout_at: InstantMillis(timeout_at),
            mtu: 500,
            initiator_signing: Ed25519PublicKey([id; 32]),
            destination: dest(id),
            identity: IdentityHash::new([id; 16]),
            proof_strategy: ProofStrategy::ProveNone,
        }
    }

    fn iface(byte: u8) -> InterfaceId {
        InterfaceId::new([byte; 8])
    }

    #[test]
    fn a_tracked_initiation_holds_its_request_until_the_proof_arrives() {
        let mut links = TestLinks::default();
        links.track_initiated(initiated(1, 5_000)).unwrap();

        let Some(LinkPhase::Pending {
            destination,
            requested_at,
            ..
        }) = links.phase_for(&link_id(1))
        else {
            panic!("a tracked initiation must be pending");
        };
        assert_eq!(*destination, dest(1));
        assert_eq!(*requested_at, InstantMillis(1_000));
        assert!(links.phase_for(&link_id(2)).is_none());
    }

    #[test]
    fn validating_the_proof_activates_an_initiated_link() {
        let mut links = TestLinks::default();
        links.track_initiated(initiated(1, 5_000)).unwrap();

        links
            .activate_initiated(
                &link_id(1),
                key(1, 9),
                &LinkActivation {
                    rtt: RttMillis::new(250),
                    mtu: 500,
                    attached_interface: iface(0xEE),
                    peer_signing: Ed25519PublicKey([0x99; 32]),
                },
                InstantMillis(2_000),
            )
            .unwrap();

        let Some(LinkPhase::Active {
            role: LinkRole::Initiator { .. },
            rtt,
            ..
        }) = links.phase_for(&link_id(1))
        else {
            panic!("a proven link must be active as initiator");
        };
        assert_eq!(*rtt, RttMillis::new(250));
        assert_eq!(
            links.earliest_timeout_at(),
            Some(InstantMillis(2_000 + 51_428)),
            "activation arms the initiator's keepalive deadline from its rtt",
        );
    }

    #[test]
    fn the_rtt_packet_activates_a_responding_link_with_its_handshake_key() {
        let mut links = TestLinks::default();
        links.track_responding(responding(2, 5_000)).unwrap();

        links
            .activate_responding(
                &link_id(2),
                RttMillis::new(500),
                iface(0xEE),
                InstantMillis(2_000),
            )
            .unwrap();

        let Some(LinkPhase::Active {
            key: stored,
            role: LinkRole::Responder { .. },
            rtt,
            ..
        }) = links.phase_for(&link_id(2))
        else {
            panic!("a responding link with its rtt must be active as responder");
        };
        assert_eq!(*rtt, RttMillis::new(500));

        let iv = [0xA5u8; 16];
        let mut via_table = [0u8; 96];
        let mut via_rederivation = [0u8; 96];
        let n = stored.seal(&iv, b"same key", &mut via_table).unwrap();
        let m = key(2, 2)
            .seal(&iv, b"same key", &mut via_rederivation)
            .unwrap();
        assert_eq!(
            &via_table[..n],
            &via_rederivation[..m],
            "the handshake key must survive activation",
        );
    }

    #[test]
    fn link_count_via_reports_only_active_links_attached_to_that_interface() {
        let mut links = TestLinks::default();

        links.track_initiated(initiated(1, 5_000)).unwrap();
        links
            .activate_initiated(
                &link_id(1),
                key(1, 9),
                &LinkActivation {
                    rtt: RttMillis::new(250),
                    mtu: 500,
                    attached_interface: iface(0xAA),
                    peer_signing: Ed25519PublicKey([0x99; 32]),
                },
                InstantMillis(2_000),
            )
            .unwrap();

        links.track_responding(responding(2, 5_000)).unwrap();
        links
            .activate_responding(
                &link_id(2),
                RttMillis::new(120),
                iface(0xAA),
                InstantMillis(2_000),
            )
            .unwrap();

        links.track_initiated(initiated(3, 5_000)).unwrap();
        links
            .activate_initiated(
                &link_id(3),
                key(3, 9),
                &LinkActivation {
                    rtt: RttMillis::new(250),
                    mtu: 500,
                    attached_interface: iface(0xBB),
                    peer_signing: Ed25519PublicKey([0x99; 32]),
                },
                InstantMillis(2_000),
            )
            .unwrap();

        links.track_initiated(initiated(4, 5_000)).unwrap();

        assert_eq!(
            links.link_count_via(iface(0xAA)),
            2,
            "both an initiator and a responder link attach here; the still-pending one is not live",
        );
        assert_eq!(links.link_count_via(iface(0xBB)), 1);
        assert_eq!(
            links.link_count_via(iface(0xCC)),
            0,
            "an interface holding no live link reads zero",
        );

        assert!(links.remove(&link_id(1)));
        assert_eq!(
            links.link_count_via(iface(0xAA)),
            1,
            "tearing a live link down drops its interface's count",
        );
    }

    #[test]
    fn activation_demands_the_matching_phase() {
        let mut links = TestLinks::default();
        assert_eq!(
            links.activate_initiated(
                &link_id(9),
                key(9, 9),
                &LinkActivation {
                    rtt: RttMillis::new(100),
                    mtu: 500,
                    attached_interface: iface(0xEE),
                    peer_signing: Ed25519PublicKey([0x99; 32]),
                },
                InstantMillis(2_000),
            ),
            Err(LinkActivationError::UnknownLink),
        );
        assert_eq!(
            links.activate_responding(
                &link_id(9),
                RttMillis::new(100),
                iface(0xEE),
                InstantMillis(2_000)
            ),
            Err(LinkActivationError::UnknownLink),
        );

        links.track_initiated(initiated(1, 5_000)).unwrap();
        links.track_responding(responding(2, 5_000)).unwrap();

        assert_eq!(
            links.activate_responding(
                &link_id(1),
                RttMillis::new(100),
                iface(0xEE),
                InstantMillis(2_000)
            ),
            Err(LinkActivationError::WrongPhase),
        );
        assert!(matches!(
            links.phase_for(&link_id(1)),
            Some(LinkPhase::Pending { .. }),
        ));
        assert_eq!(
            links.activate_initiated(
                &link_id(2),
                key(2, 9),
                &LinkActivation {
                    rtt: RttMillis::new(100),
                    mtu: 500,
                    attached_interface: iface(0xEE),
                    peer_signing: Ed25519PublicKey([0x99; 32]),
                },
                InstantMillis(2_000),
            ),
            Err(LinkActivationError::WrongPhase),
        );

        links
            .activate_initiated(
                &link_id(1),
                key(1, 9),
                &LinkActivation {
                    rtt: RttMillis::new(100),
                    mtu: 500,
                    attached_interface: iface(0xEE),
                    peer_signing: Ed25519PublicKey([0x99; 32]),
                },
                InstantMillis(2_000),
            )
            .unwrap();
        assert_eq!(
            links.activate_initiated(
                &link_id(1),
                key(1, 9),
                &LinkActivation {
                    rtt: RttMillis::new(100),
                    mtu: 500,
                    attached_interface: iface(0xEE),
                    peer_signing: Ed25519PublicKey([0x99; 32]),
                },
                InstantMillis(2_000),
            ),
            Err(LinkActivationError::WrongPhase),
        );
    }

    #[test]
    fn a_duplicate_link_id_is_refused() {
        let mut links = TestLinks::default();
        links.track_initiated(initiated(1, 5_000)).unwrap();

        assert_eq!(
            links.track_initiated(initiated(1, 9_000)),
            Err(TrackLinkError::AlreadyTracked),
        );
        assert_eq!(
            links.track_responding(responding(1, 9_000)),
            Err(TrackLinkError::AlreadyTracked),
        );
        assert_eq!(links.len(), 1);
    }

    #[test]
    fn a_full_table_refuses_new_links() {
        let mut links = Links::<FixedLinkTable<2>>::default();
        links.track_initiated(initiated(1, 5_000)).unwrap();
        links.track_responding(responding(2, 5_000)).unwrap();

        assert_eq!(
            links.track_initiated(initiated(3, 5_000)),
            Err(TrackLinkError::TableFull),
        );
        assert_eq!(links.len(), 2);
        assert!(links.phase_for(&link_id(1)).is_some());
        assert!(links.phase_for(&link_id(2)).is_some());
    }

    #[test]
    fn overdue_establishments_pop_with_their_shapes() {
        let mut links = TestLinks::default();
        links.track_initiated(initiated(1, 5_000)).unwrap();
        links.track_responding(responding(2, 3_000)).unwrap();
        links.track_initiated(initiated(3, 9_000)).unwrap();
        links
            .activate_initiated(
                &link_id(3),
                key(3, 9),
                &LinkActivation {
                    rtt: RttMillis::new(100),
                    mtu: 500,
                    attached_interface: iface(0xEE),
                    peer_signing: Ed25519PublicKey([0x99; 32]),
                },
                InstantMillis(2_000),
            )
            .unwrap();

        assert_eq!(links.pop_overdue(InstantMillis(2_999)), None);

        let popped = [
            links.pop_overdue(InstantMillis(5_000)).unwrap(),
            links.pop_overdue(InstantMillis(5_000)).unwrap(),
        ];
        assert_eq!(links.pop_overdue(InstantMillis(5_000)), None);

        assert!(popped.contains(&OverdueLink::Initiated {
            link_id: link_id(1),
            command_id: CommandId(1),
            destination: dest(1),
        }));
        assert!(popped.contains(&OverdueLink::Responding {
            link_id: link_id(2),
        }));
        assert_eq!(links.len(), 1, "the active link never times out here");
    }

    #[test]
    fn the_earliest_establishment_deadline_drives_the_wakeup() {
        let mut links = TestLinks::default();
        assert_eq!(links.earliest_timeout_at(), None);

        links.track_initiated(initiated(1, 5_000)).unwrap();
        links.track_initiated(initiated(2, 3_000)).unwrap();
        assert_eq!(links.earliest_timeout_at(), Some(InstantMillis(3_000)));

        links
            .activate_initiated(
                &link_id(2),
                key(2, 9),
                &LinkActivation {
                    rtt: RttMillis::new(100),
                    mtu: 500,
                    attached_interface: iface(0xEE),
                    peer_signing: Ed25519PublicKey([0x99; 32]),
                },
                InstantMillis(2_000),
            )
            .unwrap();
        assert_eq!(links.earliest_timeout_at(), Some(InstantMillis(5_000)));

        links
            .activate_initiated(
                &link_id(1),
                key(1, 9),
                &LinkActivation {
                    rtt: RttMillis::new(100),
                    mtu: 500,
                    attached_interface: iface(0xEE),
                    peer_signing: Ed25519PublicKey([0x99; 32]),
                },
                InstantMillis(2_000),
            )
            .unwrap();
        assert_eq!(
            links.earliest_timeout_at(),
            Some(InstantMillis(2_000 + 20_571)),
            "active links keep a maintenance deadline armed",
        );
    }

    #[test]
    fn heap_columns_track_past_any_fixed_ceiling() {
        let mut links = Links::<HeapLinkTable>::default();
        for byte in 0..8u8 {
            links.track_initiated(initiated(byte, 5_000)).unwrap();
        }
        assert_eq!(links.len(), 8);
        assert!(links.phase_for(&link_id(5)).is_some());
    }

    fn active_initiator(links: &mut TestLinks, id: u8, rtt: RttMillis, now: u64) {
        links.track_initiated(initiated(id, 5_000)).unwrap();
        links
            .activate_initiated(
                &link_id(id),
                key(id, 9),
                &LinkActivation {
                    rtt,
                    mtu: 500,
                    attached_interface: iface(0xEE),
                    peer_signing: Ed25519PublicKey([0x99; 32]),
                },
                InstantMillis(now),
            )
            .unwrap();
    }

    #[test]
    fn a_quiet_initiator_waits_out_the_grace_before_teardown() {
        let mut links = TestLinks::default();
        active_initiator(&mut links, 1, RttMillis::new(250), 2_000);

        assert_eq!(
            links.pop_stale(InstantMillis(2_000 + 102_856)),
            None,
            "reaching the stale boundary must not tear the link down — RNS waits the grace",
        );
        assert_eq!(
            links.pop_stale(InstantMillis(2_000 + 102_856 + 5_999)),
            None,
            "nor anywhere inside the grace window",
        );
        assert_eq!(
            links.pop_stale(InstantMillis(2_000 + 102_856 + 6_000)),
            Some(link_id(1)),
            "teardown fires only after rtt*4 + STALE_GRACE past the stale boundary",
        );
    }

    #[test]
    fn a_quiet_responder_also_waits_out_the_grace() {
        let mut links = TestLinks::default();
        links.track_responding(responding(2, 5_000)).unwrap();
        links
            .activate_responding(
                &link_id(2),
                RttMillis::new(500),
                iface(0xEE),
                InstantMillis(2_000),
            )
            .unwrap();

        assert_eq!(
            links.earliest_timeout_at(),
            Some(InstantMillis(2_000 + 205_714 + 7_000)),
            "a responder sends no keepalive of its own, so its only deadline is teardown at stale + grace",
        );
        assert_eq!(links.pop_stale(InstantMillis(2_000 + 205_714)), None);
        assert_eq!(
            links.pop_stale(InstantMillis(2_000 + 205_714 + 7_000)),
            Some(link_id(2)),
        );
    }

    #[test]
    fn an_inbound_inside_the_grace_revives_the_link() {
        let mut links = TestLinks::default();
        active_initiator(&mut links, 1, RttMillis::new(250), 2_000);

        links.note_inbound(&link_id(1), InstantMillis(105_000));

        assert_eq!(
            links.pop_stale(InstantMillis(110_856)),
            None,
            "an inbound inside the grace window revives the link past the old teardown moment",
        );
        assert_eq!(
            links.earliest_timeout_at(),
            Some(InstantMillis(105_000 + 51_428)),
            "the revived link rearms its keepalive cadence from the fresh inbound",
        );
    }

    #[test]
    fn the_final_keepalive_lands_at_the_stale_boundary_and_none_during_grace() {
        let mut links = TestLinks::default();
        active_initiator(&mut links, 1, RttMillis::new(10), 2_000);

        let due = DueKeepalive {
            link_id: link_id(1),
            attached_interface: iface(0xEE),
        };
        assert_eq!(links.pop_due_keepalive(InstantMillis(7_000)), Some(due));
        assert_eq!(
            links.pop_due_keepalive(InstantMillis(12_000)),
            Some(due),
            "the initiator sends one last keepalive exactly at the stale boundary",
        );
        assert_eq!(
            links.pop_due_keepalive(InstantMillis(17_000)),
            None,
            "no keepalive leaves during the grace window even when its cadence comes due",
        );
        assert_eq!(
            links.pop_stale(InstantMillis(17_040)),
            Some(link_id(1)),
            "teardown still waits the full rtt*4 + STALE_GRACE grace",
        );
    }
}
