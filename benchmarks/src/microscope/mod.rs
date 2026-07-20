mod cycle;
mod forward;
mod resource;

use personal_rns::engine::{
    AnnounceAppData, AnnounceNow, AnnounceTarget, CommandId, Directive, EngineCommand,
    EngineReaction, EngineState, EstablishLink, IngestIo, InstantMillis, IssuedCommand, Journaled,
    LinkEstablished, RatchetPolicy, SendSinglePacket, SendSinglePacketPayload, Settlement,
};
use personal_rns::identity::{Zeroizing, IDENTITY_SECRET_KEY_LEN};
use personal_rns::interfaces::tcp;
use personal_rns::interfaces::AttachedInterfaces;
use personal_rns::interfaces::{InboundPacket, InterfaceDescriptor, InterfaceId};
use personal_rns::reactor::interface_seam::MAX_WIRE_FRAME_LEN;
use personal_rns::routing::announce::defaults::DEFAULT_REBROADCAST_JITTER_WINDOW_MS;
use personal_rns::routing::delivery::Delivery;
use personal_rns::routing::links::resources::{ResourceStrategy, MAX_EFFICIENT_SIZE};
use personal_rns::routing::links::LinkId;
use personal_rns::routing::{LinkRequestPolicy, ProofStrategy};
use personal_rns::storage::GrowableHeap;
use personal_rns::wire::{DestinationHash, WireContext, WirePacketHeader};
use std::time::{Duration, Instant};

const WIRE: InterfaceId = InterfaceId::new([0xC7; 8]);
const NOW: InstantMillis = InstantMillis(1_000);
pub const PAYLOAD_LEN: usize = 300;
pub const RESOURCE_PAYLOAD_LEN: usize = 1024 * 1024 - 1;

const IF_UP: InterfaceId = InterfaceId::new([0xA1; 8]);
const IF_DOWN: InterfaceId = InterfaceId::new([0xD0; 8]);
const SETUP_NOW: InstantMillis = InstantMillis(1_000);
const REBROADCAST_NOW: InstantMillis =
    InstantMillis(1_000 + DEFAULT_REBROADCAST_JITTER_WINDOW_MS + 1);
const FORWARD_NOW: InstantMillis = InstantMillis(2_000);

/// Deterministic entropy (splitmix64): every run pulls the identical stream, so a
/// measured difference between runs is the code, never the keys.
struct Splitmix(u64);

impl Splitmix {
    fn fill(&mut self, bytes: &mut [u8]) {
        for chunk in bytes.chunks_mut(8) {
            self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut word = self.0;
            word = (word ^ (word >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            word = (word ^ (word >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            word ^= word >> 31;
            chunk.copy_from_slice(&word.to_le_bytes()[..chunk.len()]);
        }
    }
}

#[derive(Default)]
struct FeedCapture {
    frames: Vec<Vec<u8>>,
    settlements: Vec<(CommandId, Settlement)>,
    announce_heard: bool,
    link_established: Option<LinkEstablished>,
    resource_received: bool,
}

impl FeedCapture {
    fn absorb(&mut self, reaction: EngineReaction<'_>, scratch: &mut Vec<u8>) {
        match reaction {
            EngineReaction::Directive(Directive::Send { bytes, .. })
            | EngineReaction::Directive(Directive::SendAnnounce { bytes, .. }) => {
                self.frames.push(bytes.to_vec());
            }
            EngineReaction::Directive(Directive::EmitFrame { fill, .. }) => {
                scratch.resize(MAX_WIRE_FRAME_LEN, 0);
                if let Some(n) = fill(scratch.as_mut_slice()) {
                    self.frames.push(scratch[..n].to_vec());
                }
            }
            EngineReaction::Journaled(Journaled::CommandSettled { id, settlement }) => {
                self.settlements.push((id, settlement));
            }
            EngineReaction::Journaled(Journaled::LinkEstablished(established)) => {
                self.link_established = Some(established);
            }
            EngineReaction::Journaled(Journaled::AnnounceHeard { .. }) => {
                self.announce_heard = true;
            }
            EngineReaction::Journaled(Journaled::ResourceReceived { .. }) => {
                self.resource_received = true;
            }
            _ => {}
        }
    }
}

fn frame_context(frame: &[u8]) -> Option<WireContext> {
    WirePacketHeader::parse(frame)
        .ok()
        .map(|(header, _)| header.context)
}

pub use cycle::Cycle;
pub use forward::Forward;
pub use resource::{ResourceCycle, ResourceTransferProfile};
