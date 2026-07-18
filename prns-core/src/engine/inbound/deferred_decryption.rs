use super::delivery::DeliveryIo;
use crate::crypto::X25519SharedSecret;
use crate::engine::{DecryptOwed, EngineReaction, EngineState, RatchetDecryptOwed};
use crate::identity::{decrypt_finish_in_place, OpenedBy, OpenedToken};
use crate::interfaces::AttachedInterfaces;
use crate::routing::delivery::{Delivery, SingleDelivery};
use crate::routing::proof::{DeferredProofSign, ProofObligation, ProofOwed, ProofRequest};
use crate::storage::StorageLayout;

impl<S: StorageLayout> EngineState<S> {
    pub fn resume_decrypt(
        &mut self,
        owed: DecryptOwed,
        shared: X25519SharedSecret,
        interfaces: AttachedInterfaces<'_>,
        should_prove: &mut impl FnMut(&ProofRequest) -> bool,
        deferred_sign: &mut Option<DeferredProofSign>,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) {
        let DecryptOwed {
            destination,
            context,
            arrived_at,
            source_interface,
            identity,
            proof_strategy,
            packet_hash,
            mut token,
            ..
        } = owed;
        let Ok(plaintext) = decrypt_finish_in_place(&shared, &identity, &mut token) else {
            return;
        };
        let proof = ProofObligation::for_delivery(
            proof_strategy,
            ProofOwed {
                packet_hash,
                identity,
            },
        );
        let delivery = Delivery::Single(SingleDelivery {
            destination,
            context,
            plaintext,
            opened_by: OpenedBy::IdentityKey,
            arrived_at,
            source_interface,
        });
        self.process_delivery(
            delivery,
            proof,
            source_interface,
            &mut DeliveryIo {
                interfaces,
                should_prove: &mut *should_prove,
                deferred_sign: &mut *deferred_sign,
                sink: &mut *sink,
            },
        );
    }

    pub fn resume_ratchet_decrypt(
        &mut self,
        owed: RatchetDecryptOwed,
        opened: OpenedToken<'_>,
        interfaces: AttachedInterfaces<'_>,
        should_prove: &mut impl FnMut(&ProofRequest) -> bool,
        deferred_sign: &mut Option<DeferredProofSign>,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) {
        let proof = ProofObligation::for_delivery(
            owed.proof_strategy,
            ProofOwed {
                packet_hash: owed.packet_hash,
                identity: owed.identity,
            },
        );
        let delivery = Delivery::Single(SingleDelivery {
            destination: owed.destination,
            context: owed.context,
            plaintext: opened.plaintext,
            opened_by: opened.opened_by,
            arrived_at: owed.arrived_at,
            source_interface: owed.source_interface,
        });
        self.process_delivery(
            delivery,
            proof,
            owed.source_interface,
            &mut DeliveryIo {
                interfaces,
                should_prove: &mut *should_prove,
                deferred_sign: &mut *deferred_sign,
                sink: &mut *sink,
            },
        );
    }
}
