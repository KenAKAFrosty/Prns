mod impls;

pub use impls::*;

use crate::crypto::sha256_chunks;
use crate::wire::{
    DestinationHash, DestinationType, PacketType, WireContext, WireError, TRUNCATED_HASH_BYTE_LEN,
};

pub const PACKET_HASH_LEN: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PacketHash([u8; PACKET_HASH_LEN]);

impl PacketHash {
    pub const fn new(bytes: [u8; PACKET_HASH_LEN]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; PACKET_HASH_LEN] {
        &self.0
    }

    /// RNS 1.3.1 `Packet.get_hashable_part`: a packet's identity is the flag
    /// bits that name what it is (destination type + packet type), the
    /// destination, context, and payload (never the hops count or the
    /// in-transport id), so the hash survives relaying unchanged.
    pub fn of_wire_packet(bytes: &[u8]) -> Result<Self, WireError> {
        const HASHED_FLAG_BITS: u8 = 0b0000_1111;
        const TYPE_2_BIT: u8 = 0b0100_0000;

        let flags = *bytes.first().ok_or(WireError::BufferTooShort)?;
        let after_hops = 2;
        let hashable_from = if flags & TYPE_2_BIT == TYPE_2_BIT {
            after_hops + TRUNCATED_HASH_BYTE_LEN
        } else {
            after_hops
        };
        let tail = bytes
            .get(hashable_from..)
            .ok_or(WireError::BufferTooShort)?;

        Ok(Self(sha256_chunks(&[&[flags & HASHED_FLAG_BITS], tail])))
    }

    /// RNS 1.3.1 `ProofDestination`: a proof of receipt is addressed to the
    /// first [`TRUNCATED_HASH_BYTE_LEN`] bytes of the proved packet's hash.
    /// The sender derives the same address from its own copy and matches the
    /// proof to its receipt.
    pub fn proof_destination(&self) -> DestinationHash {
        let mut bytes = [0u8; TRUNCATED_HASH_BYTE_LEN];
        bytes.copy_from_slice(&self.0[..TRUNCATED_HASH_BYTE_LEN]);
        DestinationHash::new(bytes)
    }

    /// The same hash as [`Self::of_wire_packet`], reconstructed from a data
    /// packet's typed fields (what the engine holds after classification),
    /// with the wire buffer already carved up.
    pub fn of_data_fields(
        destination_type: DestinationType,
        destination: &DestinationHash,
        context: WireContext,
        payload: &[u8],
    ) -> Self {
        let hashed_flags = ((destination_type as u8) << 2) | (PacketType::Data as u8);
        Self(sha256_chunks(&[
            &[hashed_flags],
            destination.as_bytes(),
            &[context.to_byte()],
            payload,
        ]))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RememberPacketOutcome {
    AlreadyKnown,
    StoredFresh,
    StoredAfterRotation,
}

/// RNS 1.3.1 `Transport.packet_hashlist` semantics: two generations of seen
/// packet hashes. Remembering into a full current generation rotates: the
/// current set becomes the previous, the oldest generation is forgotten.
/// `contains` answers across both.
pub trait PacketHashHistory {
    fn generation_capacity(&self) -> usize;
    fn len(&self) -> usize;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn contains(&self, hash: &PacketHash) -> bool;
    fn remember(&mut self, hash: PacketHash) -> RememberPacketOutcome;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hx<const N: usize>(s: &str) -> [u8; N] {
        let mut out = [0u8; N];
        for (i, byte) in out.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).expect("valid hex");
        }
        out
    }

    fn raw(hex: &str) -> std::vec::Vec<u8> {
        (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).expect("valid hex"))
            .collect()
    }

    const RAW_PLAIN_DATA: &str = "080012f815e3e65add6ceb2fda0e7be338680068656c6c6f2d706c61696e";

    const RAW_TYPE_2_DATA: &str =
        "48004cd0cc45a7405dbd5cf9b5be1ef92f1012f815e3e65add6ceb2fda0e7be3386800ee";

    #[test]
    fn packet_hash_matches_the_rns_1_3_1_vectors() {
        assert_eq!(
            PacketHash::of_wire_packet(&raw(RAW_PLAIN_DATA)),
            Ok(PacketHash::new(hx(
                "2cab2ae2659f871fecf8f8da596c7f6369e5e8efcd9094d2f119dddec5704716"
            ))),
        );
        assert_eq!(
            PacketHash::of_wire_packet(&raw(RAW_TYPE_2_DATA)),
            Ok(PacketHash::new(hx(
                "211f3da55c2c402e74645188c5e86fa9e2caaf0bde1a132ec8fd29eb4b38aa67"
            ))),
        );
    }

    #[test]
    fn field_wise_hashing_equals_wire_hashing() {
        for packet in [RAW_PLAIN_DATA, RAW_TYPE_2_DATA] {
            let bytes = raw(packet);
            let from_wire = PacketHash::of_wire_packet(&bytes).unwrap();

            let type_2 = bytes[0] & 0b0100_0000 != 0;
            let destination_at = if type_2 { 18 } else { 2 };
            let destination = crate::wire::DestinationHash::from_slice(
                &bytes[destination_at..destination_at + 16],
            )
            .unwrap();
            let destination_type = match (bytes[0] >> 2) & 0b11 {
                0b00 => DestinationType::Single,
                0b10 => DestinationType::Plain,
                other => panic!("unexpected destination type bits {other:02b}"),
            };
            let payload = &bytes[destination_at + 17..];

            assert_eq!(
                PacketHash::of_data_fields(
                    destination_type,
                    &destination,
                    WireContext::None,
                    payload,
                ),
                from_wire,
            );
        }
    }

    #[test]
    fn the_hops_count_never_changes_a_packets_hash() {
        let mut relayed = raw(RAW_PLAIN_DATA);
        relayed[1] = 77;
        assert_eq!(
            PacketHash::of_wire_packet(&relayed),
            PacketHash::of_wire_packet(&raw(RAW_PLAIN_DATA)),
        );
    }

    #[test]
    fn the_transport_id_never_changes_a_packets_hash() {
        let type_1 = raw(&format!(
            "0800{}{}",
            "12f815e3e65add6ceb2fda0e7be33868", "00ee"
        ));
        let type_2 = raw(RAW_TYPE_2_DATA);
        assert_eq!(
            PacketHash::of_wire_packet(&type_1),
            PacketHash::of_wire_packet(&type_2),
        );
    }

    #[test]
    fn unhashed_flag_bits_never_change_a_packets_hash() {
        let baseline = PacketHash::of_wire_packet(&raw(RAW_PLAIN_DATA));
        for high_bit in [0b1000_0000u8, 0b0010_0000, 0b0001_0000] {
            let mut flipped = raw(RAW_PLAIN_DATA);
            flipped[0] |= high_bit;
            assert_eq!(PacketHash::of_wire_packet(&flipped), baseline);
        }
    }

    #[test]
    fn distinct_payloads_hash_distinctly() {
        let mut other_payload = raw(RAW_PLAIN_DATA);
        let last = other_payload.len() - 1;
        other_payload[last] ^= 0xFF;
        assert_ne!(
            PacketHash::of_wire_packet(&other_payload),
            PacketHash::of_wire_packet(&raw(RAW_PLAIN_DATA)),
        );
    }

    #[test]
    fn truncated_packets_are_unhashable() {
        assert_eq!(
            PacketHash::of_wire_packet(&[]),
            Err(WireError::BufferTooShort),
        );
        let mut type_2_too_short = raw(RAW_TYPE_2_DATA);
        type_2_too_short.truncate(10);
        assert_eq!(
            PacketHash::of_wire_packet(&type_2_too_short),
            Err(WireError::BufferTooShort),
        );
    }
}
