pub const TRUNCATED_HASH_BYTE_LEN: usize = 16;
pub const ANNOUNCE_PUBLIC_KEY_BYTE_LEN: usize = 64;
pub const DOTTED_NAME_HASH_BYTE_LEN: usize = 10;
pub const RATCHET_BYTE_LEN: usize = 32;
pub const SIGNATURE_BYTE_LEN: usize = 64;

/// RNS 1.4.2 `Reticulum.MTU`: the maximum byte size of one packet on the broadcast plane (announces, path requests, un-linked Singles), which peers must agree on. A wire-protocol invariant, permanently 500 even on fat pipes; the per-link MTU a LINKREQUEST negotiates is separate.
pub const BROADCAST_MTU: usize = 500;
/// RNS 1.4.2 `Transport.PATHFINDER_M`: packets beyond this hop count are outside reach. A wire-protocol invariant, not a sizing knob.
pub const MAX_HOP_COUNT: u8 = 128;

/// Whether an inbound packet's on-wire hop field remains below the RNS 1.4.2
/// `PATHFINDER_M` rejection boundary. A packet received with 127 wire hops
/// reaches the 128-hop boundary after ingress; 128 on the wire is already out
/// of reach.
#[must_use]
pub const fn wire_hop_count_is_valid(hops: u8) -> bool {
    hops < MAX_HOP_COUNT
}

/// The type-1 (direct, no transport id) header: `flags, hops, destination, context`
pub const HEADER_MIN_LEN: usize = 2 + TRUNCATED_HASH_BYTE_LEN + 1;
/// RNS 1.4.2 `Reticulum.HEADER_MAXSIZE`. The type-2 (transport-routed) header: `flags, hops, transport id, destination, context`. Outbound payload budgets reserve this even when emitting type-1, because a relay re-emits the packet with the transport id added.
pub const HEADER_MAX_LEN: usize = 2 + TRUNCATED_HASH_BYTE_LEN * 2 + 1;
/// RNS 1.4.2 `Reticulum.IFAC_MIN_SIZE`: the smallest per-interface access-code overhead a packet may gain; reserved in every payload budget like the transport header.
pub const IFAC_MIN_LEN: usize = 1;
/// RNS 1.4.2 `Reticulum.MDU`: the most payload bytes one packet may carry once the worst-case header and minimum IFAC are reserved.
pub const BROADCAST_MDU: usize = BROADCAST_MTU - HEADER_MAX_LEN - IFAC_MIN_LEN;
