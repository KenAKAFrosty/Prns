pub mod registry;
mod synthesize;

#[cfg(feature = "alloc")]
pub use registry::HeapTunnelTable;
pub use registry::{
    FixedTunnelTable, PersistedTunnelRow, SeedTunnelOutcome, TunnelTable, TunnelTransition,
    Tunnels, TUNNEL_TIMEOUT_MS,
};
#[allow(deprecated)]
pub use synthesize::{
    assemble_synthesize_payload, compute_tunnel_id, parse_synthesize_payload,
    prepare_synthesize_verify, synthesize_signed_region, write_synthesize_wire_packet, TunnelId,
    TunnelSynthesizeVerification, TunnelSynthesizeVerifyOwed, VerifiedSynthesize,
    INTERFACE_HASH_LEN, PUBLIC_KEY_LEN, RANDOM_HASH_LEN, SIGNATURE_BYTE_LEN, SIGNED_REGION_LEN,
    SYNTHESIZE_PAYLOAD_LEN, TUNNEL_SYNTHESIZE_DESTINATION,
};
