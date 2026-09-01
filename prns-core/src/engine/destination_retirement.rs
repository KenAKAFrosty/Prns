use crate::engine::{
    Directive, EngineReaction, EngineState, LinkClosedReason, UnregisterDestinationOutcome,
    WriteLinkCloseError,
};
use crate::identity::ENCRYPTION_IV_LEN;
use crate::interfaces::{AttachedInterfaces, Egress};
use crate::routing::links::table::LinkPhase;
use crate::routing::links::LinkId;
use crate::routing::upstream_app_destinations::UpstreamAppDestination;
use crate::storage::StorageLayout;
use crate::units::LinkCount;
use crate::wire::{DestinationHash, BROADCAST_MTU};

#[derive(Debug, PartialEq, Eq)]
#[must_use]
pub enum RetireDestinationOutcome {
    Retired {
        registration: UpstreamAppDestination,
        retired_links: LinkCount,
    },
    RetirementIncomplete {
        first_remaining_link: LinkId,
        retired_links: LinkCount,
    },
    NotRegistered,
}

impl<S: StorageLayout> EngineState<S> {
    pub fn retire_destination<F>(
        &mut self,
        destination: &DestinationHash,
        interfaces: AttachedInterfaces<'_>,
        fill_random: &mut F,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) -> RetireDestinationOutcome
    where
        F: FnMut(&mut [u8]),
    {
        if self
            .upstream_app_destinations()
            .all(|registration| registration.destination != *destination)
        {
            return RetireDestinationOutcome::NotRegistered;
        }

        let maximum_retirements = self.links.len();
        let mut retired_links = 0usize;
        for _ in 0..maximum_retirements {
            let Some(link_id) = self.responder_links_for_destination(destination).next() else {
                break;
            };

            let active = matches!(
                self.links.phase_for(&link_id),
                Some(LinkPhase::Active { .. })
            );
            if active {
                let mut iv = [0u8; ENCRYPTION_IV_LEN];
                fill_random(&mut iv);
                let mut buf = [0u8; BROADCAST_MTU];
                match self.write_owed_link_close(
                    &link_id,
                    LinkClosedReason::LocallyClosed,
                    &iv,
                    &mut buf,
                    sink,
                ) {
                    Ok(dispatch) => {
                        if let Some(target) = dispatch.fire_on {
                            if interfaces.is_egress_eligible(target, Egress::Transmit) {
                                sink(EngineReaction::Directive(Directive::Send {
                                    target,
                                    bytes: &buf[..dispatch.wire_bytes],
                                }));
                            }
                        }
                    }
                    Err(WriteLinkCloseError::NoSuchLink | WriteLinkCloseError::Serialize) => {}
                }
            } else {
                self.retire_link(&link_id, LinkClosedReason::LocallyClosed, sink);
            }
            retired_links += 1;
        }

        match self.unregister_destination(destination) {
            UnregisterDestinationOutcome::Unregistered { registration } => {
                RetireDestinationOutcome::Retired {
                    registration,
                    retired_links: LinkCount(retired_links),
                }
            }
            UnregisterDestinationOutcome::ResponderLinksPresent { first_link } => {
                RetireDestinationOutcome::RetirementIncomplete {
                    first_remaining_link: first_link,
                    retired_links: LinkCount(retired_links),
                }
            }
            UnregisterDestinationOutcome::NotRegistered => RetireDestinationOutcome::NotRegistered,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::ratchets::RatchetPolicy;
    use crate::crypto::{
        x25519_diffie_hellman, x25519_public_key, Ed25519PublicKey, X25519SecretKey,
    };
    use crate::engine::test_support::{fixed_secret_key, routable_descriptor, TestStorageLayout};
    use crate::engine::InstantMillis;
    use crate::interfaces::InterfaceId;
    use crate::routing::links::table::RespondingLink;
    use crate::routing::links::{LinkId, LinkKey};
    use crate::routing::upstream_app_destinations::{LinkRequestPolicy, ProofStrategy};
    use crate::wire::{WireContext, WirePacketHeader};

    #[test]
    fn destination_retirement_closes_every_responder_link_then_unregisters() {
        let mut state = EngineState::<TestStorageLayout>::default();
        let identity = state.hold_identity(fixed_secret_key()).unwrap();
        let destination = state
            .register_single_destination(
                &identity,
                "personal",
                &["pairing"],
                b"pair",
                ProofStrategy::ProveAll,
                LinkRequestPolicy::AcceptAll,
                RatchetPolicy::Ratcheted,
            )
            .unwrap();
        let registration = state
            .upstream_app_destinations()
            .find(|registration| registration.destination == destination)
            .unwrap();
        let local_secret = X25519SecretKey::new([0x61; 32]);
        let remote_secret = X25519SecretKey::new([0x62; 32]);
        let shared = x25519_diffie_hellman(&local_secret, &x25519_public_key(&remote_secret));
        let handshake = LinkId::new([0x63; 16]);
        let active = LinkId::new([0x64; 16]);
        for link_id in [handshake, active] {
            state
                .links
                .track_responding(RespondingLink {
                    link_id,
                    key: LinkKey::derive(&link_id, &shared),
                    requested_at: InstantMillis(1_000),
                    timeout_at: InstantMillis(2_000),
                    mtu: BROADCAST_MTU,
                    initiator_signing: Ed25519PublicKey([0x65; 32]),
                    destination,
                    identity,
                    proof_strategy: ProofStrategy::ProveAll,
                })
                .unwrap();
        }
        let interface = InterfaceId::new([0x66; 8]);
        state
            .links
            .activate_responding(
                &active,
                crate::units::RttMillis::new(25),
                interface,
                InstantMillis(1_100),
            )
            .unwrap();

        let descriptors = [routable_descriptor(interface)];
        let mut entropy_draws = 0;
        let mut close_frames = 0;
        let mut closed = std::vec::Vec::new();
        let outcome = state.retire_destination(
            &destination,
            AttachedInterfaces::new(&descriptors),
            &mut |bytes| {
                entropy_draws += 1;
                bytes.fill(0x67);
            },
            &mut |reaction| match reaction {
                EngineReaction::Directive(Directive::Send { target, bytes }) => {
                    assert_eq!(target, interface);
                    let (header, _) = WirePacketHeader::parse(bytes).unwrap();
                    assert_eq!(header.context, WireContext::LinkClose);
                    close_frames += 1;
                }
                EngineReaction::Journaled(crate::engine::Journaled::LinkClosed {
                    link_id,
                    reason,
                }) => {
                    closed.push((link_id, reason));
                }
                EngineReaction::Journaled(_) | EngineReaction::Directive(_) => {}
            },
        );

        assert_eq!(
            outcome,
            RetireDestinationOutcome::Retired {
                registration,
                retired_links: LinkCount(2),
            },
        );
        assert_eq!(entropy_draws, 1);
        assert_eq!(close_frames, 1);
        assert_eq!(
            closed,
            [
                (handshake, LinkClosedReason::LocallyClosed),
                (active, LinkClosedReason::LocallyClosed),
            ],
        );
        assert!(state.upstream_app_destinations().next().is_none());
        assert!(state
            .responder_links_for_destination(&destination)
            .next()
            .is_none());
        assert!(!state.self_ratchets.is_tracked(&destination));
    }

    #[test]
    fn retiring_an_unknown_destination_is_inert() {
        let mut state = EngineState::<TestStorageLayout>::default();
        let mut entropy_drawn = false;
        let mut reaction_emitted = false;
        assert_eq!(
            state.retire_destination(
                &DestinationHash::new([0x71; 16]),
                AttachedInterfaces::new(&[]),
                &mut |_| entropy_drawn = true,
                &mut |_| reaction_emitted = true,
            ),
            RetireDestinationOutcome::NotRegistered,
        );
        assert!(!entropy_drawn);
        assert!(!reaction_emitted);
    }
}
