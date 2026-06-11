//! The per-link state a node holds from LINKREQUEST to ACTIVE (RNS 1.3.1
//! `Link.status`): the initiator's pending establishments, the responder's
//! handshakes awaiting an RTT, and the active sessions both settle into.

mod impls;

pub use impls::*;

use crate::crypto::X25519SecretKey;
use crate::engine::commands::CommandId;
use crate::engine::InstantMillis;
use crate::interfaces::InterfaceId;
use crate::routing::links::{LinkId, LinkKey};
use crate::wire::DestinationHash;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkRole {
    Initiator,
    Responder,
}

pub enum LinkPhase {
    Pending {
        destination: DestinationHash,
        initiator_secret: X25519SecretKey,
        requested_at: InstantMillis,
        command_id: CommandId,
    },
    Handshake {
        key: LinkKey,
        requested_at: InstantMillis,
        mtu: usize,
    },
    Active {
        key: LinkKey,
        role: LinkRole,
        rtt_ms: u64,
        mtu: usize,
        attached_interface: InterfaceId,
    },
}

impl LinkPhase {
    pub fn vacant() -> Self {
        Self::Pending {
            destination: DestinationHash::new([0u8; 16]),
            initiator_secret: X25519SecretKey::new([0u8; 32]),
            requested_at: InstantMillis(0),
            command_id: CommandId(0),
        }
    }
}

// The Pending phase holds the initiator secret, so Debug can't be derived —
// X25519SecretKey deliberately has no Debug to leak. The manual impl prints
// around it, and the key fields go through LinkKey's redacted Debug. Wiping
// is the field types' own job: both zeroize on drop, wherever a phase dies.
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
            } => f
                .debug_struct("Handshake")
                .field("key", key)
                .field("requested_at", requested_at)
                .field("mtu", mtu)
                .finish(),
            Self::Active {
                key,
                role,
                rtt_ms,
                mtu,
                attached_interface,
            } => f
                .debug_struct("Active")
                .field("key", key)
                .field("role", role)
                .field("rtt_ms", rtt_ms)
                .field("mtu", mtu)
                .field("attached_interface", attached_interface)
                .finish(),
        }
    }
}

pub struct InitiatedLink {
    pub link_id: LinkId,
    pub destination: DestinationHash,
    pub initiator_secret: X25519SecretKey,
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverdueLink {
    Initiated {
        link_id: LinkId,
        command_id: CommandId,
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
pub enum LinkActivationError {
    UnknownLink,
    WrongPhase,
}

pub trait LinkColumns {
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
}

#[derive(Debug, Default)]
pub struct Links<C: LinkColumns> {
    columns: C,
}

impl<C: LinkColumns> Links<C> {
    pub fn track_initiated(&mut self, link: InitiatedLink) -> Result<(), TrackLinkError> {
        if self.index_of(&link.link_id).is_some() {
            return Err(TrackLinkError::AlreadyTracked);
        }
        self.columns.push(
            link.link_id,
            LinkPhase::Pending {
                destination: link.destination,
                initiator_secret: link.initiator_secret,
                requested_at: link.requested_at,
                command_id: link.command_id,
            },
            Some(link.timeout_at),
        )?;
        Ok(())
    }

    pub fn track_responding(&mut self, link: RespondingLink) -> Result<(), TrackLinkError> {
        if self.index_of(&link.link_id).is_some() {
            return Err(TrackLinkError::AlreadyTracked);
        }
        self.columns.push(
            link.link_id,
            LinkPhase::Handshake {
                key: link.key,
                requested_at: link.requested_at,
                mtu: link.mtu,
            },
            Some(link.timeout_at),
        )?;
        Ok(())
    }

    pub fn phase_for(&self, link_id: &LinkId) -> Option<&LinkPhase> {
        let index = self.index_of(link_id)?;
        self.columns.phases().get(index)
    }

    pub fn activate_initiated(
        &mut self,
        link_id: &LinkId,
        key: LinkKey,
        rtt_ms: u64,
        mtu: usize,
        attached_interface: InterfaceId,
    ) -> Result<(), LinkActivationError> {
        let index = self
            .index_of(link_id)
            .ok_or(LinkActivationError::UnknownLink)?;
        if !matches!(&self.columns.phases()[index], LinkPhase::Pending { .. }) {
            return Err(LinkActivationError::WrongPhase);
        }
        *self.columns.phase_mut(index) = LinkPhase::Active {
            key,
            role: LinkRole::Initiator,
            rtt_ms,
            mtu,
            attached_interface,
        };
        self.columns.set_timeout_at(index, None);
        Ok(())
    }

    pub fn activate_responding(
        &mut self,
        link_id: &LinkId,
        rtt_ms: u64,
        attached_interface: InterfaceId,
    ) -> Result<(), LinkActivationError> {
        let index = self
            .index_of(link_id)
            .ok_or(LinkActivationError::UnknownLink)?;
        let phase = self.columns.phase_mut(index);
        match core::mem::replace(phase, LinkPhase::vacant()) {
            LinkPhase::Handshake { key, mtu, .. } => {
                *phase = LinkPhase::Active {
                    key,
                    role: LinkRole::Responder,
                    rtt_ms,
                    mtu,
                    attached_interface,
                };
                self.columns.set_timeout_at(index, None);
                Ok(())
            }
            other => {
                *phase = other;
                Err(LinkActivationError::WrongPhase)
            }
        }
    }

    pub fn pop_overdue(&mut self, now: InstantMillis) -> Option<OverdueLink> {
        let index = self
            .columns
            .timeout_ats()
            .iter()
            .position(|timeout_at| timeout_at.is_some_and(|at| at <= now))?;
        let link_id = self.columns.link_ids()[index];
        let overdue = match &self.columns.phases()[index] {
            LinkPhase::Pending { command_id, .. } => OverdueLink::Initiated {
                link_id,
                command_id: *command_id,
            },
            LinkPhase::Handshake { .. } | LinkPhase::Active { .. } => {
                OverdueLink::Responding { link_id }
            }
        };
        self.columns.swap_remove(index);
        Some(overdue)
    }

    pub fn earliest_timeout_at(&self) -> Option<InstantMillis> {
        self.columns.timeout_ats().iter().flatten().min().copied()
    }

    pub fn len(&self) -> usize {
        self.columns.len()
    }

    pub fn is_empty(&self) -> bool {
        self.columns.is_empty()
    }

    fn index_of(&self, link_id: &LinkId) -> Option<usize> {
        self.columns
            .link_ids()
            .iter()
            .position(|candidate| candidate == link_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::{x25519_diffie_hellman, x25519_public_key};

    type TestLinks = Links<FixedLinkColumns<4>>;

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
        }
    }

    fn iface(byte: u8) -> InterfaceId {
        InterfaceId::new([byte; 16])
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
            .activate_initiated(&link_id(1), key(1, 9), 250, 500, iface(0xEE))
            .unwrap();

        let Some(LinkPhase::Active {
            role: LinkRole::Initiator,
            rtt_ms,
            ..
        }) = links.phase_for(&link_id(1))
        else {
            panic!("a proven link must be active as initiator");
        };
        assert_eq!(*rtt_ms, 250);
        assert_eq!(links.earliest_timeout_at(), None);
    }

    #[test]
    fn the_rtt_packet_activates_a_responding_link_with_its_handshake_key() {
        let mut links = TestLinks::default();
        links.track_responding(responding(2, 5_000)).unwrap();

        links
            .activate_responding(&link_id(2), 500, iface(0xEE))
            .unwrap();

        let Some(LinkPhase::Active {
            key: stored,
            role: LinkRole::Responder,
            rtt_ms,
            ..
        }) = links.phase_for(&link_id(2))
        else {
            panic!("a responding link with its rtt must be active as responder");
        };
        assert_eq!(*rtt_ms, 500);

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
    fn activation_demands_the_matching_phase() {
        let mut links = TestLinks::default();
        assert_eq!(
            links.activate_initiated(&link_id(9), key(9, 9), 100, 500, iface(0xEE)),
            Err(LinkActivationError::UnknownLink),
        );
        assert_eq!(
            links.activate_responding(&link_id(9), 100, iface(0xEE)),
            Err(LinkActivationError::UnknownLink),
        );

        links.track_initiated(initiated(1, 5_000)).unwrap();
        links.track_responding(responding(2, 5_000)).unwrap();

        assert_eq!(
            links.activate_responding(&link_id(1), 100, iface(0xEE)),
            Err(LinkActivationError::WrongPhase),
        );
        assert!(matches!(
            links.phase_for(&link_id(1)),
            Some(LinkPhase::Pending { .. }),
        ));
        assert_eq!(
            links.activate_initiated(&link_id(2), key(2, 9), 100, 500, iface(0xEE)),
            Err(LinkActivationError::WrongPhase),
        );

        links
            .activate_initiated(&link_id(1), key(1, 9), 100, 500, iface(0xEE))
            .unwrap();
        assert_eq!(
            links.activate_initiated(&link_id(1), key(1, 9), 100, 500, iface(0xEE)),
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
        let mut links = Links::<FixedLinkColumns<2>>::default();
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
            .activate_initiated(&link_id(3), key(3, 9), 100, 500, iface(0xEE))
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
            .activate_initiated(&link_id(2), key(2, 9), 100, 500, iface(0xEE))
            .unwrap();
        assert_eq!(links.earliest_timeout_at(), Some(InstantMillis(5_000)));

        links
            .activate_initiated(&link_id(1), key(1, 9), 100, 500, iface(0xEE))
            .unwrap();
        assert_eq!(links.earliest_timeout_at(), None);
    }

    #[test]
    fn heap_columns_track_past_any_fixed_ceiling() {
        let mut links = Links::<HeapLinkColumns>::default();
        for byte in 0..8u8 {
            links.track_initiated(initiated(byte, 5_000)).unwrap();
        }
        assert_eq!(links.len(), 8);
        assert!(links.phase_for(&link_id(5)).is_some());
    }
}
