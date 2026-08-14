use crate::crypto::{Ed25519Signature, X25519PublicKey, X25519SharedSecret};
use crate::engine::settlement::settle;
use crate::engine::{
    CommandId, Directive, EngineReaction, EngineState, InstantMillis, LinkEstablished, LinkRttOwed,
    Settlement, WakeSchedule, WakeSchedules,
};
use crate::identity::ENCRYPTION_IV_LEN;
use crate::interfaces::{AttachedInterfaces, Egress, InterfaceId};
use crate::routing::links::establish::link_mtu_ceiling;
use crate::routing::links::establish::LinkRttTimes;
use crate::routing::links::handshake::{LinkProofSignOwed, LinkProofVerifyOwed};
use crate::routing::links::table::LinkActivation;
use crate::routing::links::LinkId;
use crate::storage::StorageLayout;
use crate::units::RttMillis;
use crate::wire::BROADCAST_MTU;

impl<S: StorageLayout> EngineState<S> {
    fn emit_link_established(
        command_id: CommandId,
        link_id: LinkId,
        rtt: RttMillis,
        target: InterfaceId,
        written: &[u8],
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) {
        sink(EngineReaction::Directive(Directive::Send {
            target,
            bytes: written,
        }));
        settle(
            sink,
            command_id,
            Settlement::EstablishLink(Ok(LinkEstablished {
                link_id,
                rtt_millis: rtt.millis(),
            })),
        );
    }

    pub(super) fn process_owes_link_rtt<F>(
        &mut self,
        owed: LinkRttOwed,
        source: InterfaceId,
        interfaces: AttachedInterfaces<'_>,
        _now: InstantMillis,
        fill_entropy: &mut F,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) -> WakeSchedule
    where
        F: FnMut(&mut [u8]),
    {
        if !interfaces.is_egress_eligible(source, Egress::Transmit) {
            return WakeSchedule::Unchanged;
        }
        let mut iv = [0u8; ENCRYPTION_IV_LEN];
        fill_entropy(&mut iv);
        let mut buf = [0u8; BROADCAST_MTU];
        if let Ok(written) = self.write_owed_link_rtt(
            &owed.link_id,
            &owed.responder_encryption,
            &LinkActivation {
                received_hops: owed.received_hops,
                rtt: owed.rtt,
                mtu: owed.mtu.min(link_mtu_ceiling(interfaces, source)),
                attached_interface: source,
                peer_signing: owed.responder_signing,
            },
            owed.arrived_at,
            &iv,
            &mut buf,
        ) {
            Self::emit_link_established(
                owed.command_id,
                owed.link_id,
                owed.rtt,
                source,
                &buf[..written],
                sink,
            );
        }
        self.link_deadlines_wake()
    }

    fn process_owes_link_rtt_with_shared<F>(
        &mut self,
        owed: LinkProofVerifyOwed,
        shared: X25519SharedSecret,
        interfaces: AttachedInterfaces<'_>,
        now: InstantMillis,
        fill_entropy: &mut F,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) -> WakeSchedule
    where
        F: FnMut(&mut [u8]),
    {
        let source = owed.source_interface;
        if !interfaces.is_egress_eligible(source, Egress::Transmit) {
            return WakeSchedule::Unchanged;
        }
        let mut iv = [0u8; ENCRYPTION_IV_LEN];
        fill_entropy(&mut iv);
        let mut buf = [0u8; BROADCAST_MTU];
        if let Ok(written) = self.write_owed_link_rtt_with_shared_observed(
            &owed.link_id,
            &shared,
            &LinkActivation {
                received_hops: owed.received_hops,
                rtt: owed.rtt,
                mtu: owed.mtu.min(link_mtu_ceiling(interfaces, source)),
                attached_interface: source,
                peer_signing: owed.responder_signing,
            },
            LinkRttTimes {
                activated_at: now,
                evidence_observed_at: owed.arrived_at,
            },
            &iv,
            &mut buf,
        ) {
            Self::emit_link_established(
                owed.command_id,
                owed.link_id,
                owed.rtt,
                source,
                &buf[..written],
                sink,
            );
        }
        self.link_deadlines_wake()
    }

    pub fn resume_link_proof<F>(
        &mut self,
        owed: LinkProofVerifyOwed,
        shared: X25519SharedSecret,
        interfaces: AttachedInterfaces<'_>,
        now: InstantMillis,
        fill_entropy: &mut F,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) -> WakeSchedules
    where
        F: FnMut(&mut [u8]),
    {
        let mut wake = WakeSchedules::UNCHANGED;
        wake.link_deadlines = self.process_owes_link_rtt_with_shared(
            owed,
            shared,
            interfaces,
            now,
            fill_entropy,
            sink,
        );
        wake
    }

    pub fn resume_link_proof_sign(
        &mut self,
        owed: LinkProofSignOwed,
        responder_encryption: X25519PublicKey,
        shared: X25519SharedSecret,
        signature: Ed25519Signature,
        interfaces: AttachedInterfaces<'_>,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) -> WakeSchedules {
        let mut wake = WakeSchedules::UNCHANGED;
        if !interfaces.is_egress_eligible(owed.source_interface, Egress::Transmit) {
            return wake;
        }
        let mut buf = [0u8; BROADCAST_MTU];
        if let Ok(written) = self.write_owed_link_proof_with_parts(
            &owed,
            &responder_encryption,
            &shared,
            &signature,
            &mut buf,
        ) {
            sink(EngineReaction::Directive(Directive::Send {
                target: owed.source_interface,
                bytes: &buf[..written],
            }));
        }
        wake.link_deadlines = self.link_deadlines_wake();
        wake
    }
}
