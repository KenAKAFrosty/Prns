use crate::engine::{Directive, EngineReaction, EngineState, Journaled};
use crate::interfaces::{AttachedInterfaces, Egress, InterfaceId};
use crate::routing::delivery::Delivery;
use crate::routing::proof::{
    DeferredProofSign, LinkProofOwed, ProofObligation, ProofOwed, ProofRequest, LINK_PROOF_WIRE_LEN,
};
use crate::storage::StorageLayout;

pub(super) struct DeliveryIo<'a, P, K>
where
    P: FnMut(&ProofRequest) -> bool,
    K: FnMut(EngineReaction<'_>),
{
    pub(super) interfaces: AttachedInterfaces<'a>,
    pub(super) should_prove: &'a mut P,
    pub(super) deferred_sign: &'a mut Option<DeferredProofSign>,
    pub(super) sink: &'a mut K,
}

enum ResolvedProof {
    Withheld,
    Implicit(ProofOwed),
    OverLink(LinkProofOwed),
}

impl<S: StorageLayout> EngineState<S> {
    pub(super) fn process_delivery<'d, P, K>(
        &mut self,
        delivery: Delivery<'d>,
        proof: ProofObligation,
        source: InterfaceId,
        io: &mut DeliveryIo<'_, P, K>,
    ) where
        P: FnMut(&ProofRequest) -> bool,
        K: FnMut(EngineReaction<'_>),
    {
        (io.sink)(EngineReaction::Journaled(Journaled::Delivered(delivery)));
        let resolved = match proof {
            ProofObligation::None => ResolvedProof::Withheld,
            ProofObligation::Owed(owed) => ResolvedProof::Implicit(owed),
            ProofObligation::OwedIfApp(owed) => match delivery {
                Delivery::Single(single) => {
                    if (io.should_prove)(&ProofRequest {
                        destination: single.destination,
                        plaintext: single.plaintext,
                    }) {
                        ResolvedProof::Implicit(owed)
                    } else {
                        ResolvedProof::Withheld
                    }
                }
                Delivery::Plain(_) | Delivery::Group(_) | Delivery::Link(_) => {
                    ResolvedProof::Withheld
                }
            },
            ProofObligation::OwedOverLink(owed) => ResolvedProof::OverLink(owed),
            ProofObligation::OwedIfAppOverLink(owed) => match delivery {
                Delivery::Link(link) => {
                    if (io.should_prove)(&ProofRequest {
                        destination: owed.destination,
                        plaintext: link.plaintext,
                    }) {
                        ResolvedProof::OverLink(owed)
                    } else {
                        ResolvedProof::Withheld
                    }
                }
                Delivery::Plain(_) | Delivery::Single(_) | Delivery::Group(_) => {
                    ResolvedProof::Withheld
                }
            },
        };
        match resolved {
            ResolvedProof::Withheld => {}
            ResolvedProof::Implicit(owed) => {
                if io.interfaces.is_egress_eligible(source, Egress::Transmit) {
                    if let Some(signing_secret) = self
                        .held_identities
                        .get(&owed.identity)
                        .map(|held| held.signing_secret_clone())
                    {
                        *io.deferred_sign = Some(DeferredProofSign {
                            target: source,
                            packet_hash: owed.packet_hash,
                            signing_secret,
                        });
                    }
                }
            }
            ResolvedProof::OverLink(owed) => {
                if io.interfaces.is_egress_eligible(source, Egress::Transmit) {
                    let mut proof = [0u8; LINK_PROOF_WIRE_LEN];
                    if let Ok(written) = self.write_link_proof(&owed, &mut proof) {
                        (io.sink)(EngineReaction::Directive(Directive::Send {
                            target: source,
                            bytes: &proof[..written],
                        }));
                    }
                }
            }
        }
    }
}
