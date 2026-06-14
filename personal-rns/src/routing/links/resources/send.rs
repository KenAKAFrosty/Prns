//! The engine's send path for a resource — RNS 1.3.1 `Resource(data, link)`
//! plus `Resource.advertise`: seal the transfer straight into the outgoing
//! register and owe the advertisement to the link's interface. This is a
//! borrow-taking entry point beside the command queue, not a command: a
//! payload up to a mebibyte never rides an enum. Everything that can refuse
//! settles immediately; success settles later, when the receiver's proof
//! arrives or the transfer times out.

use crate::engine::commands::{CommandId, SendResourceError, SendResourceFailure, Settlement};
use crate::engine::{Directive, EngineReaction, EngineState, InstantMillis, Journaled};
use crate::interfaces::InterfaceId;
use crate::routing::dedup::{PacketHash, PacketHashHistory, RememberPacketOutcome};
use crate::routing::ingress::{DataPacket, IngestPacketOutcome};
use crate::routing::links::data::LINK_TRAFFIC_TIMEOUT_FACTOR;
use crate::routing::links::data::{write_link_packet, write_link_raw_packet, LINK_MDU};
use crate::routing::links::request::RequestId;
use crate::routing::links::resources::advertisement::{
    write_hashmap_update_plaintext, ResourceAdvertisement, ResourceFlags,
};
use crate::routing::links::resources::build_outgoing::build_outgoing_resource;
use crate::routing::links::resources::control::{
    parse_cancel_plaintext, parse_part_request_plaintext, parse_proof_plaintext,
    write_cancel_plaintext, PART_REQUEST_PLAINTEXT_CAP,
};
use crate::routing::links::resources::serve_outgoing::{plan_hashmap_update, serve_part_indices};
use crate::routing::links::resources::table::{OutgoingResourceStatus, TrackOutgoingResourceError};
use crate::routing::links::resources::{
    resource_sdu, ResourceHash, HASHMAP_MAX_LEN, MAP_HASH_LEN, MAX_ADV_RETRIES, MAX_RETRIES,
    PER_RETRY_DELAY_MS, PROCESSING_GRACE_MS, PROOF_TIMEOUT_FACTOR, RESOURCE_HASH_LEN,
    RESOURCE_NONCE_LEN, SENDER_GRACE_MS,
};
use crate::routing::links::table::LinkPhase;
use crate::routing::links::LinkId;
use crate::storage::StorageLayout;
use crate::wire::{DestinationHash, DestinationType, PacketType, WireContext};

impl<S: StorageLayout> EngineState<S> {
    /// Build and advertise one resource over an active link. `data` is the
    /// uncompressed payload; `compressed_candidate` is the host's bz2 attempt
    /// (or `None` — an embedded host never links a compressor) and the
    /// reference's keep-only-if-smaller rule picks between them. The sealed
    /// stream lands in the outgoing register's slot, the advertisement goes
    /// out grant-first, and the command settles now only on refusal —
    /// delivery settles at the receiver's proof.
    #[allow(clippy::too_many_arguments)]
    pub fn ingest_send_resource_into<F>(
        &mut self,
        id: CommandId,
        link_id: LinkId,
        data: &[u8],
        compressed_candidate: Option<&[u8]>,
        request_id: Option<RequestId>,
        now: InstantMillis,
        fill_entropy: &mut F,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) -> crate::engine::WakeSchedules
    where
        F: FnMut(&mut [u8]),
    {
        let mut wake_schedule_changes = crate::engine::WakeSchedules::UNCHANGED;
        let settle = |sink: &mut dyn FnMut(EngineReaction<'_>), failure| {
            sink(EngineReaction::Journaled(Journaled::CommandSettled {
                id,
                settlement: Settlement::SendResource(Err(failure)),
            }));
        };
        let Some(phase) = self.links.phase_for(&link_id) else {
            settle(
                sink,
                SendResourceFailure::Rejected(SendResourceError::NoSuchLink),
            );
            return wake_schedule_changes;
        };
        let LinkPhase::Active {
            key,
            mtu,
            attached_interface,
            rtt_ms,
            ..
        } = phase
        else {
            settle(
                sink,
                SendResourceFailure::Rejected(SendResourceError::LinkNotActive),
            );
            return wake_schedule_changes;
        };
        let mtu = *mtu;
        let fire_on = *attached_interface;
        let rtt_ms = *rtt_ms;

        let mut seal_iv = [0u8; 16];
        fill_entropy(&mut seal_iv);
        let sdu = resource_sdu(mtu);
        let tracked_result =
            self.outgoing_resources
                .track(link_id, sdu, id, request_id, |transfer, hashmap| {
                    build_outgoing_resource(
                        data,
                        compressed_candidate,
                        key,
                        &seal_iv,
                        || {
                            let mut nonce = [0u8; RESOURCE_NONCE_LEN];
                            fill_entropy(&mut nonce);
                            nonce
                        },
                        sdu,
                        transfer,
                        hashmap,
                    )
                });
        let hash = match tracked_result {
            Ok(hash) => hash,
            Err(error) => {
                let rejection = match error {
                    TrackOutgoingResourceError::TableFull => SendResourceError::TableFull,
                    TrackOutgoingResourceError::LinkBusy => SendResourceError::LinkBusy,
                    TrackOutgoingResourceError::Build(build) => SendResourceError::Build(build),
                };
                settle(sink, SendResourceFailure::Rejected(rejection));
                return wake_schedule_changes;
            }
        };

        let mut adv_iv = [0u8; 16];
        fill_entropy(&mut adv_iv);
        let wrote = emit_resource_advertisement(
            &self.outgoing_resources,
            &link_id,
            &hash,
            key,
            mtu,
            fire_on,
            &adv_iv,
            sink,
        );
        if wrote {
            if let Some(index) = self.outgoing_resources.lookup(&link_id, &hash) {
                self.outgoing_resources.state_mut(index).retries_left = MAX_ADV_RETRIES;
                self.outgoing_resources
                    .set_timeout_at(index, Some(advertised_deadline(now, rtt_ms)));
            }
        } else {
            self.outgoing_resources.remove(&link_id, &hash);
            settle(sink, SendResourceFailure::WriteFailed);
        }
        wake_schedule_changes.resource_deadlines = self.resource_deadlines_wake();
        wake_schedule_changes
    }

    /// RNS 1.3.1 `Transport.packet_filter` exempts `RESOURCE_REQ` from
    /// duplicate filtering. A receiver's retry is byte-identical by design,
    /// so unlike the other sealed link contexts this classifier never
    /// consults the packet-hash history.
    pub(crate) fn classify_resource_request<'p>(
        &mut self,
        data: DataPacket<'p>,
        arrived_at: InstantMillis,
    ) -> IngestPacketOutcome<'p> {
        let link_id = LinkId::new(*data.destination.as_bytes());
        let Some(LinkPhase::Active { key, .. }) = self.links.phase_for(&link_id) else {
            return IngestPacketOutcome::Ignored;
        };
        let Ok(plaintext) = key.open_in_place(data.payload) else {
            return IngestPacketOutcome::Ignored;
        };
        let plaintext: &'p [u8] = plaintext;
        let Ok(parsed) = parse_part_request_plaintext(plaintext) else {
            return IngestPacketOutcome::Ignored;
        };
        if self
            .outgoing_resources
            .lookup(&link_id, &parsed.hash)
            .is_none()
        {
            return IngestPacketOutcome::Ignored;
        }
        self.links.note_inbound(&link_id, arrived_at);
        IngestPacketOutcome::OwesResourceParts {
            link_id,
            hash: parsed.hash,
            requested: parsed.requested,
            exhausted_at: parsed.last_known_map_hash,
        }
    }

    /// RNS 1.3.1 `Resource.validate_proof`, with the hash half matched here
    /// where the reference matches it at dispatch: a 64-byte proof naming a
    /// transfer in our register settles its send the moment the proof half
    /// equals what the build predicted. `None` means the link is not ours at
    /// all — the caller falls through to the transported-link switch so a
    /// relay keeps forwarding resource proofs blind. `RESOURCE_PRF` is
    /// exempt from duplicate filtering, like the request.
    pub(crate) fn classify_resource_proof(
        &mut self,
        destination: &DestinationHash,
        payload: &[u8],
        arrived_at: InstantMillis,
    ) -> Option<IngestPacketOutcome<'static>> {
        let link_id = LinkId::new(*destination.as_bytes());
        self.links.phase_for(&link_id)?;
        let Ok((hash, proof)) = parse_proof_plaintext(payload) else {
            return Some(IngestPacketOutcome::Ignored);
        };
        let Some(index) = self.outgoing_resources.lookup(&link_id, &hash) else {
            return Some(IngestPacketOutcome::Ignored);
        };
        if proof != self.outgoing_resources.state(index).expected_proof {
            return Some(IngestPacketOutcome::Ignored);
        }
        let id = self.outgoing_resources.state(index).command_id;
        self.outgoing_resources.remove(&link_id, &hash);
        self.links.note_inbound(&link_id, arrived_at);
        Some(IngestPacketOutcome::ResourceDelivered { id })
    }

    /// RNS 1.3.1's link dispatch for `RESOURCE_RCL` — `Resource._rejected`:
    /// the receiver refused the offered transfer outright. The register row
    /// drops and the send settles rejected-by-peer. Sealed, and behind the
    /// duplicate filter.
    pub(crate) fn classify_resource_receiver_cancel<'p>(
        &mut self,
        data: DataPacket<'p>,
        arrived_at: InstantMillis,
    ) -> IngestPacketOutcome<'static> {
        let link_id = LinkId::new(*data.destination.as_bytes());
        let Some(LinkPhase::Active { key, .. }) = self.links.phase_for(&link_id) else {
            return IngestPacketOutcome::Ignored;
        };
        let packet_hash = PacketHash::of_fields(
            DestinationType::Link,
            PacketType::Data,
            &data.destination,
            data.context,
            data.payload,
        );
        match self.packet_hash_history.remember(packet_hash) {
            RememberPacketOutcome::AlreadyKnown => return IngestPacketOutcome::Ignored,
            RememberPacketOutcome::StoredFresh | RememberPacketOutcome::StoredAfterRotation => {}
        }
        let Ok(plaintext) = key.open_in_place(data.payload) else {
            return IngestPacketOutcome::Ignored;
        };
        let Ok(hash) = parse_cancel_plaintext(plaintext) else {
            return IngestPacketOutcome::Ignored;
        };
        let Some(index) = self.outgoing_resources.lookup(&link_id, &hash) else {
            return IngestPacketOutcome::Ignored;
        };
        let id = self.outgoing_resources.state(index).command_id;
        self.outgoing_resources.remove(&link_id, &hash);
        self.links.note_inbound(&link_id, arrived_at);
        IngestPacketOutcome::ResourceRejectedByPeer { id }
    }

    /// Answer one part request from the outgoing register — RNS 1.3.1
    /// `Resource.request`: the requested parts go back raw on the arrival
    /// lane (slices of the sealed stream, no token around them), a
    /// hashmap-exhausted request earns the next segment of names and slides
    /// the serving scope, and a request that breaks the segment sequencing
    /// cancels the transfer the way the reference does — except we settle
    /// the command with the failure's name.
    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn serve_resource_request<F>(
        &mut self,
        link_id: &LinkId,
        hash: &ResourceHash,
        requested: &[u8],
        exhausted_at: Option<[u8; MAP_HASH_LEN]>,
        fire_on: InterfaceId,
        now: InstantMillis,
        fill_entropy: &mut F,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) where
        F: FnMut(&mut [u8]),
    {
        let Some(index) = self.outgoing_resources.lookup(link_id, hash) else {
            return;
        };
        let Some(LinkPhase::Active {
            key, mtu, rtt_ms, ..
        }) = self.links.phase_for(link_id)
        else {
            return;
        };
        let mtu = *mtu;
        let rtt_ms = *rtt_ms;
        {
            let state = self.outgoing_resources.state_mut(index);
            if state.status == OutgoingResourceStatus::Advertised {
                state.status = OutgoingResourceStatus::Transferring;
                state.retries_left = MAX_RETRIES;
            }
        }
        self.outgoing_resources
            .set_timeout_at(index, Some(transferring_deadline(now, rtt_ms)));

        let scope_start = self.outgoing_resources.state(index).scope_start;
        let mut picked = [0usize; MAX_REQUESTED_PARTS];
        let mut picked_len = 0;
        for part in serve_part_indices(
            self.outgoing_resources.names_flat(index),
            scope_start,
            requested,
        ) {
            if picked_len == picked.len() {
                break;
            }
            picked[picked_len] = part;
            picked_len += 1;
        }

        for &part in &picked[..picked_len] {
            let outgoing = &self.outgoing_resources;
            let mut fill = |slot: &mut [u8]| -> Option<usize> {
                let state = outgoing.state(index);
                let sealed = outgoing.sealed_transfer(index);
                let start = part * state.sdu;
                let end = (start + state.sdu).min(sealed.len());
                write_link_raw_packet(
                    link_id,
                    PacketType::Data,
                    WireContext::Resource,
                    mtu,
                    &sealed[start..end],
                    slot,
                )
                .ok()
            };
            sink(EngineReaction::Directive(Directive::EmitFrame {
                target: fire_on,
                fill: &mut fill,
            }));
            self.outgoing_resources.mark_sent(index, part);
        }

        if let Some(last_known) = exhausted_at {
            let plan = plan_hashmap_update(
                self.outgoing_resources.names_flat(index),
                scope_start,
                &last_known,
            );
            match plan {
                Ok(plan) => {
                    self.outgoing_resources.state_mut(index).scope_start = plan.scope_start;
                    let mut iv = [0u8; 16];
                    fill_entropy(&mut iv);
                    let outgoing = &self.outgoing_resources;
                    let mut fill = |slot: &mut [u8]| -> Option<usize> {
                        let names = outgoing.names_flat(index);
                        let segment_names = &names
                            [plan.entries_start * MAP_HASH_LEN..plan.entries_end * MAP_HASH_LEN];
                        let mut plaintext = [0u8; LINK_MDU];
                        let plaintext_len = write_hashmap_update_plaintext(
                            hash,
                            plan.segment,
                            segment_names,
                            &mut plaintext,
                        )
                        .ok()?;
                        write_link_packet(
                            link_id,
                            key,
                            mtu,
                            WireContext::ResourceHashUpdate,
                            &plaintext[..plaintext_len],
                            &iv,
                            slot,
                        )
                        .ok()
                    };
                    sink(EngineReaction::Directive(Directive::EmitFrame {
                        target: fire_on,
                        fill: &mut fill,
                    }));
                }
                Err(_) => {
                    self.cancel_outgoing_resource(
                        link_id,
                        hash,
                        SendResourceFailure::Sequencing,
                        fill_entropy,
                        sink,
                    );
                    return;
                }
            }
        }

        let state = self.outgoing_resources.state_mut(index);
        if state.sent_part_count == state.part_count {
            state.status = OutgoingResourceStatus::AwaitingProof;
            state.retries_left = AWAITING_PROOF_RETRIES;
            self.outgoing_resources
                .set_timeout_at(index, Some(awaiting_proof_deadline(now, rtt_ms)));
        }
    }

    /// Cancel one outgoing transfer the way RNS 1.3.1 `Resource.cancel` does
    /// on the sending side: a sealed `RESOURCE_ICL` tells the receiver, the
    /// register row drops, and the command settles with the failure's name.
    pub(crate) fn cancel_outgoing_resource<F>(
        &mut self,
        link_id: &LinkId,
        hash: &ResourceHash,
        failure: SendResourceFailure,
        fill_entropy: &mut F,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) where
        F: FnMut(&mut [u8]),
    {
        let Some(index) = self.outgoing_resources.lookup(link_id, hash) else {
            return;
        };
        let id = self.outgoing_resources.state(index).command_id;
        self.outgoing_resources.remove(link_id, hash);
        if let Some(LinkPhase::Active {
            key,
            mtu,
            attached_interface,
            ..
        }) = self.links.phase_for(link_id)
        {
            let mtu = *mtu;
            let fire_on = *attached_interface;
            let mut cancel_iv = [0u8; 16];
            fill_entropy(&mut cancel_iv);
            let mut cancel_plaintext = [0u8; RESOURCE_HASH_LEN];
            if write_cancel_plaintext(hash, &mut cancel_plaintext).is_ok() {
                let mut fill = |slot: &mut [u8]| -> Option<usize> {
                    write_link_packet(
                        link_id,
                        key,
                        mtu,
                        WireContext::ResourceInitiatorCancel,
                        &cancel_plaintext,
                        &cancel_iv,
                        slot,
                    )
                    .ok()
                };
                sink(EngineReaction::Directive(Directive::EmitFrame {
                    target: fire_on,
                    fill: &mut fill,
                }));
            }
        }
        sink(EngineReaction::Journaled(Journaled::CommandSettled {
            id,
            settlement: Settlement::SendResource(Err(failure)),
        }));
    }

    /// The sender's half of the resource deadline lane — RNS 1.3.1's
    /// watchdog states as deadlines on the register: an unanswered
    /// advertisement re-sends up to its retry budget, a stalled transfer
    /// cancels at the fat wait, and a missing proof re-arms its window
    /// (the reference re-queries the network cache here — deferred with
    /// `CACHE_REQUEST`) before giving up.
    pub(crate) fn fire_due_outgoing_resources<F>(
        &mut self,
        now: InstantMillis,
        fill_entropy: &mut F,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) where
        F: FnMut(&mut [u8]),
    {
        while let Some(index) = self.outgoing_resources.due_index(now) {
            let link_id = *self.outgoing_resources.link_at(index);
            let hash = *self.outgoing_resources.hash_at(index);
            let state = *self.outgoing_resources.state(index);
            let Some(LinkPhase::Active {
                key,
                mtu,
                attached_interface,
                rtt_ms,
                ..
            }) = self.links.phase_for(&link_id)
            else {
                let id = state.command_id;
                self.outgoing_resources.remove(&link_id, &hash);
                sink(EngineReaction::Journaled(Journaled::CommandSettled {
                    id,
                    settlement: Settlement::SendResource(Err(SendResourceFailure::Timeout)),
                }));
                continue;
            };
            let mtu = *mtu;
            let fire_on = *attached_interface;
            let rtt_ms = *rtt_ms;
            match state.status {
                OutgoingResourceStatus::Advertised => {
                    if state.retries_left == 0 {
                        self.cancel_outgoing_resource(
                            &link_id,
                            &hash,
                            SendResourceFailure::Timeout,
                            fill_entropy,
                            sink,
                        );
                        continue;
                    }
                    let mut adv_iv = [0u8; 16];
                    fill_entropy(&mut adv_iv);
                    emit_resource_advertisement(
                        &self.outgoing_resources,
                        &link_id,
                        &hash,
                        key,
                        mtu,
                        fire_on,
                        &adv_iv,
                        sink,
                    );
                    let state = self.outgoing_resources.state_mut(index);
                    state.retries_left -= 1;
                    self.outgoing_resources
                        .set_timeout_at(index, Some(advertised_deadline(now, rtt_ms)));
                }
                OutgoingResourceStatus::Transferring => {
                    self.cancel_outgoing_resource(
                        &link_id,
                        &hash,
                        SendResourceFailure::Timeout,
                        fill_entropy,
                        sink,
                    );
                }
                OutgoingResourceStatus::AwaitingProof => {
                    if state.retries_left == 0 {
                        self.cancel_outgoing_resource(
                            &link_id,
                            &hash,
                            SendResourceFailure::Timeout,
                            fill_entropy,
                            sink,
                        );
                        continue;
                    }
                    let state = self.outgoing_resources.state_mut(index);
                    state.retries_left -= 1;
                    self.outgoing_resources
                        .set_timeout_at(index, Some(awaiting_proof_deadline(now, rtt_ms)));
                }
            }
        }
    }
}

fn advertised_deadline(now: InstantMillis, rtt_ms: u64) -> InstantMillis {
    InstantMillis(
        now.0
            .saturating_add(rtt_ms.saturating_mul(LINK_TRAFFIC_TIMEOUT_FACTOR))
            .saturating_add(PROCESSING_GRACE_MS),
    )
}

/// RNS 1.3.1's sender-side transferring wait: the full retry budget's worth
/// of round trips plus the sender grace and every per-retry delay — one fat
/// deadline re-armed on each request, after which the receiver is gone.
fn transferring_deadline(now: InstantMillis, rtt_ms: u64) -> InstantMillis {
    let retry_rtts = rtt_ms
        .saturating_mul(LINK_TRAFFIC_TIMEOUT_FACTOR)
        .saturating_mul(MAX_RETRIES as u64);
    let max_extra_wait = PER_RETRY_DELAY_MS * ((MAX_RETRIES as u64) * (MAX_RETRIES as u64 + 1) / 2);
    InstantMillis(
        now.0
            .saturating_add(retry_rtts)
            .saturating_add(SENDER_GRACE_MS)
            .saturating_add(max_extra_wait),
    )
}

fn awaiting_proof_deadline(now: InstantMillis, rtt_ms: u64) -> InstantMillis {
    InstantMillis(
        now.0
            .saturating_add(rtt_ms.saturating_mul(PROOF_TIMEOUT_FACTOR))
            .saturating_add(SENDER_GRACE_MS),
    )
}

/// Write one advertisement for a registered transfer into the link's wire
/// slot — shared by the first send and every watchdog re-send.
#[allow(clippy::too_many_arguments)]
fn emit_resource_advertisement<C>(
    outgoing: &crate::routing::links::resources::table::OutgoingResources<C>,
    link_id: &LinkId,
    hash: &ResourceHash,
    key: &crate::routing::links::LinkKey,
    mtu: usize,
    fire_on: InterfaceId,
    adv_iv: &[u8; 16],
    sink: &mut impl FnMut(EngineReaction<'_>),
) -> bool
where
    C: crate::routing::links::resources::table::ResourceColumns<
        crate::routing::links::resources::table::OutgoingResourceState,
    >,
{
    let mut wrote = false;
    let mut fill = |slot: &mut [u8]| -> Option<usize> {
        let index = outgoing.lookup(link_id, hash)?;
        let state = outgoing.state(index);
        let names = outgoing.names_flat(index);
        let first_segment = &names[..names.len().min(HASHMAP_MAX_LEN * MAP_HASH_LEN)];
        let advertisement = ResourceAdvertisement {
            transfer_size: state.sealed_transfer_len as u64,
            data_size: state.uncompressed_data_len,
            part_count: state.part_count as u64,
            hash: *hash,
            salt_nonce: state.salt_nonce,
            original_hash: *hash,
            segment_index: 1,
            total_segments: 1,
            request_id: state.request_id,
            flags: ResourceFlags {
                encrypted: true,
                compressed: state.compression.wire_flag(),
                split: false,
                is_request: false,
                is_response: state.request_id.is_some(),
                has_metadata: false,
            },
            hashmap: first_segment,
        };
        let mut plaintext = [0u8; LINK_MDU];
        let plaintext_len = advertisement.write(&mut plaintext).ok()?;
        let wire_len = write_link_packet(
            link_id,
            key,
            mtu,
            WireContext::ResourceAdvertisement,
            &plaintext[..plaintext_len],
            adv_iv,
            slot,
        )
        .ok()?;
        wrote = true;
        Some(wire_len)
    };
    sink(EngineReaction::Directive(Directive::EmitFrame {
        target: fire_on,
        fill: &mut fill,
    }));
    wrote
}

/// The most names one part request can carry:
/// the plaintext cap less the flag byte and resource hash, in map-hash strides.
const MAX_REQUESTED_PARTS: usize =
    (PART_REQUEST_PLAINTEXT_CAP - 1 - RESOURCE_HASH_LEN) / MAP_HASH_LEN;

/// RNS 1.3.1 `Resource.request`: `retries_left = 3` once every part has been
/// sent and only the proof is owed.
const AWAITING_PROOF_RETRIES: u8 = 3;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::{x25519_diffie_hellman, X25519PublicKey, X25519SecretKey};
    use crate::crypto::{CryptoError, Ed25519PublicKey, Ed25519SecretKey};
    use crate::engine::test_support::{filled_frame, Cap};
    use crate::engine::InstantMillis;
    use crate::interfaces::InterfaceId;
    use crate::routing::links::resources::build_outgoing::BuildOutgoingResourceError;
    use crate::routing::links::resources::table::OutgoingResourceStatus;
    use crate::routing::links::table::InitiatedLink;
    use crate::routing::links::LinkKey;
    use crate::wire::{DestinationHash, PacketType, WirePacketHeader, BROADCAST_MTU};

    fn hx(s: &str) -> std::vec::Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("valid hex"))
            .collect()
    }

    const LINK_ID: &str = "000102030405060708090a0b0c0d0e0f";
    const INITIATOR_SCALAR: &str =
        "3333333333333333333333333333333333333333333333333333333333333333";
    const RESPONDER_PUBLIC: &str =
        "ff2ee45601ec1b67310c7790404585ae697331eee1c1f8cf2419731c1fff3e6b";

    pub(crate) fn link_id() -> LinkId {
        LinkId::new(hx(LINK_ID).try_into().unwrap())
    }

    pub(crate) fn link_key() -> LinkKey {
        let scalar: [u8; 32] = hx(INITIATOR_SCALAR).try_into().unwrap();
        let public: [u8; 32] = hx(RESPONDER_PUBLIC).try_into().unwrap();
        let shared = x25519_diffie_hellman(&X25519SecretKey::new(scalar), &X25519PublicKey(public));
        LinkKey::derive(&link_id(), &shared)
    }

    fn lane() -> InterfaceId {
        InterfaceId::new([0xEE; 16])
    }

    fn install_active_link<S: StorageLayout>(engine: &mut EngineState<S>) {
        engine
            .links
            .track_initiated(InitiatedLink {
                link_id: link_id(),
                destination: DestinationHash::new([0x77; 16]),
                initiator_secret: X25519SecretKey::new([0x33; 32]),
                link_signing: Ed25519SecretKey::new([0x33; 32]),
                requested_at: InstantMillis(500),
                timeout_at: InstantMillis(5_000),
                command_id: CommandId(1),
            })
            .unwrap();
        engine
            .links
            .activate_initiated(
                &link_id(),
                link_key(),
                250,
                BROADCAST_MTU,
                lane(),
                InstantMillis(1_000),
                Ed25519PublicKey([0x99; 32]),
            )
            .unwrap();
    }

    pub(crate) fn sender_with_active_link() -> EngineState<Cap> {
        let mut engine = EngineState::<Cap>::default();
        install_active_link(&mut engine);
        engine
    }

    pub(crate) struct SendCapture {
        pub(crate) frames: std::vec::Vec<(InterfaceId, std::vec::Vec<u8>)>,
        pub(crate) settlements: std::vec::Vec<(CommandId, Settlement)>,
    }

    pub(crate) fn watch_capture(engine: &mut EngineState<Cap>, at: u64) -> SendCapture {
        let mut capture = SendCapture {
            frames: std::vec::Vec::new(),
            settlements: std::vec::Vec::new(),
        };
        engine.fire_due_resource_deadlines(
            InstantMillis(at),
            &mut |bytes: &mut [u8]| bytes.fill(0xF1),
            &mut |reaction| match reaction {
                EngineReaction::Directive(Directive::EmitFrame { target, fill }) => {
                    if let Some(frame) = filled_frame(fill) {
                        capture.frames.push((target, frame));
                    }
                }
                EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) => {
                    capture.settlements.push((id, settlement));
                }
                _ => {}
            },
        );
        capture
    }

    pub(crate) fn send<S: StorageLayout>(
        engine: &mut EngineState<S>,
        id: u64,
        data: &[u8],
        candidate: Option<&[u8]>,
    ) -> SendCapture {
        let mut capture = SendCapture {
            frames: std::vec::Vec::new(),
            settlements: std::vec::Vec::new(),
        };
        engine.ingest_send_resource_into(
            CommandId(id),
            link_id(),
            data,
            candidate,
            None,
            InstantMillis(1_500),
            &mut |bytes: &mut [u8]| bytes.fill(0xA5),
            &mut |reaction| match reaction {
                EngineReaction::Directive(Directive::EmitFrame { target, fill }) => {
                    if let Some(frame) = filled_frame(fill) {
                        capture.frames.push((target, frame));
                    }
                }
                EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) => {
                    capture.settlements.push((id, settlement));
                }
                _ => {}
            },
        );
        capture
    }

    fn case1_plaintext() -> std::vec::Vec<u8> {
        b"reticulum resources ride the link ".repeat(40)
    }

    const CASE1_BZ2: &str = "425a6839314159265359cf3017f4000207918040000e6f9e002000902980000a54a7a869ea794d3227c13a1382644e09a09a1342684f213f04c09b1382704ec2684d89e04c8ab61302604d09d09d89fc5dc914e142433cc05fd0";

    #[test]
    fn a_send_resource_seals_registers_and_advertises() {
        let mut engine = sender_with_active_link();
        let plaintext = case1_plaintext();
        let candidate = hx(CASE1_BZ2);
        let capture = send(&mut engine, 7, &plaintext, Some(&candidate));

        assert!(
            capture.settlements.is_empty(),
            "success settles at the proof"
        );
        assert_eq!(capture.frames.len(), 1);
        let (target, frame) = &capture.frames[0];
        assert_eq!(*target, lane());

        let (header, payload) = WirePacketHeader::parse(frame).unwrap();
        assert_eq!(header.packet_type, PacketType::Data);
        assert_eq!(header.context, WireContext::ResourceAdvertisement);
        assert_eq!(
            header.destination,
            DestinationHash::new(*link_id().as_bytes())
        );

        let mut sealed = payload.to_vec();
        let opened = link_key().open_in_place(&mut sealed).unwrap();
        let advertisement = ResourceAdvertisement::parse(opened).unwrap();

        let index = engine
            .outgoing_resources
            .lookup(&link_id(), &advertisement.hash)
            .expect("the advertised transfer is registered");
        let state = engine.outgoing_resources.state(index);
        assert_eq!(state.status, OutgoingResourceStatus::Advertised);
        assert_eq!(
            advertisement.transfer_size,
            state.sealed_transfer_len as u64
        );
        assert_eq!(advertisement.data_size, 1_360);
        assert_eq!(advertisement.part_count, 1);
        assert_eq!(advertisement.salt_nonce, state.salt_nonce);
        assert_eq!(advertisement.original_hash, advertisement.hash);
        assert_eq!(advertisement.segment_index, 1);
        assert_eq!(advertisement.total_segments, 1);
        assert_eq!(advertisement.request_id, None);
        assert!(advertisement.flags.encrypted);
        assert!(advertisement.flags.compressed);
        assert!(!advertisement.flags.is_response);
        assert_eq!(
            advertisement.hashmap,
            engine.outgoing_resources.names_flat(index),
        );
    }

    #[test]
    fn one_resource_per_link_rejects_the_second_send() {
        let mut engine = sender_with_active_link();
        let plaintext = case1_plaintext();
        send(&mut engine, 7, &plaintext, None);
        let second = send(&mut engine, 8, &plaintext, None);

        assert!(second.frames.is_empty());
        assert_eq!(second.settlements.len(), 1);
        assert!(matches!(
            second.settlements[0],
            (
                CommandId(8),
                Settlement::SendResource(Err(SendResourceFailure::Rejected(
                    SendResourceError::LinkBusy,
                ))),
            ),
        ));
        assert_eq!(engine.outgoing_resources.len(), 1);
    }

    #[test]
    fn a_missing_or_inactive_link_rejects_by_name() {
        let mut engine = EngineState::<Cap>::default();
        let capture = send(&mut engine, 7, b"data", None);
        assert!(matches!(
            capture.settlements[0],
            (
                CommandId(7),
                Settlement::SendResource(Err(SendResourceFailure::Rejected(
                    SendResourceError::NoSuchLink,
                ))),
            ),
        ));

        engine
            .links
            .track_initiated(InitiatedLink {
                link_id: link_id(),
                destination: DestinationHash::new([0x77; 16]),
                initiator_secret: X25519SecretKey::new([0x33; 32]),
                link_signing: Ed25519SecretKey::new([0x33; 32]),
                requested_at: InstantMillis(500),
                timeout_at: InstantMillis(5_000),
                command_id: CommandId(1),
            })
            .unwrap();
        let capture = send(&mut engine, 8, b"data", None);
        assert!(matches!(
            capture.settlements[0],
            (
                CommandId(8),
                Settlement::SendResource(Err(SendResourceFailure::Rejected(
                    SendResourceError::LinkNotActive,
                ))),
            ),
        ));
        assert!(engine.outgoing_resources.is_empty());
    }

    pub(crate) struct InboundCapture {
        pub(crate) frames: std::vec::Vec<(InterfaceId, std::vec::Vec<u8>)>,
        pub(crate) settlements: std::vec::Vec<(CommandId, Settlement)>,
    }

    pub(crate) fn feed<S: StorageLayout>(
        engine: &mut EngineState<S>,
        frame: &[u8],
        at: u64,
    ) -> InboundCapture {
        use crate::engine::test_support::{routable_descriptor, TEST_ENTROPY};
        use crate::interfaces::InboundPacket;
        let mut capture = InboundCapture {
            frames: std::vec::Vec::new(),
            settlements: std::vec::Vec::new(),
        };
        let mut raw = frame.to_vec();
        engine.ingest_packet_into(
            InboundPacket {
                arrived_at: InstantMillis(at),
                source_interface: lane(),
                bytes: &mut raw,
            },
            TEST_ENTROPY,
            &[routable_descriptor(lane())],
            InstantMillis(at),
            &mut |bytes: &mut [u8]| bytes.fill(0xC7),
            &mut |_: &crate::engine::ProofRequest| false,
            &mut |reaction| match reaction {
                EngineReaction::Directive(Directive::EmitFrame { target, fill }) => {
                    if let Some(frame) = filled_frame(fill) {
                        capture.frames.push((target, frame));
                    }
                }
                EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) => {
                    capture.settlements.push((id, settlement));
                }
                _ => {}
            },
        );
        capture
    }

    pub(crate) fn request_frame(
        hash: &ResourceHash,
        last_known: Option<&[u8; MAP_HASH_LEN]>,
        requested: &[u8],
    ) -> std::vec::Vec<u8> {
        use crate::routing::links::resources::control::write_part_request_plaintext;
        let mut plaintext = [0u8; PART_REQUEST_PLAINTEXT_CAP];
        let plaintext_len =
            write_part_request_plaintext(hash, last_known, requested, &mut plaintext).unwrap();
        let mut frame = [0u8; BROADCAST_MTU];
        let wire_len = write_link_packet(
            &link_id(),
            &link_key(),
            BROADCAST_MTU,
            WireContext::ResourceRequest,
            &plaintext[..plaintext_len],
            &[0xC3; 16],
            &mut frame,
        )
        .unwrap();
        frame[..wire_len].to_vec()
    }

    fn advertised_resource<S: StorageLayout>(
        engine: &mut EngineState<S>,
        data: &[u8],
    ) -> (ResourceHash, std::vec::Vec<u8>) {
        let capture = send(engine, 7, data, None);
        let (_, frame) = &capture.frames[0];
        let (_, payload) = WirePacketHeader::parse(frame).unwrap();
        let mut sealed = payload.to_vec();
        let opened = link_key().open_in_place(&mut sealed).unwrap();
        let advertisement = ResourceAdvertisement::parse(opened).unwrap();
        (advertisement.hash, advertisement.hashmap.to_vec())
    }

    fn four_part_payload() -> std::vec::Vec<u8> {
        b"resource parts ride raw on the wire! ".repeat(41)
    }

    #[test]
    fn requested_parts_stream_back_raw_from_the_register() {
        let mut engine = sender_with_active_link();
        let data = four_part_payload();
        let (hash, names) = advertised_resource(&mut engine, &data);

        let mut requested = std::vec::Vec::new();
        requested.extend_from_slice(&names[4..8]);
        requested.extend_from_slice(&names[12..16]);
        let capture = feed(&mut engine, &request_frame(&hash, None, &requested), 2_000);

        assert_eq!(capture.frames.len(), 2);
        let index = engine.outgoing_resources.lookup(&link_id(), &hash).unwrap();
        let sealed = engine.outgoing_resources.sealed_transfer(index);
        for ((target, frame), expected) in capture
            .frames
            .iter()
            .zip([&sealed[464..928], &sealed[1_392..]])
        {
            assert_eq!(*target, lane());
            let (header, payload) = WirePacketHeader::parse(frame).unwrap();
            assert_eq!(header.packet_type, PacketType::Data);
            assert_eq!(header.context, WireContext::Resource);
            assert_eq!(payload, expected, "the part is a raw sealed-stream slice");
        }
        let state = engine.outgoing_resources.state(index);
        assert_eq!(state.status, OutgoingResourceStatus::Transferring);
        assert_eq!(state.sent_part_count, 2);
    }

    #[test]
    fn serving_every_part_awaits_the_proof_and_resends_are_not_recounted() {
        let mut engine = sender_with_active_link();
        let data = four_part_payload();
        let (hash, names) = advertised_resource(&mut engine, &data);

        let first = feed(&mut engine, &request_frame(&hash, None, &names), 2_000);
        assert_eq!(first.frames.len(), 4);
        let index = engine.outgoing_resources.lookup(&link_id(), &hash).unwrap();
        let state = engine.outgoing_resources.state(index);
        assert_eq!(state.status, OutgoingResourceStatus::AwaitingProof);
        assert_eq!(state.sent_part_count, 4);
        assert_eq!(state.retries_left, 3);

        let again = feed(&mut engine, &request_frame(&hash, None, &names), 2_500);
        assert_eq!(
            again.frames.len(),
            4,
            "an identical retry passes the duplicate filter, like the reference exempts RESOURCE_REQ",
        );
        let state = engine.outgoing_resources.state(index);
        assert_eq!(state.sent_part_count, 4, "a resend is never recounted");
    }

    #[test]
    fn a_request_for_an_unknown_transfer_is_ignored() {
        let mut engine = sender_with_active_link();
        let data = four_part_payload();
        let (_, names) = advertised_resource(&mut engine, &data);

        let unknown = ResourceHash::new([0x5A; 32]);
        let capture = feed(
            &mut engine,
            &request_frame(&unknown, None, &names[..4]),
            2_000,
        );
        assert!(capture.frames.is_empty());
        assert!(capture.settlements.is_empty());
    }

    #[test]
    fn an_exhausted_request_earns_the_next_hashmap_segment() {
        use crate::routing::links::resources::advertisement::parse_hashmap_update_plaintext;
        use crate::storage::GrowableHeap;

        let mut engine = EngineState::<GrowableHeap>::default();
        install_active_link(&mut engine);
        let data = std::vec![0x42u8; 100 * 464 - 100];
        let (hash, names) = advertised_resource(&mut engine, &data);
        assert_eq!(
            names.len(),
            74 * MAP_HASH_LEN,
            "the advertisement carries one segment"
        );

        let last_known: [u8; 4] = names[73 * 4..74 * 4].try_into().unwrap();
        let capture = feed(
            &mut engine,
            &request_frame(&hash, Some(&last_known), &names[72 * 4..74 * 4]),
            2_000,
        );

        assert_eq!(capture.frames.len(), 3, "two parts and the hashmap update");
        let (_, hmu_frame) = &capture.frames[2];
        let (header, payload) = WirePacketHeader::parse(hmu_frame).unwrap();
        assert_eq!(header.context, WireContext::ResourceHashUpdate);
        let mut sealed = payload.to_vec();
        let opened = link_key().open_in_place(&mut sealed).unwrap();
        let update = parse_hashmap_update_plaintext(opened).unwrap();
        assert_eq!(update.hash, hash);
        assert_eq!(update.segment, 1);

        let index = engine.outgoing_resources.lookup(&link_id(), &hash).unwrap();
        assert_eq!(
            update.hashmap,
            &engine.outgoing_resources.names_flat(index)[74 * MAP_HASH_LEN..],
            "the update carries every name past the first segment",
        );
        assert_eq!(engine.outgoing_resources.state(index).scope_start, 0);
    }

    #[test]
    fn a_sequencing_break_cancels_the_transfer_by_name() {
        use crate::storage::GrowableHeap;

        let mut engine = EngineState::<GrowableHeap>::default();
        install_active_link(&mut engine);
        let data = std::vec![0x42u8; 100 * 464 - 100];
        let (hash, names) = advertised_resource(&mut engine, &data);

        let off_boundary: [u8; 4] = names[10 * 4..11 * 4].try_into().unwrap();
        let capture = feed(
            &mut engine,
            &request_frame(&hash, Some(&off_boundary), &[]),
            2_000,
        );

        assert_eq!(capture.frames.len(), 1, "the cancel rides to the receiver");
        let (_, cancel) = &capture.frames[0];
        let (header, payload) = WirePacketHeader::parse(cancel).unwrap();
        assert_eq!(header.context, WireContext::ResourceInitiatorCancel);
        let mut sealed = payload.to_vec();
        let opened = link_key().open_in_place(&mut sealed).unwrap();
        assert_eq!(
            crate::routing::links::resources::control::parse_cancel_plaintext(opened).unwrap(),
            hash,
        );
        assert!(matches!(
            capture.settlements[0],
            (
                CommandId(7),
                Settlement::SendResource(Err(SendResourceFailure::Sequencing)),
            ),
        ));
        assert!(engine.outgoing_resources.is_empty());
    }

    fn proof_frame(
        hash: &ResourceHash,
        proof: &crate::routing::links::resources::ResourceProof,
    ) -> std::vec::Vec<u8> {
        use crate::routing::links::resources::control::write_proof_plaintext;
        let mut plaintext = [0u8; 64];
        write_proof_plaintext(hash, proof, &mut plaintext).unwrap();
        let mut frame = [0u8; BROADCAST_MTU];
        let wire_len = write_link_raw_packet(
            &link_id(),
            PacketType::Proof,
            WireContext::ResourceProof,
            BROADCAST_MTU,
            &plaintext,
            &mut frame,
        )
        .unwrap();
        frame[..wire_len].to_vec()
    }

    #[test]
    fn the_receivers_proof_settles_the_send_and_retires_the_transfer() {
        use crate::routing::links::resources::assemble_incoming::{
            open_transfer, verify_and_prove,
        };

        let mut engine = sender_with_active_link();
        let data = four_part_payload();
        let capture = send(&mut engine, 7, &data, None);
        let (_, adv_frame) = &capture.frames[0];
        let (_, adv_payload) = WirePacketHeader::parse(adv_frame).unwrap();
        let mut sealed_adv = adv_payload.to_vec();
        let opened = link_key().open_in_place(&mut sealed_adv).unwrap();
        let advertisement = ResourceAdvertisement::parse(opened).unwrap();

        let serve = feed(
            &mut engine,
            &request_frame(&advertisement.hash, None, advertisement.hashmap),
            2_000,
        );
        let mut reassembled = std::vec::Vec::new();
        for (_, frame) in &serve.frames {
            let (_, part) = WirePacketHeader::parse(frame).unwrap();
            reassembled.extend_from_slice(part);
        }
        let plaintext = open_transfer(&link_key(), &mut reassembled).unwrap();
        assert_eq!(
            plaintext,
            &data[..],
            "the receiver assembles the original data"
        );
        let proof =
            verify_and_prove(plaintext, &advertisement.salt_nonce, &advertisement.hash).unwrap();

        let settled = feed(
            &mut engine,
            &proof_frame(&advertisement.hash, &proof),
            3_000,
        );
        assert!(matches!(
            settled.settlements[0],
            (CommandId(7), Settlement::SendResource(Ok(()))),
        ));
        assert!(
            engine.outgoing_resources.is_empty(),
            "a proven transfer retires its register row",
        );
    }

    #[test]
    fn a_wrong_or_misaddressed_proof_settles_nothing() {
        use crate::routing::links::resources::ResourceProof;

        let mut engine = sender_with_active_link();
        let data = four_part_payload();
        let (hash, names) = advertised_resource(&mut engine, &data);
        feed(&mut engine, &request_frame(&hash, None, &names), 2_000);

        let forged = feed(
            &mut engine,
            &proof_frame(&hash, &ResourceProof::new([0x5A; 32])),
            3_000,
        );
        assert!(forged.settlements.is_empty());

        let unknown = feed(
            &mut engine,
            &proof_frame(
                &ResourceHash::new([0x66; 32]),
                &ResourceProof::new([0x5A; 32]),
            ),
            3_100,
        );
        assert!(unknown.settlements.is_empty());

        let index = engine.outgoing_resources.lookup(&link_id(), &hash).unwrap();
        assert_eq!(
            engine.outgoing_resources.state(index).status,
            OutgoingResourceStatus::AwaitingProof,
            "the transfer keeps waiting for the genuine proof",
        );
    }

    #[test]
    fn a_transfer_the_store_cannot_hold_rejects_and_releases_the_slot() {
        let mut engine = sender_with_active_link();
        let oversized = std::vec![0x42u8; 5_000];
        let capture = send(&mut engine, 7, &oversized, None);

        assert!(capture.frames.is_empty());
        assert!(matches!(
            capture.settlements[0],
            (
                CommandId(7),
                Settlement::SendResource(Err(SendResourceFailure::Rejected(
                    SendResourceError::Build(BuildOutgoingResourceError::Seal(
                        CryptoError::BufferTooShort,
                    )),
                ))),
            ),
        ));
        assert!(engine.outgoing_resources.is_empty());
    }
}

#[cfg(test)]
mod watchdog_tests {
    use super::tests::{link_id, sender_with_active_link, watch_capture, SendCapture};
    use super::*;
    use crate::engine::commands::Settlement;
    use crate::engine::{InstantMillis, LaneWake};
    use crate::wire::WirePacketHeader;

    fn advertised_sender() -> (
        crate::engine::EngineState<crate::engine::test_support::Cap>,
        SendCapture,
    ) {
        let mut engine = sender_with_active_link();
        let data = b"watchdogs keep the resource honest! ".repeat(40);
        let capture = super::tests::send(&mut engine, 7, &data, None);
        (engine, capture)
    }

    #[test]
    fn an_unanswered_advertisement_retries_then_cancels_with_its_name() {
        let (mut engine, _) = advertised_sender();
        assert_eq!(
            engine.resource_deadlines_wake(),
            LaneWake::At(InstantMillis(1_500 + 250 * 6 + 1_000)),
            "the advertisement arms rtt x traffic factor plus the processing grace",
        );

        let mut now = 4_000u64;
        for retry in 0..4u64 {
            let capture = watch_capture(&mut engine, now);
            assert_eq!(capture.frames.len(), 1, "retry {retry} re-advertises");
            let (header, _) = WirePacketHeader::parse(&capture.frames[0].1).unwrap();
            assert_eq!(header.context, WireContext::ResourceAdvertisement);
            assert!(capture.settlements.is_empty());
            now += 3_000;
        }

        let capture = watch_capture(&mut engine, now);
        assert_eq!(capture.frames.len(), 1, "the cancel rides out");
        let (header, _) = WirePacketHeader::parse(&capture.frames[0].1).unwrap();
        assert_eq!(header.context, WireContext::ResourceInitiatorCancel);
        assert!(matches!(
            capture.settlements[0],
            (
                CommandId(7),
                Settlement::SendResource(Err(SendResourceFailure::Timeout)),
            ),
        ));
        assert!(engine.outgoing_resources.is_empty());
        assert_eq!(engine.resource_deadlines_wake(), LaneWake::Idle);
    }

    #[test]
    fn a_missing_proof_rearms_its_retries_then_cancels() {
        let (mut engine, capture) = advertised_sender();
        let (_, adv_frame) = &capture.frames[0];
        let (_, payload) = WirePacketHeader::parse(adv_frame).unwrap();
        let mut sealed = payload.to_vec();
        let opened = super::tests::link_key().open_in_place(&mut sealed).unwrap();
        let advertisement = ResourceAdvertisement::parse(opened).unwrap();
        super::tests::feed(
            &mut engine,
            &super::tests::request_frame(&advertisement.hash, None, advertisement.hashmap),
            2_000,
        );
        let index = engine
            .outgoing_resources
            .lookup(&link_id(), &advertisement.hash)
            .unwrap();
        assert_eq!(
            engine.outgoing_resources.state(index).status,
            OutgoingResourceStatus::AwaitingProof,
        );
        assert_eq!(
            engine.resource_deadlines_wake(),
            LaneWake::At(InstantMillis(2_000 + 250 * 3 + 10_000)),
        );

        let mut now = 13_000u64;
        for _ in 0..3 {
            let capture = watch_capture(&mut engine, now);
            assert!(capture.frames.is_empty(), "a proof retry sends nothing yet");
            assert!(capture.settlements.is_empty());
            now += 11_000;
        }
        let capture = watch_capture(&mut engine, now);
        assert!(matches!(
            capture.settlements[0],
            (
                CommandId(7),
                Settlement::SendResource(Err(SendResourceFailure::Timeout)),
            ),
        ));
        assert!(engine.outgoing_resources.is_empty());
    }
}
