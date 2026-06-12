//! The engine's receive path for a resource — RNS 1.3.1 `Resource.accept`
//! plus the receiver's half of the link dispatch: gate an inbound
//! advertisement on the link's [`ResourceStrategy`] and the store's
//! capacity, register the transfer, and start pulling parts by name. The
//! strategy gate runs before a single part moves: the advertisement declares
//! the decompressed size and compression kind up front, so refusing is free.

use crate::engine::commands::{
    CommandId, CommandOutcome, SetResourceStrategy, SetResourceStrategyError,
};
use crate::engine::Journaled;
use crate::engine::{Directive, EngineReaction, EngineState, InstantMillis};
use crate::routing::dedup::{PacketHash, PacketHashHistory, RememberPacketOutcome};
use crate::routing::ingress::{DataPacket, IngestPacketOutcome};
use crate::routing::links::data::write_link_packet;
use crate::routing::links::data::write_link_raw_packet;
use crate::routing::links::resources::advertisement::{
    parse_hashmap_update_plaintext, ResourceAdvertisement,
};
use crate::routing::links::resources::assemble_incoming::{
    match_part_in_window, open_transfer, verify_and_prove,
};
use crate::routing::links::resources::control::{
    parse_cancel_plaintext, write_part_request_plaintext, write_proof_plaintext,
    PROOF_PLAINTEXT_LEN,
};
use crate::routing::links::resources::table::{AcceptedResource, IncomingResourceStatus};
use crate::routing::links::resources::{
    resource_sdu, ResourceCompression, ResourceHash, ResourceStrategy, MAP_HASH_LEN, MAX_RETRIES,
    PART_TIMEOUT_FACTOR, PER_RETRY_DELAY_MS, RETRY_GRACE_MS, WINDOW_FLEXIBILITY, WINDOW_MAX,
};
use crate::routing::links::table::LinkPhase;
use crate::routing::links::LinkId;
use crate::routing::storage::EngineStorage;
use crate::wire::{DestinationType, PacketType, WireContext};

impl<S: EngineStorage> EngineState<S> {
    pub(crate) fn ingest_set_resource_strategy(
        &mut self,
        id: CommandId,
        set: SetResourceStrategy,
    ) -> CommandOutcome {
        use crate::routing::links::table::LinkActivationError;
        match self.links.set_resource_strategy(&set.link_id, set.strategy) {
            Ok(()) => CommandOutcome::ResourceStrategySet { id },
            Err(LinkActivationError::UnknownLink) => CommandOutcome::SetResourceStrategyRejected {
                id,
                error: SetResourceStrategyError::NoSuchLink,
            },
            Err(LinkActivationError::WrongPhase) => CommandOutcome::SetResourceStrategyRejected {
                id,
                error: SetResourceStrategyError::LinkNotActive,
            },
        }
    }

    /// RNS 1.3.1 `Resource.accept`, behind the strategy gate. Refusals are
    /// silent, like a reference receiver that never accepts: the sender's
    /// advertisement simply goes unanswered. The deferred shapes — split
    /// (multi-segment), resource-as-request, metadata — are refused here
    /// too, named in order in the gate. Advertisements stay behind the
    /// duplicate filter (only `RESOURCE_REQ`/`RESOURCE`/`RESOURCE_PRF` are
    /// exempt in the reference).
    pub(crate) fn classify_resource_advertisement<'p>(
        &mut self,
        data: DataPacket<'p>,
        arrived_at: InstantMillis,
    ) -> IngestPacketOutcome<'static> {
        let link_id = LinkId::new(*data.destination.as_bytes());
        let Some(LinkPhase::Active {
            key,
            mtu,
            resource_strategy,
            ..
        }) = self.links.phase_for(&link_id)
        else {
            return IngestPacketOutcome::Ignored;
        };
        let ResourceStrategy::Accept {
            max_uncompressed_len,
            accept_compressed,
        } = *resource_strategy
        else {
            return IngestPacketOutcome::Ignored;
        };
        let mtu = *mtu;
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
        let Ok(advertisement) = ResourceAdvertisement::parse(plaintext) else {
            return IngestPacketOutcome::Ignored;
        };
        if !advertisement.flags.encrypted
            || advertisement.flags.split
            || advertisement.flags.is_request
            || advertisement.flags.has_metadata
            || advertisement.total_segments != 1
            || advertisement.hashmap.is_empty()
        {
            return IngestPacketOutcome::Ignored;
        }
        let compression = ResourceCompression::from_wire_flag(advertisement.flags.compressed);
        if compression == ResourceCompression::Bz2 && !accept_compressed {
            return IngestPacketOutcome::Ignored;
        }
        if advertisement.data_size > max_uncompressed_len {
            return IngestPacketOutcome::Ignored;
        }
        let Ok(sealed_transfer_len) = usize::try_from(advertisement.transfer_size) else {
            return IngestPacketOutcome::Ignored;
        };
        let sdu = resource_sdu(mtu);
        let part_count = sealed_transfer_len.div_ceil(sdu);
        if part_count == 0 {
            return IngestPacketOutcome::Ignored;
        }
        let accepted = AcceptedResource {
            hash: advertisement.hash,
            salt_nonce: advertisement.salt_nonce,
            compression,
            uncompressed_data_len: advertisement.data_size,
            sealed_transfer_len,
            part_count,
            sdu,
            request_id: advertisement.request_id,
            initial_names: advertisement.hashmap,
        };
        let Ok(index) = self.incoming_resources.accept(link_id, accepted) else {
            return IngestPacketOutcome::Ignored;
        };
        self.incoming_resources.state_mut(index).retries_left = MAX_RETRIES;
        self.links.note_inbound(&link_id, arrived_at);
        IngestPacketOutcome::OwesResourcePull {
            link_id,
            hash: advertisement.hash,
        }
    }

    /// RNS 1.3.1 `Resource.request_next`: scan one window of part slots from
    /// the consecutive-completed height, request every missing part whose
    /// name is known, and flag the request hashmap-exhausted — carrying the
    /// last known name — when the window runs past the names received so
    /// far. Nothing is emitted when the window holds nothing to ask for.
    pub(crate) fn emit_resource_pull<F>(
        &mut self,
        link_id: &LinkId,
        hash: &ResourceHash,
        now: InstantMillis,
        fill_entropy: &mut F,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) where
        F: FnMut(&mut [u8]),
    {
        let Some(index) = self.incoming_resources.lookup(link_id, hash) else {
            return;
        };
        let state = *self.incoming_resources.state(index);

        let mut requested = [0u8; WINDOW_MAX * MAP_HASH_LEN];
        let mut requested_count = 0;
        let mut exhausted = false;
        {
            let received = self.incoming_resources.received_flags(index);
            let names = self.incoming_resources.names_flat(index);
            let mut position = state.consecutive_completed.map_or(0, |height| height + 1);
            let mut scanned = 0;
            while position < state.part_count && scanned < state.window {
                if !received[position] {
                    if position < state.hashmap_height {
                        requested[requested_count * MAP_HASH_LEN..][..MAP_HASH_LEN]
                            .copy_from_slice(&names[position * MAP_HASH_LEN..][..MAP_HASH_LEN]);
                        requested_count += 1;
                    } else {
                        exhausted = true;
                        break;
                    }
                }
                position += 1;
                scanned += 1;
            }
        }
        if requested_count == 0 && !exhausted {
            return;
        }
        let names = self.incoming_resources.names_flat(index);
        let last = state.hashmap_height.saturating_sub(1);
        let Ok(last_known) =
            <[u8; MAP_HASH_LEN]>::try_from(&names[last * MAP_HASH_LEN..(last + 1) * MAP_HASH_LEN])
        else {
            return;
        };
        {
            let state = self.incoming_resources.state_mut(index);
            state.outstanding_part_count = requested_count;
            state.waiting_for_hmu = exhausted;
        }

        let Some(LinkPhase::Active {
            key,
            mtu,
            attached_interface,
            rtt_ms,
            ..
        }) = self.links.phase_for(link_id)
        else {
            return;
        };
        let mtu = *mtu;
        let fire_on = *attached_interface;
        let rtt_ms = *rtt_ms;
        self.incoming_resources.set_timeout_at(
            index,
            Some(part_retry_deadline(now, rtt_ms, state.retries_left)),
        );
        let mut iv = [0u8; 16];
        fill_entropy(&mut iv);
        let mut fill = |slot: &mut [u8]| -> Option<usize> {
            let mut plaintext = [0u8; 1 + MAP_HASH_LEN + 32 + WINDOW_MAX * MAP_HASH_LEN];
            let plaintext_len = write_part_request_plaintext(
                hash,
                exhausted.then_some(&last_known),
                &requested[..requested_count * MAP_HASH_LEN],
                &mut plaintext,
            )
            .ok()?;
            write_link_packet(
                link_id,
                key,
                mtu,
                WireContext::ResourceRequest,
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
}

impl<S: EngineStorage> EngineState<S> {
    /// RNS 1.3.1's link dispatch for context `RESOURCE`: a part names no
    /// transfer and carries no index, so every incoming transfer on the link
    /// tries to place it by its salted name. Exempt from duplicate filtering
    /// like the request — a resent part is byte-identical. Placement decides
    /// what is owed next: assembly when the transfer completed, the next
    /// window when the outstanding count drained (growing the window the way
    /// `receive_part` does), nothing while parts are still in flight.
    pub(crate) fn classify_resource_part<'p>(
        &mut self,
        data: DataPacket<'p>,
        arrived_at: InstantMillis,
    ) -> IngestPacketOutcome<'static> {
        let link_id = LinkId::new(*data.destination.as_bytes());
        if !matches!(
            self.links.phase_for(&link_id),
            Some(LinkPhase::Active { .. }),
        ) {
            return IngestPacketOutcome::Ignored;
        }
        let part: &[u8] = data.payload;
        let mut placed = None;
        for index in 0..self.incoming_resources.len() {
            if self.incoming_resources.link_at(index) != &link_id {
                continue;
            }
            let state = *self.incoming_resources.state(index);
            if state.status != IncomingResourceStatus::Transferring {
                continue;
            }
            let scan_from = state.consecutive_completed.unwrap_or(0);
            let at = match_part_in_window(
                part,
                &state.salt_nonce,
                self.incoming_resources.names_flat(index),
                scan_from,
                state.window,
            );
            if let Some(at) = at {
                if self.incoming_resources.place_part(index, at, part) {
                    placed = Some(index);
                }
            }
        }
        let Some(index) = placed else {
            return IngestPacketOutcome::Ignored;
        };
        self.links.note_inbound(&link_id, arrived_at);
        if let Some(LinkPhase::Active { rtt_ms, .. }) = self.links.phase_for(&link_id) {
            let rtt_ms = *rtt_ms;
            let retries_left = self.incoming_resources.state(index).retries_left;
            self.incoming_resources.set_timeout_at(
                index,
                Some(part_retry_deadline(arrived_at, rtt_ms, retries_left)),
            );
        }
        let hash = *self.incoming_resources.hash_at(index);
        let state = *self.incoming_resources.state(index);
        if state.received_part_count == state.part_count {
            return IngestPacketOutcome::OwesResourceAssembly { link_id, hash };
        }
        if state.outstanding_part_count == 0 && !state.waiting_for_hmu {
            let state = self.incoming_resources.state_mut(index);
            if state.window < state.window_max {
                state.window += 1;
                if state.window - state.window_min > WINDOW_FLEXIBILITY - 1 {
                    state.window_min += 1;
                }
            }
            return IngestPacketOutcome::OwesResourcePull { link_id, hash };
        }
        IngestPacketOutcome::Ignored
    }

    /// RNS 1.3.1 `Resource.hashmap_update_packet`: a sealed segment of names
    /// extends what the receiver can ask for, and the pull resumes. Stays
    /// behind the duplicate filter. A segment that misfits the register —
    /// past the part count, or skipping ahead of the height — cancels the
    /// transfer where the reference would crash its link thread.
    pub(crate) fn classify_resource_hashmap_update<'p>(
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
        let Ok(update) = parse_hashmap_update_plaintext(plaintext) else {
            return IngestPacketOutcome::Ignored;
        };
        let Some(index) = self.incoming_resources.lookup(&link_id, &update.hash) else {
            return IngestPacketOutcome::Ignored;
        };
        self.links.note_inbound(&link_id, arrived_at);
        match self
            .incoming_resources
            .apply_hashmap_update(index, update.segment, update.hashmap)
        {
            Ok(_) => IngestPacketOutcome::OwesResourcePull {
                link_id,
                hash: update.hash,
            },
            Err(_) => {
                self.incoming_resources.remove(&link_id, &update.hash);
                IngestPacketOutcome::ResourceConcludedFailed {
                    link_id,
                    hash: update.hash,
                }
            }
        }
    }

    /// RNS 1.3.1's link dispatch for `RESOURCE_ICL`: the sender cancelled an
    /// inbound transfer — the matching row drops and the app hears the
    /// failure. Sealed, and behind the duplicate filter like the
    /// advertisement.
    pub(crate) fn classify_resource_cancel<'p>(
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
        if !self.incoming_resources.remove(&link_id, &hash) {
            return IngestPacketOutcome::Ignored;
        }
        self.links.note_inbound(&link_id, arrived_at);
        IngestPacketOutcome::ResourceConcludedFailed { link_id, hash }
    }

    /// The closing move of RNS 1.3.1 `Resource.assemble` + `prove`: open the
    /// completed transfer in place, verify the salted hash, send the 64-byte
    /// proof back raw, journal the plaintext to the app, and retire the row.
    /// A compressed transfer stops at AwaitingDecompression instead — the
    /// host owns the inflate (the seam lands with the next slice). Corrupt
    /// assemblies retire the row and journal the failure.
    pub(crate) fn conclude_resource(
        &mut self,
        link_id: &LinkId,
        hash: &ResourceHash,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) {
        let Some(index) = self.incoming_resources.lookup(link_id, hash) else {
            return;
        };
        let state = *self.incoming_resources.state(index);
        let Some(LinkPhase::Active {
            key,
            mtu,
            attached_interface,
            ..
        }) = self.links.phase_for(link_id)
        else {
            return;
        };
        let mtu = *mtu;
        let fire_on = *attached_interface;

        if state.compression == ResourceCompression::Bz2 {
            let mut opened = false;
            {
                let transfer = self.incoming_resources.sealed_transfer_mut(index);
                if let Ok(stream) = open_transfer(key, transfer) {
                    sink(EngineReaction::Journaled(
                        Journaled::ResourceNeedsDecompression {
                            link_id: *link_id,
                            hash: *hash,
                            stream,
                            uncompressed_data_len: state.uncompressed_data_len,
                        },
                    ));
                    opened = true;
                }
            }
            if opened {
                self.incoming_resources.state_mut(index).status =
                    IncomingResourceStatus::AwaitingDecompression;
                self.incoming_resources.set_timeout_at(index, None);
            } else {
                self.incoming_resources.remove(link_id, hash);
                sink(EngineReaction::Journaled(Journaled::ResourceFailed {
                    link_id: *link_id,
                    hash: *hash,
                }));
            }
            return;
        }

        let mut delivered = false;
        {
            let transfer = self.incoming_resources.sealed_transfer_mut(index);
            if let Ok(plaintext) = open_transfer(key, transfer) {
                if let Ok(proof) = verify_and_prove(plaintext, &state.salt_nonce, hash) {
                    if let Some(prove) = proof_emission(link_id, hash, &proof, mtu) {
                        emit_proof(prove, fire_on, sink);
                        sink(EngineReaction::Journaled(Journaled::ResourceReceived {
                            link_id: *link_id,
                            hash: *hash,
                            data: plaintext,
                        }));
                        delivered = true;
                    }
                }
            }
        }
        self.incoming_resources.remove(link_id, hash);
        if !delivered {
            sink(EngineReaction::Journaled(Journaled::ResourceFailed {
                link_id: *link_id,
                hash: *hash,
            }));
        }
    }

    /// The host's answer to [`Journaled::ResourceNeedsDecompression`]: the
    /// inflated plaintext, in a buffer the host sized from the advertised
    /// length. The engine verifies it exactly like an uncompressed assembly:
    /// a wrong length, a wrong hash, or a vanished link all retire the row
    /// as failed (the host signals its own inflate failure by answering with
    /// an empty slice).
    ///
    /// A borrow-taking entry point beside the command
    /// queue, like `ingest_send_resource_into` (a mebibyte never rides an
    /// enum).
    pub fn provide_decompressed(
        &mut self,
        link_id: LinkId,
        hash: ResourceHash,
        plaintext: &[u8],
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) {
        let Some(index) = self.incoming_resources.lookup(&link_id, &hash) else {
            return;
        };
        let state = *self.incoming_resources.state(index);
        if state.status != IncomingResourceStatus::AwaitingDecompression {
            return;
        }
        self.incoming_resources.remove(&link_id, &hash);

        let verified = u64::try_from(plaintext.len()) == Ok(state.uncompressed_data_len)
            && self.links.phase_for(&link_id).is_some();
        let proven = verified
            .then(|| verify_and_prove(plaintext, &state.salt_nonce, &hash).ok())
            .flatten();
        let emission = proven.and_then(|proof| {
            let LinkPhase::Active {
                mtu,
                attached_interface,
                ..
            } = self.links.phase_for(&link_id)?
            else {
                return None;
            };
            Some((proof, *mtu, *attached_interface))
        });
        match emission {
            Some((proof, mtu, fire_on)) => {
                if let Some(prove) = proof_emission(&link_id, &hash, &proof, mtu) {
                    emit_proof(prove, fire_on, sink);
                }
                sink(EngineReaction::Journaled(Journaled::ResourceReceived {
                    link_id,
                    hash,
                    data: plaintext,
                }));
            }
            None => {
                sink(EngineReaction::Journaled(Journaled::ResourceFailed {
                    link_id,
                    hash,
                }));
            }
        }
    }
}

impl<S: EngineStorage> EngineState<S> {
    /// The receiver's half of the resource deadline lane — RNS 1.3.1's
    /// watchdog TRANSFERRING branch: a timed-out window shrinks (the window
    /// eases down, its ceiling follows), the hashmap-exhausted wait clears,
    /// and the pull goes out again until the retry budget runs dry. The
    /// deadline is the pre-eifr rtt form; the measured-rate form lands with
    /// the window dynamics. A receiver that gives up goes silent, like the
    /// reference — the sender discovers through its own watchdog.
    pub(crate) fn fire_due_incoming_resources<F>(
        &mut self,
        now: InstantMillis,
        fill_entropy: &mut F,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) where
        F: FnMut(&mut [u8]),
    {
        while let Some(index) = self.incoming_resources.due_index(now) {
            let link_id = *self.incoming_resources.link_at(index);
            let hash = *self.incoming_resources.hash_at(index);
            let state = *self.incoming_resources.state(index);
            if state.retries_left == 0 || self.links.phase_for(&link_id).is_none() {
                self.incoming_resources.remove(&link_id, &hash);
                sink(EngineReaction::Journaled(Journaled::ResourceFailed {
                    link_id,
                    hash,
                }));
                continue;
            }
            {
                let state = self.incoming_resources.state_mut(index);
                if state.window > state.window_min {
                    state.window -= 1;
                    if state.window_max > state.window_min {
                        state.window_max -= 1;
                        if (state.window_max - state.window) > (WINDOW_FLEXIBILITY - 1) {
                            state.window_max -= 1;
                        }
                    }
                }
                state.waiting_for_hmu = false;
                state.outstanding_part_count = 0;
                state.retries_left -= 1;
            }
            self.emit_resource_pull(&link_id, &hash, now, fill_entropy, sink);
        }
    }

    /// Drain both registers' due deadlines — the [`DueLane::ResourceDeadlines`]
    /// arm.
    pub fn fire_due_resource_deadlines<F>(
        &mut self,
        now: InstantMillis,
        fill_entropy: &mut F,
        sink: &mut impl FnMut(EngineReaction<'_>),
    ) -> crate::engine::WakeSchedules
    where
        F: FnMut(&mut [u8]),
    {
        self.fire_due_outgoing_resources(now, fill_entropy, sink);
        self.fire_due_incoming_resources(now, fill_entropy, sink);
        let mut wake_schedule_changes = crate::engine::WakeSchedules::UNCHANGED;
        wake_schedule_changes.resource_deadlines = self.resource_deadlines_wake();
        wake_schedule_changes
    }
}

/// RNS 1.3.1's receiver part wait in its pre-eifr rtt form: a part-timeout
/// factor of round trips, the retry grace, and half a second more for every
/// retry already spent.
fn part_retry_deadline(now: InstantMillis, rtt_ms: u64, retries_left: u8) -> InstantMillis {
    let retries_used = (MAX_RETRIES.saturating_sub(retries_left)) as u64;
    InstantMillis(
        now.0
            .saturating_add(rtt_ms.saturating_mul(PART_TIMEOUT_FACTOR))
            .saturating_add(RETRY_GRACE_MS)
            .saturating_add(retries_used.saturating_mul(PER_RETRY_DELAY_MS)),
    )
}

struct ProofEmission {
    link_id: LinkId,
    plaintext: [u8; PROOF_PLAINTEXT_LEN],
    mtu: usize,
}

fn proof_emission(
    link_id: &LinkId,
    hash: &ResourceHash,
    proof: &crate::routing::links::resources::ResourceProof,
    mtu: usize,
) -> Option<ProofEmission> {
    let mut plaintext = [0u8; PROOF_PLAINTEXT_LEN];
    write_proof_plaintext(hash, proof, &mut plaintext).ok()?;
    Some(ProofEmission {
        link_id: *link_id,
        plaintext,
        mtu,
    })
}

fn emit_proof(
    prove: ProofEmission,
    fire_on: crate::interfaces::InterfaceId,
    sink: &mut impl FnMut(EngineReaction<'_>),
) {
    let mut fill = |slot: &mut [u8]| -> Option<usize> {
        write_link_raw_packet(
            &prove.link_id,
            PacketType::Proof,
            WireContext::ResourceProof,
            prove.mtu,
            &prove.plaintext,
            slot,
        )
        .ok()
    };
    sink(EngineReaction::Directive(Directive::EmitFrame {
        target: fire_on,
        fill: &mut fill,
    }));
}

#[cfg(test)]
mod tests_support {
    use super::*;
    use crate::crypto::{
        x25519_diffie_hellman, Ed25519PublicKey, X25519PublicKey, X25519SecretKey,
    };
    use crate::engine::commands::{EngineCommand, IssuedCommand, Settlement};
    use crate::engine::test_support::{filled_frame, routable_descriptor, Cap, TEST_ENTROPY};
    use crate::engine::Journaled;
    use crate::interfaces::{InboundPacket, InterfaceId};
    use crate::routing::links::table::InitiatedLink;
    use crate::routing::links::LinkKey;
    use crate::wire::{DestinationHash, BROADCAST_MTU};

    pub(crate) fn hx(s: &str) -> std::vec::Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("valid hex"))
            .collect()
    }

    pub(crate) const LINK_ID: &str = "000102030405060708090a0b0c0d0e0f";
    pub(crate) const INITIATOR_SCALAR: &str =
        "3333333333333333333333333333333333333333333333333333333333333333";
    pub(crate) const RESPONDER_PUBLIC: &str =
        "ff2ee45601ec1b67310c7790404585ae697331eee1c1f8cf2419731c1fff3e6b";
    pub(crate) const CASE1_BZ2: &str = "425a6839314159265359cf3017f4000207918040000e6f9e002000902980000a54a7a869ea794d3227c13a1382644e09a09a1342684f213f04c09b1382704ec2684d89e04c8ab61302604d09d09d89fc5dc914e142433cc05fd0";

    pub(crate) fn link_id() -> LinkId {
        LinkId::new(hx(LINK_ID).try_into().unwrap())
    }

    pub(crate) fn link_key() -> LinkKey {
        let scalar: [u8; 32] = hx(INITIATOR_SCALAR).try_into().unwrap();
        let public: [u8; 32] = hx(RESPONDER_PUBLIC).try_into().unwrap();
        let shared = x25519_diffie_hellman(&X25519SecretKey::new(scalar), &X25519PublicKey(public));
        LinkKey::derive(&link_id(), &shared)
    }

    pub(crate) fn lane() -> InterfaceId {
        InterfaceId::new([0xEE; 16])
    }

    pub(crate) fn engine_with_active_link() -> EngineState<Cap> {
        let mut engine = EngineState::<Cap>::default();
        engine
            .links
            .track_initiated(InitiatedLink {
                link_id: link_id(),
                destination: DestinationHash::new([0x77; 16]),
                initiator_secret: X25519SecretKey::new([0x33; 32]),
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
        engine
    }

    pub(crate) fn advertisement_frame(data: &[u8], candidate: Option<&[u8]>) -> std::vec::Vec<u8> {
        let mut sender = engine_with_active_link();
        let mut frame = None;
        sender.ingest_send_resource_into(
            CommandId(7),
            link_id(),
            data,
            candidate,
            None,
            InstantMillis(1_500),
            &mut |bytes: &mut [u8]| bytes.fill(0xA5),
            &mut |reaction| {
                if let EngineReaction::Directive(Directive::EmitFrame { fill, .. }) = reaction {
                    frame = filled_frame(fill);
                }
            },
        );
        frame.expect("the sender advertises")
    }

    pub(crate) struct InboundCapture {
        pub(crate) frames: std::vec::Vec<(InterfaceId, std::vec::Vec<u8>)>,
        pub(crate) settlements: std::vec::Vec<(CommandId, Settlement)>,
        pub(crate) received: std::vec::Vec<(ResourceHash, std::vec::Vec<u8>)>,
        pub(crate) failed: std::vec::Vec<ResourceHash>,
    }

    pub(crate) fn feed(engine: &mut EngineState<Cap>, frame: &[u8], at: u64) -> InboundCapture {
        let mut capture = InboundCapture {
            frames: std::vec::Vec::new(),
            settlements: std::vec::Vec::new(),
            received: std::vec::Vec::new(),
            failed: std::vec::Vec::new(),
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
                EngineReaction::Journaled(Journaled::ResourceReceived { hash, data, .. }) => {
                    capture.received.push((hash, data.to_vec()));
                }
                EngineReaction::Journaled(Journaled::ResourceFailed { hash, .. }) => {
                    capture.failed.push(hash);
                }
                _ => {}
            },
        );
        capture
    }

    pub(crate) fn accept_everything(engine: &mut EngineState<Cap>) {
        let mut settled = std::vec::Vec::new();
        engine.ingest_command_into(
            IssuedCommand {
                id: CommandId(9),
                command: EngineCommand::SetResourceStrategy(SetResourceStrategy {
                    link_id: link_id(),
                    strategy: ResourceStrategy::Accept {
                        max_uncompressed_len: 1 << 20,
                        accept_compressed: true,
                    },
                }),
            },
            &[routable_descriptor(lane())],
            InstantMillis(1_500),
            &mut |bytes: &mut [u8]| bytes.fill(0xB1),
            &mut |reaction| {
                if let EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) =
                    reaction
                {
                    settled.push((id, settlement));
                }
            },
        );
        assert!(matches!(
            settled[0],
            (CommandId(9), Settlement::SetResourceStrategy(Ok(()))),
        ));
    }

    pub(crate) fn four_part_payload() -> std::vec::Vec<u8> {
        b"resource parts ride raw on the wire! ".repeat(41)
    }
}

#[cfg(test)]
mod tests {
    use super::tests_support::*;
    use super::*;
    use crate::engine::commands::{
        EngineCommand, IssuedCommand, SetResourceStrategyFailure, Settlement,
    };
    use crate::engine::test_support::{routable_descriptor, Cap};
    use crate::engine::Journaled;
    use crate::routing::links::resources::control::parse_part_request_plaintext;
    use crate::wire::WirePacketHeader;

    #[test]
    fn the_default_strategy_ignores_advertisements() {
        let mut receiver = engine_with_active_link();
        let capture = feed(
            &mut receiver,
            &advertisement_frame(&four_part_payload(), None),
            2_000,
        );
        assert!(capture.frames.is_empty());
        assert!(receiver.incoming_resources.is_empty());
    }

    #[test]
    fn the_strategy_command_demands_an_active_link() {
        let mut engine = EngineState::<Cap>::default();
        let mut settled = std::vec::Vec::new();
        engine.ingest_command_into(
            IssuedCommand {
                id: CommandId(9),
                command: EngineCommand::SetResourceStrategy(SetResourceStrategy {
                    link_id: link_id(),
                    strategy: ResourceStrategy::AcceptNone,
                }),
            },
            &[routable_descriptor(lane())],
            InstantMillis(1_500),
            &mut |bytes: &mut [u8]| bytes.fill(0xB1),
            &mut |reaction| {
                if let EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) =
                    reaction
                {
                    settled.push((id, settlement));
                }
            },
        );
        assert!(matches!(
            settled[0],
            (
                CommandId(9),
                Settlement::SetResourceStrategy(Err(SetResourceStrategyFailure::Rejected(
                    SetResourceStrategyError::NoSuchLink,
                ))),
            ),
        ));
    }

    #[test]
    fn an_accepted_advertisement_registers_and_pulls_the_first_window() {
        let mut receiver = engine_with_active_link();
        accept_everything(&mut receiver);
        let capture = feed(
            &mut receiver,
            &advertisement_frame(&four_part_payload(), None),
            2_000,
        );

        assert_eq!(capture.frames.len(), 1);
        let (target, frame) = &capture.frames[0];
        assert_eq!(*target, lane());
        let (header, payload) = WirePacketHeader::parse(frame).unwrap();
        assert_eq!(header.context, WireContext::ResourceRequest);
        let mut sealed = payload.to_vec();
        let opened = link_key().open_in_place(&mut sealed).unwrap();
        let request = parse_part_request_plaintext(opened).unwrap();
        assert_eq!(request.last_known_map_hash, None);

        let index = receiver
            .incoming_resources
            .lookup(&link_id(), &request.hash)
            .expect("the transfer is registered");
        let state = receiver.incoming_resources.state(index);
        assert_eq!(state.part_count, 4);
        assert_eq!(state.outstanding_part_count, 4);
        assert!(!state.waiting_for_hmu);
        assert_eq!(
            request.requested,
            receiver.incoming_resources.names_flat(index),
            "the first window asks for every part it can name",
        );
    }

    #[test]
    fn policy_refusals_are_silent() {
        let compressed = advertisement_frame(
            &b"reticulum resources ride the link ".repeat(40),
            Some(&hx(CASE1_BZ2)),
        );

        let mut no_compression = engine_with_active_link();
        let mut settled = std::vec::Vec::new();
        no_compression.ingest_command_into(
            IssuedCommand {
                id: CommandId(9),
                command: EngineCommand::SetResourceStrategy(SetResourceStrategy {
                    link_id: link_id(),
                    strategy: ResourceStrategy::Accept {
                        max_uncompressed_len: 1 << 20,
                        accept_compressed: false,
                    },
                }),
            },
            &[routable_descriptor(lane())],
            InstantMillis(1_500),
            &mut |bytes: &mut [u8]| bytes.fill(0xB1),
            &mut |reaction| {
                if let EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) =
                    reaction
                {
                    settled.push((id, settlement));
                }
            },
        );
        let capture = feed(&mut no_compression, &compressed, 2_000);
        assert!(capture.frames.is_empty());
        assert!(no_compression.incoming_resources.is_empty());

        let mut tiny_cap = engine_with_active_link();
        let mut settled = std::vec::Vec::new();
        tiny_cap.ingest_command_into(
            IssuedCommand {
                id: CommandId(9),
                command: EngineCommand::SetResourceStrategy(SetResourceStrategy {
                    link_id: link_id(),
                    strategy: ResourceStrategy::Accept {
                        max_uncompressed_len: 100,
                        accept_compressed: true,
                    },
                }),
            },
            &[routable_descriptor(lane())],
            InstantMillis(1_500),
            &mut |bytes: &mut [u8]| bytes.fill(0xB1),
            &mut |reaction| {
                if let EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) =
                    reaction
                {
                    settled.push((id, settlement));
                }
            },
        );
        let capture = feed(
            &mut tiny_cap,
            &advertisement_frame(&four_part_payload(), None),
            2_000,
        );
        assert!(capture.frames.is_empty());
        assert!(tiny_cap.incoming_resources.is_empty());
    }

    #[test]
    fn a_duplicate_advertisement_is_filtered() {
        let mut receiver = engine_with_active_link();
        accept_everything(&mut receiver);
        let advertisement = advertisement_frame(&four_part_payload(), None);
        let first = feed(&mut receiver, &advertisement, 2_000);
        let second = feed(&mut receiver, &advertisement, 2_100);
        assert_eq!(first.frames.len(), 1);
        assert!(second.frames.is_empty());
        assert_eq!(receiver.incoming_resources.len(), 1);
    }
}

#[cfg(test)]
mod loop_tests {
    use super::tests_support::*;
    use super::*;
    use crate::engine::commands::Settlement;
    use crate::engine::test_support::filled_frame;
    use crate::routing::links::data::write_link_packet;
    use crate::routing::links::resources::advertisement::write_hashmap_update_plaintext;
    use crate::routing::links::resources::control::write_part_request_plaintext;
    use crate::routing::links::resources::SaltNonce;
    use crate::wire::{PacketType as WirePacketType, WirePacketHeader, BROADCAST_MTU};

    fn eight_part_payload() -> std::vec::Vec<u8> {
        b"closing the resource loop one window at a time! ".repeat(75)
    }

    #[test]
    fn a_full_uncompressed_transfer_crosses_two_live_engines() {
        let mut sender = engine_with_active_link();
        let mut receiver = engine_with_active_link();
        accept_everything(&mut receiver);
        let data = four_part_payload();

        let mut advertisement = None;
        sender.ingest_send_resource_into(
            CommandId(7),
            link_id(),
            &data,
            None,
            None,
            InstantMillis(1_500),
            &mut |bytes: &mut [u8]| bytes.fill(0xA5),
            &mut |reaction| {
                if let EngineReaction::Directive(Directive::EmitFrame { fill, .. }) = reaction {
                    advertisement = filled_frame(fill);
                }
            },
        );

        let pull = feed(&mut receiver, &advertisement.unwrap(), 2_000);
        assert_eq!(
            pull.frames.len(),
            1,
            "the receiver asks for the first window"
        );

        let serve = feed(&mut sender, &pull.frames[0].1, 2_100);
        assert_eq!(
            serve.frames.len(),
            4,
            "the sender streams every requested part"
        );

        let mut conclusion = None;
        for (arrived, (_, part)) in serve.frames.iter().enumerate() {
            let capture = feed(&mut receiver, part, 2_200 + arrived as u64);
            if !capture.received.is_empty() || !capture.frames.is_empty() {
                conclusion = Some(capture);
            }
        }
        let conclusion = conclusion.expect("the last part concludes the transfer");
        assert_eq!(conclusion.received.len(), 1);
        assert_eq!(
            conclusion.received[0].1, data,
            "the journaled plaintext is the original payload",
        );
        assert!(
            receiver.incoming_resources.is_empty(),
            "a delivered transfer retires its row",
        );
        assert_eq!(conclusion.frames.len(), 1, "and the proof goes back");

        let settled = feed(&mut sender, &conclusion.frames[0].1, 3_000);
        assert!(matches!(
            settled.settlements[0],
            (CommandId(7), Settlement::SendResource(Ok(()))),
        ));
        assert!(sender.outgoing_resources.is_empty());
    }

    #[test]
    fn a_drained_window_grows_and_pulls_the_next_slice() {
        let mut sender = engine_with_active_link();
        let mut receiver = engine_with_active_link();
        accept_everything(&mut receiver);
        let data = eight_part_payload();

        let mut advertisement = None;
        sender.ingest_send_resource_into(
            CommandId(7),
            link_id(),
            &data,
            None,
            None,
            InstantMillis(1_500),
            &mut |bytes: &mut [u8]| bytes.fill(0xA5),
            &mut |reaction| {
                if let EngineReaction::Directive(Directive::EmitFrame { fill, .. }) = reaction {
                    advertisement = filled_frame(fill);
                }
            },
        );
        let pull = feed(&mut receiver, &advertisement.unwrap(), 2_000);
        let serve = feed(&mut sender, &pull.frames[0].1, 2_100);
        assert_eq!(serve.frames.len(), 4, "window four to start");

        let mut next_pull = None;
        for (arrived, (_, part)) in serve.frames.iter().enumerate() {
            let capture = feed(&mut receiver, part, 2_200 + arrived as u64);
            if !capture.frames.is_empty() {
                next_pull = Some(capture);
            }
        }
        let next_pull = next_pull.expect("the drained window re-pulls");

        let hash = *receiver.incoming_resources.hash_at(0);
        let index = receiver
            .incoming_resources
            .lookup(&link_id(), &hash)
            .unwrap();
        let state = receiver.incoming_resources.state(index);
        assert_eq!(state.window, 5, "an emptied window grows by one");
        assert_eq!(state.consecutive_completed, Some(3));

        let (_, request) = &next_pull.frames[0];
        let (_, payload) = WirePacketHeader::parse(request).unwrap();
        let mut sealed = payload.to_vec();
        let opened = link_key().open_in_place(&mut sealed).unwrap();
        let request =
            crate::routing::links::resources::control::parse_part_request_plaintext(opened)
                .unwrap();
        assert_eq!(
            request.requested,
            &receiver.incoming_resources.names_flat(index)[4 * MAP_HASH_LEN..8 * MAP_HASH_LEN],
            "the next pull asks for the remaining four parts",
        );
    }

    fn crafted_partial_advertisement(names: &[u8], part_count: usize) -> std::vec::Vec<u8> {
        use crate::routing::links::resources::advertisement::{
            ResourceAdvertisement, ResourceFlags,
        };
        let advertisement = ResourceAdvertisement {
            transfer_size: (part_count * 464) as u64,
            data_size: 2_700,
            part_count: part_count as u64,
            hash: ResourceHash::new([0xAB; 32]),
            salt_nonce: SaltNonce::new([0x61; 4]),
            original_hash: ResourceHash::new([0xAB; 32]),
            segment_index: 1,
            total_segments: 1,
            request_id: None,
            flags: ResourceFlags {
                encrypted: true,
                compressed: false,
                split: false,
                is_request: false,
                is_response: false,
                has_metadata: false,
            },
            hashmap: names,
        };
        let mut plaintext = [0u8; 431];
        let plaintext_len = advertisement.write(&mut plaintext).unwrap();
        let mut frame = [0u8; BROADCAST_MTU];
        let wire_len = write_link_packet(
            &link_id(),
            &link_key(),
            BROADCAST_MTU,
            WireContext::ResourceAdvertisement,
            &plaintext[..plaintext_len],
            &[0xD1; 16],
            &mut frame,
        )
        .unwrap();
        frame[..wire_len].to_vec()
    }

    fn sealed_hashmap_update(segment: u64, names: &[u8], iv: u8) -> std::vec::Vec<u8> {
        let mut plaintext = [0u8; 431];
        let plaintext_len = write_hashmap_update_plaintext(
            &ResourceHash::new([0xAB; 32]),
            segment,
            names,
            &mut plaintext,
        )
        .unwrap();
        let mut frame = [0u8; BROADCAST_MTU];
        let wire_len = write_link_packet(
            &link_id(),
            &link_key(),
            BROADCAST_MTU,
            WireContext::ResourceHashUpdate,
            &plaintext[..plaintext_len],
            &[iv; 16],
            &mut frame,
        )
        .unwrap();
        frame[..wire_len].to_vec()
    }

    fn six_names() -> std::vec::Vec<u8> {
        let mut names = std::vec::Vec::new();
        for i in 1u32..=6 {
            names.extend_from_slice(&i.to_be_bytes());
        }
        names
    }

    #[test]
    fn an_exhausted_pull_resumes_when_the_hashmap_update_lands() {
        let mut receiver = engine_with_active_link();
        accept_everything(&mut receiver);

        let names = six_names();
        let pull = feed(
            &mut receiver,
            &crafted_partial_advertisement(&names[..8], 6),
            2_000,
        );
        assert_eq!(pull.frames.len(), 1);
        let (_, payload) = WirePacketHeader::parse(&pull.frames[0].1).unwrap();
        let mut sealed = payload.to_vec();
        let opened = link_key().open_in_place(&mut sealed).unwrap();
        let request =
            crate::routing::links::resources::control::parse_part_request_plaintext(opened)
                .unwrap();
        assert_eq!(
            request.requested,
            &names[..8],
            "only two parts are nameable"
        );
        assert_eq!(
            request.last_known_map_hash,
            Some(names[4..8].try_into().unwrap()),
            "the request flags exhaustion at the last known name",
        );

        let resumed = feed(
            &mut receiver,
            &sealed_hashmap_update(0, &names, 0xD2),
            2_100,
        );
        assert_eq!(resumed.frames.len(), 1, "the pull resumes");
        let index = receiver
            .incoming_resources
            .lookup(&link_id(), &ResourceHash::new([0xAB; 32]))
            .unwrap();
        let state = receiver.incoming_resources.state(index);
        assert_eq!(state.hashmap_height, 6);
        assert!(!state.waiting_for_hmu);
    }

    #[test]
    fn a_misfit_hashmap_update_cancels_the_transfer() {
        let mut receiver = engine_with_active_link();
        accept_everything(&mut receiver);
        let names = six_names();
        feed(
            &mut receiver,
            &crafted_partial_advertisement(&names[..8], 6),
            2_000,
        );

        let cancelled = feed(
            &mut receiver,
            &sealed_hashmap_update(5, &names, 0xD3),
            2_100,
        );
        assert!(cancelled.frames.is_empty());
        assert_eq!(cancelled.failed.len(), 1);
        assert!(receiver.incoming_resources.is_empty());
    }

    #[test]
    fn a_transfer_advertised_under_a_false_hash_fails_and_never_proves() {
        use crate::routing::links::resources::advertisement::ResourceAdvertisement;

        let mut sender = engine_with_active_link();
        let mut receiver = engine_with_active_link();
        accept_everything(&mut receiver);
        let data = four_part_payload();

        let mut advertisement = None;
        sender.ingest_send_resource_into(
            CommandId(7),
            link_id(),
            &data,
            None,
            None,
            InstantMillis(1_500),
            &mut |bytes: &mut [u8]| bytes.fill(0xA5),
            &mut |reaction| {
                if let EngineReaction::Directive(Directive::EmitFrame { fill, .. }) = reaction {
                    advertisement = filled_frame(fill);
                }
            },
        );
        let advertisement = advertisement.unwrap();
        let (_, payload) = WirePacketHeader::parse(&advertisement).unwrap();
        let mut sealed = payload.to_vec();
        let opened = link_key().open_in_place(&mut sealed).unwrap();
        let genuine = ResourceAdvertisement::parse(opened).unwrap();

        let mut lying = genuine;
        let mut wrong = *genuine.hash.as_bytes();
        wrong[0] ^= 1;
        lying.hash = ResourceHash::new(wrong);
        let mut plaintext = [0u8; 431];
        let plaintext_len = lying.write(&mut plaintext).unwrap();
        let mut frame = [0u8; BROADCAST_MTU];
        let wire_len = write_link_packet(
            &link_id(),
            &link_key(),
            BROADCAST_MTU,
            WireContext::ResourceAdvertisement,
            &plaintext[..plaintext_len],
            &[0xD4; 16],
            &mut frame,
        )
        .unwrap();
        feed(&mut receiver, &frame[..wire_len], 2_000);

        let mut request_plaintext = [0u8; 337];
        let request_len = write_part_request_plaintext(
            &genuine.hash,
            None,
            genuine.hashmap,
            &mut request_plaintext,
        )
        .unwrap();
        let mut request_frame = [0u8; BROADCAST_MTU];
        let request_wire_len = write_link_packet(
            &link_id(),
            &link_key(),
            BROADCAST_MTU,
            WireContext::ResourceRequest,
            &request_plaintext[..request_len],
            &[0xD5; 16],
            &mut request_frame,
        )
        .unwrap();
        let serve = feed(&mut sender, &request_frame[..request_wire_len], 2_100);
        assert_eq!(serve.frames.len(), 4);

        let mut outcome = None;
        for (arrived, (_, part)) in serve.frames.iter().enumerate() {
            let capture = feed(&mut receiver, part, 2_200 + arrived as u64);
            if !capture.failed.is_empty() || !capture.frames.is_empty() {
                outcome = Some(capture);
            }
        }
        let outcome = outcome.expect("the last part concludes");
        assert!(outcome.frames.is_empty(), "no proof for a corrupt transfer");
        assert!(outcome.received.is_empty());
        assert_eq!(outcome.failed.len(), 1);
        assert!(receiver.incoming_resources.is_empty());

        let _ = WirePacketType::Proof;
    }
}

#[cfg(test)]
mod seam_tests {
    use super::tests_support::*;
    use super::*;
    use crate::engine::commands::Settlement;
    use crate::engine::test_support::filled_frame;
    use crate::engine::Journaled;
    use crate::routing::links::resources::table::IncomingResourceStatus;

    fn case1_plaintext() -> std::vec::Vec<u8> {
        b"reticulum resources ride the link ".repeat(40)
    }

    #[test]
    fn a_compressed_transfer_crosses_through_the_host_inflate_seam() {
        let mut sender = engine_with_active_link();
        let mut receiver = engine_with_active_link();
        accept_everything(&mut receiver);
        let plaintext = case1_plaintext();
        let candidate = hx(CASE1_BZ2);

        let mut advertisement = None;
        sender.ingest_send_resource_into(
            CommandId(7),
            link_id(),
            &plaintext,
            Some(&candidate),
            None,
            InstantMillis(1_500),
            &mut |bytes: &mut [u8]| bytes.fill(0xA5),
            &mut |reaction| {
                if let EngineReaction::Directive(Directive::EmitFrame { fill, .. }) = reaction {
                    advertisement = filled_frame(fill);
                }
            },
        );

        let pull = feed(&mut receiver, &advertisement.unwrap(), 2_000);
        let serve = feed(&mut sender, &pull.frames[0].1, 2_100);
        assert_eq!(serve.frames.len(), 1, "the compressed stream is one part");

        let mut needs = None;
        let mut raw = serve.frames[0].1.clone();
        receiver.ingest_packet_into(
            crate::interfaces::InboundPacket {
                arrived_at: InstantMillis(2_200),
                source_interface: lane(),
                bytes: &mut raw,
            },
            crate::engine::test_support::TEST_ENTROPY,
            &[crate::engine::test_support::routable_descriptor(lane())],
            InstantMillis(2_200),
            &mut |bytes: &mut [u8]| bytes.fill(0xC7),
            &mut |_: &crate::engine::ProofRequest| false,
            &mut |reaction| {
                if let EngineReaction::Journaled(Journaled::ResourceNeedsDecompression {
                    hash,
                    stream,
                    uncompressed_data_len,
                    ..
                }) = reaction
                {
                    needs = Some((hash, stream.to_vec(), uncompressed_data_len));
                }
            },
        );
        let (hash, stream, advertised_len) = needs.expect("the seam asks the host to inflate");
        assert_eq!(
            stream,
            hx(CASE1_BZ2),
            "the host receives exactly the bz2 stream the sender compressed",
        );
        assert_eq!(advertised_len, 1_360);
        let index = receiver
            .incoming_resources
            .lookup(&link_id(), &hash)
            .unwrap();
        assert_eq!(
            receiver.incoming_resources.state(index).status,
            IncomingResourceStatus::AwaitingDecompression,
        );

        let mut frames = std::vec::Vec::new();
        let mut received = std::vec::Vec::new();
        receiver.provide_decompressed(
            link_id(),
            hash,
            &plaintext,
            &mut |reaction| match reaction {
                EngineReaction::Directive(Directive::EmitFrame { fill, .. }) => {
                    if let Some(frame) = filled_frame(fill) {
                        frames.push(frame);
                    }
                }
                EngineReaction::Journaled(Journaled::ResourceReceived { data, .. }) => {
                    received.push(data.to_vec());
                }
                _ => {}
            },
        );
        assert_eq!(received.len(), 1);
        assert_eq!(received[0], plaintext);
        assert!(receiver.incoming_resources.is_empty());
        assert_eq!(frames.len(), 1, "the proof rides back");

        let settled = feed(&mut sender, &frames[0], 3_000);
        assert!(matches!(
            settled.settlements[0],
            (CommandId(7), Settlement::SendResource(Ok(()))),
        ));
        assert!(sender.outgoing_resources.is_empty());
    }

    #[test]
    fn a_wrong_or_empty_inflate_fails_the_transfer_by_hash() {
        let mut sender = engine_with_active_link();
        let mut receiver = engine_with_active_link();
        accept_everything(&mut receiver);
        let plaintext = case1_plaintext();
        let candidate = hx(CASE1_BZ2);

        let mut advertisement = None;
        sender.ingest_send_resource_into(
            CommandId(7),
            link_id(),
            &plaintext,
            Some(&candidate),
            None,
            InstantMillis(1_500),
            &mut |bytes: &mut [u8]| bytes.fill(0xA5),
            &mut |reaction| {
                if let EngineReaction::Directive(Directive::EmitFrame { fill, .. }) = reaction {
                    advertisement = filled_frame(fill);
                }
            },
        );
        let pull = feed(&mut receiver, &advertisement.unwrap(), 2_000);
        let serve = feed(&mut sender, &pull.frames[0].1, 2_100);
        let conclusion = feed(&mut receiver, &serve.frames[0].1, 2_200);
        assert!(conclusion.received.is_empty());
        let hash = *receiver.incoming_resources.hash_at(0);

        let mut corrupted = plaintext.clone();
        corrupted[0] ^= 1;
        let mut failed = std::vec::Vec::new();
        let mut frames = 0usize;
        receiver.provide_decompressed(
            link_id(),
            hash,
            &corrupted,
            &mut |reaction| match reaction {
                EngineReaction::Journaled(Journaled::ResourceFailed { hash, .. }) => {
                    failed.push(hash);
                }
                EngineReaction::Directive(_) => frames += 1,
                _ => {}
            },
        );
        assert_eq!(failed.len(), 1);
        assert_eq!(frames, 0, "a failed inflate proves nothing");
        assert!(receiver.incoming_resources.is_empty());

        receiver.provide_decompressed(link_id(), hash, &plaintext, &mut |_| {
            panic!("a retired transfer answers nothing");
        });
    }

    #[test]
    fn the_seam_only_answers_transfers_awaiting_decompression() {
        let mut receiver = engine_with_active_link();
        accept_everything(&mut receiver);
        let mut touched = false;
        receiver.provide_decompressed(
            link_id(),
            ResourceHash::new([0x42; 32]),
            b"anything",
            &mut |_| touched = true,
        );
        assert!(!touched, "an unknown transfer answers nothing");
    }
}

#[cfg(test)]
mod cancel_tests {
    use super::tests_support::*;
    use super::*;
    use crate::engine::commands::{SendResourceFailure, Settlement};
    use crate::engine::test_support::filled_frame;
    use crate::routing::links::data::write_link_packet;
    use crate::routing::links::resources::control::write_cancel_plaintext;
    use crate::routing::links::resources::RESOURCE_HASH_LEN;
    use crate::wire::BROADCAST_MTU;

    fn four_part_setup() -> (EngineState<crate::engine::test_support::Cap>, EngineState<crate::engine::test_support::Cap>, ResourceHash) {
        let mut sender = engine_with_active_link();
        let mut receiver = engine_with_active_link();
        accept_everything(&mut receiver);
        let data = four_part_payload();
        let mut advertisement = None;
        sender.ingest_send_resource_into(
            CommandId(7),
            link_id(),
            &data,
            None,
            None,
            InstantMillis(1_500),
            &mut |bytes: &mut [u8]| bytes.fill(0xA5),
            &mut |reaction| {
                if let EngineReaction::Directive(Directive::EmitFrame { fill, .. }) = reaction {
                    advertisement = filled_frame(fill);
                }
            },
        );
        feed(&mut receiver, &advertisement.unwrap(), 2_000);
        let hash = *receiver.incoming_resources.hash_at(0);
        (sender, receiver, hash)
    }

    fn sealed_cancel(hash: &ResourceHash, context: WireContext, iv: u8) -> std::vec::Vec<u8> {
        let mut plaintext = [0u8; RESOURCE_HASH_LEN];
        write_cancel_plaintext(hash, &mut plaintext).unwrap();
        let mut frame = [0u8; BROADCAST_MTU];
        let wire_len = write_link_packet(
            &link_id(),
            &link_key(),
            BROADCAST_MTU,
            context,
            &plaintext,
            &[iv; 16],
            &mut frame,
        )
        .unwrap();
        frame[..wire_len].to_vec()
    }

    #[test]
    fn the_senders_cancel_drops_the_receivers_transfer() {
        let (_, mut receiver, hash) = four_part_setup();
        let cancelled = feed(
            &mut receiver,
            &sealed_cancel(&hash, WireContext::ResourceInitiatorCancel, 0xE1),
            2_500,
        );
        assert_eq!(cancelled.failed.len(), 1);
        assert!(receiver.incoming_resources.is_empty());

        let again = feed(
            &mut receiver,
            &sealed_cancel(&hash, WireContext::ResourceInitiatorCancel, 0xE2),
            2_600,
        );
        assert!(again.failed.is_empty(), "a cancel for nothing journals nothing");
    }

    #[test]
    fn the_receivers_reject_settles_the_send_by_its_name() {
        let (mut sender, _, hash) = four_part_setup();
        let rejected = feed(
            &mut sender,
            &sealed_cancel(&hash, WireContext::ResourceReceiverCancel, 0xE3),
            2_500,
        );
        assert!(matches!(
            rejected.settlements[0],
            (
                CommandId(7),
                Settlement::SendResource(Err(SendResourceFailure::RejectedByPeer)),
            ),
        ));
        assert!(sender.outgoing_resources.is_empty());

        let unknown = feed(
            &mut sender,
            &sealed_cancel(&ResourceHash::new([0x5A; 32]), WireContext::ResourceReceiverCancel, 0xE4),
            2_600,
        );
        assert!(unknown.settlements.is_empty());
    }
}

#[cfg(test)]
mod watchdog_tests {
    use super::tests_support::*;
    use super::*;
    use crate::engine::test_support::filled_frame;
    use crate::engine::{Journaled, LaneWake};

    struct WatchCapture {
        frames: usize,
        failed: usize,
    }

    fn fire(engine: &mut EngineState<crate::engine::test_support::Cap>, at: u64) -> WatchCapture {
        let mut capture = WatchCapture {
            frames: 0,
            failed: 0,
        };
        engine.fire_due_resource_deadlines(
            InstantMillis(at),
            &mut |bytes: &mut [u8]| bytes.fill(0xF2),
            &mut |reaction| match reaction {
                EngineReaction::Directive(Directive::EmitFrame { fill, .. }) => {
                    if filled_frame(fill).is_some() {
                        capture.frames += 1;
                    }
                }
                EngineReaction::Journaled(Journaled::ResourceFailed { .. }) => {
                    capture.failed += 1;
                }
                _ => {}
            },
        );
        capture
    }

    #[test]
    fn a_starved_pull_shrinks_its_window_and_asks_again() {
        let mut receiver = engine_with_active_link();
        accept_everything(&mut receiver);
        let pull = feed(&mut receiver, &advertisement_frame(&four_part_payload(), None), 2_000);
        assert_eq!(pull.frames.len(), 1);
        assert_eq!(
            receiver.resource_deadlines_wake(),
            LaneWake::At(InstantMillis(2_000 + 250 * 4 + 250)),
            "the pull arms the part-timeout rtt form",
        );

        let retried = fire(&mut receiver, 3_250);
        assert_eq!(retried.frames, 1, "the pull goes out again");
        let hash = *receiver.incoming_resources.hash_at(0);
        let index = receiver.incoming_resources.lookup(&link_id(), &hash).unwrap();
        let state = receiver.incoming_resources.state(index);
        assert_eq!(state.window, 3, "the window eases down");
        assert_eq!(state.window_max, 8, "and its ceiling follows twice");
        assert_eq!(state.retries_left, 15);
        assert!(
            receiver.resource_deadlines_wake()
                == LaneWake::At(InstantMillis(3_250 + 250 * 4 + 250 + 500)),
            "the next deadline stretches by one per-retry delay",
        );
    }

    #[test]
    fn a_receiver_out_of_retries_goes_silent_and_fails() {
        let mut receiver = engine_with_active_link();
        accept_everything(&mut receiver);
        feed(&mut receiver, &advertisement_frame(&four_part_payload(), None), 2_000);
        let hash = *receiver.incoming_resources.hash_at(0);
        let index = receiver.incoming_resources.lookup(&link_id(), &hash).unwrap();
        receiver.incoming_resources.state_mut(index).retries_left = 0;

        let gave_up = fire(&mut receiver, 3_250);
        assert_eq!(gave_up.frames, 0, "giving up sends nothing, like the reference");
        assert_eq!(gave_up.failed, 1);
        assert!(receiver.incoming_resources.is_empty());
        assert_eq!(receiver.resource_deadlines_wake(), LaneWake::Idle);
    }
}
