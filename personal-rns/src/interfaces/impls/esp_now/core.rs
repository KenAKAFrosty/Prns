//! Platform-agnostic part of the ESP-NOW interface: its routing descriptor and
//! the on-air frame codec.
//!
//! ESP-NOW is a Personal-native broadcast medium with no stock-RNS counterpart —
//! a Hopspot-to-Hopspot link over the 2.4 GHz radio. Both ends are ours, so the
//! frame format is ours. A v2 ESP-NOW frame carries up to
//! [`ESP_NOW_MAX_FRAME_PAYLOAD`] bytes (1470) — far above Reticulum's 500-byte
//! [`MTU`](crate::wire::MTU) — so a whole packet always fits one frame and we
//! never fragment.
//!
//! Instead we spend that headroom the other way: **coalescing**. Because several
//! Reticulum packets fit one frame, the worker packs as many as are queued (on a
//! short timer) into one frame and transmits once, amortizing the fixed
//! per-transmission radio overhead. A frame is a one-byte version tag followed by
//! length-delimited packets: [`EspNowFrameWriter`] packs them on the way out,
//! [`decode_frame`] walks them back out on the way in. Reassembly is unnecessary —
//! un-coalescing one received frame yields N whole packets, each handed to the
//! engine as-is.

use crate::interfaces::{
    Capabilities, ConnectionState, InterfaceDescriptor, InterfaceId, InterfaceMode, MediumKind,
};

/// The one-byte tag at the front of every frame. Bumped if the coalescing frame
/// format ever changes, so a peer can reject a frame it can't parse instead of
/// mis-reading it. There is no back-compat to preserve — every node is ours.
pub const ESP_NOW_FRAME_VERSION: u8 = 1;

/// The largest payload one ESP-NOW frame carries: ESP-NOW v2's
/// `ESP_NOW_MAX_DATA_LEN_V2` (1470 bytes). Well above Reticulum's 500-byte MTU,
/// which is what makes coalescing — rather than fragmentation — the design.
pub const ESP_NOW_MAX_FRAME_PAYLOAD: usize = 1470;

/// The one-byte version tag that opens a frame.
pub const ESP_NOW_FRAME_HEADER_LEN: usize = 1;

/// Each coalesced packet is prefixed with its length as a big-endian `u16`.
pub const ESP_NOW_LENGTH_PREFIX_LEN: usize = 2;

/// Packs Reticulum packets into one ESP-NOW frame, length-delimited under the
/// version tag, until the frame is full. The worker drives it: open a frame,
/// [`try_push`](Self::try_push) queued packets while they fit, then transmit
/// [`frame`](Self::frame) once. A packet that does not fit is held by the caller
/// for the next frame, so a burst of small packets uses far fewer transmissions.
pub struct EspNowFrameWriter<'a> {
    buf: &'a mut [u8],
    len: usize,
    packet_count: usize,
}

impl<'a> EspNowFrameWriter<'a> {
    /// Begin a frame in `buf`, writing the version tag. `buf` must be non-empty
    /// (it always holds the header); a real frame buffer is sized to
    /// [`ESP_NOW_MAX_FRAME_PAYLOAD`].
    pub fn new(buf: &'a mut [u8]) -> Self {
        buf[0] = ESP_NOW_FRAME_VERSION;
        Self {
            buf,
            len: ESP_NOW_FRAME_HEADER_LEN,
            packet_count: 0,
        }
    }

    /// Append one packet as `[u16 len][packet]` if the frame's remaining space
    /// holds it. Returns `true` if it was packed, `false` if it did not fit (the
    /// frame is full, or the packet is larger than a length prefix can express) —
    /// the caller keeps the packet for the next frame.
    pub fn try_push(&mut self, packet: &[u8]) -> bool {
        if packet.len() > u16::MAX as usize {
            return false;
        }
        let need = ESP_NOW_LENGTH_PREFIX_LEN + packet.len();
        if self.len + need > self.buf.len() {
            return false;
        }
        self.buf[self.len..self.len + ESP_NOW_LENGTH_PREFIX_LEN]
            .copy_from_slice(&(packet.len() as u16).to_be_bytes());
        self.len += ESP_NOW_LENGTH_PREFIX_LEN;
        self.buf[self.len..self.len + packet.len()].copy_from_slice(packet);
        self.len += packet.len();
        self.packet_count += 1;
        true
    }

    /// How many packets have been coalesced into this frame so far.
    pub fn packet_count(&self) -> usize {
        self.packet_count
    }

    /// Whether no packet has been packed yet (only the version tag is present).
    pub fn is_empty(&self) -> bool {
        self.packet_count == 0
    }

    /// The framed bytes to transmit: the version tag plus every packed packet.
    pub fn frame(&self) -> &[u8] {
        &self.buf[..self.len]
    }
}

/// Why a received frame couldn't be opened for reading.
#[derive(Debug, PartialEq, Eq)]
pub enum FrameDecodeError {
    /// The frame had no version tag (it was empty).
    Empty,
    /// The version tag didn't match [`ESP_NOW_FRAME_VERSION`] — a frame from an
    /// incompatible format; the carried byte is returned for diagnostics.
    UnknownVersion(u8),
}

/// Validate a received frame's version tag and return an iterator over the
/// packets coalesced into it. A trailing truncated record — a length prefix or
/// packet body running past the frame's end — ends iteration cleanly: a corrupt
/// broadcast frame yields the packets that parsed and then stops, since the
/// engine validates each packet downstream regardless.
pub fn decode_frame(frame: &[u8]) -> Result<EspNowFrameReader<'_>, FrameDecodeError> {
    let (&version, rest) = frame.split_first().ok_or(FrameDecodeError::Empty)?;
    if version != ESP_NOW_FRAME_VERSION {
        return Err(FrameDecodeError::UnknownVersion(version));
    }
    Ok(EspNowFrameReader { rest })
}

/// Iterator over the packets a received frame coalesced — each yielded item is
/// one whole Reticulum packet, in the order it was packed. Built by
/// [`decode_frame`].
pub struct EspNowFrameReader<'a> {
    rest: &'a [u8],
}

impl<'a> Iterator for EspNowFrameReader<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<&'a [u8]> {
        if self.rest.len() < ESP_NOW_LENGTH_PREFIX_LEN {
            return None;
        }
        let (len_bytes, after_len) = self.rest.split_at(ESP_NOW_LENGTH_PREFIX_LEN);
        let packet_len = u16::from_be_bytes([len_bytes[0], len_bytes[1]]) as usize;
        if after_len.len() < packet_len {
            // Truncated record — stop rather than yield a partial packet.
            self.rest = &[];
            return None;
        }
        let (packet, tail) = after_len.split_at(packet_len);
        self.rest = tail;
        Some(packet)
    }
}

/// The routing facts an ESP-NOW interface registers: a shared half-duplex
/// broadcast medium where every neighbor hears every transmission and the node
/// repeats into it, participating fully in transport — the same medium shape as
/// LoRa, on a different radio. Reported `Connected` once the radio is up; a
/// broadcast medium has no per-peer link state.
pub fn descriptor(id: InterfaceId) -> InterfaceDescriptor {
    InterfaceDescriptor {
        id,
        capabilities: Capabilities {
            receives: true,
            transmits: true,
            forwards: true,
            // Every neighbor hears every broadcast, so the node rebroadcasts
            // announces back into the same air.
            repeats: true,
        },
        mode: InterfaceMode::Full,
        medium: MediumKind::SharedHalfDuplex,
        state: ConnectionState::Connected,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_packet_round_trips_under_the_version_tag() {
        let packet = [0xDE, 0xAD, 0xBE, 0xEF, 0x00, 0x7E];
        let mut buf = [0u8; ESP_NOW_MAX_FRAME_PAYLOAD];
        let mut w = EspNowFrameWriter::new(&mut buf);
        assert!(w.try_push(&packet));
        assert_eq!(w.packet_count(), 1);
        assert!(!w.is_empty());
        let frame = w.frame();
        assert_eq!(frame[0], ESP_NOW_FRAME_VERSION);

        let mut reader = decode_frame(frame).unwrap();
        assert_eq!(reader.next(), Some(&packet[..]));
        assert_eq!(reader.next(), None);
    }

    #[test]
    fn coalesces_several_packets_and_reads_them_back_in_order() {
        let a = [1u8, 2, 3];
        let b = [9u8; 250];
        let c = [0xAB, 0xCD];
        let mut buf = [0u8; ESP_NOW_MAX_FRAME_PAYLOAD];
        let mut w = EspNowFrameWriter::new(&mut buf);
        assert!(w.try_push(&a));
        assert!(w.try_push(&b));
        assert!(w.try_push(&c));
        assert_eq!(w.packet_count(), 3);

        let mut reader = decode_frame(w.frame()).unwrap();
        assert_eq!(reader.next(), Some(&a[..]));
        assert_eq!(reader.next(), Some(&b[..]));
        assert_eq!(reader.next(), Some(&c[..]));
        assert_eq!(reader.next(), None);
    }

    #[test]
    fn the_fat_v2_frame_coalesces_at_least_two_full_mtu_packets() {
        // The whole point of v2: a 500-byte MTU packet is small next to a 1470 B
        // frame, so a burst of full packets still fits in one transmission.
        let mtu_packet = [0x5Au8; crate::wire::MTU];
        let mut buf = [0u8; ESP_NOW_MAX_FRAME_PAYLOAD];
        let mut w = EspNowFrameWriter::new(&mut buf);
        assert!(w.try_push(&mtu_packet));
        assert!(w.try_push(&mtu_packet));
        assert_eq!(w.packet_count(), 2);
        // Two MTU packets + framing fit; the frame stays within one ESP-NOW frame.
        assert!(w.frame().len() <= ESP_NOW_MAX_FRAME_PAYLOAD);
    }

    #[test]
    fn try_push_refuses_a_packet_that_does_not_fit_and_leaves_a_valid_frame() {
        // A tiny frame buffer: header (1) + one record of 2 (one byte) fits, the
        // next does not.
        let mut buf = [0u8; ESP_NOW_FRAME_HEADER_LEN + ESP_NOW_LENGTH_PREFIX_LEN + 1];
        let mut w = EspNowFrameWriter::new(&mut buf);
        assert!(w.try_push(&[0x11]));
        assert!(!w.try_push(&[0x22])); // no room left
        assert_eq!(w.packet_count(), 1);
        // The frame written so far is still well-formed.
        let mut reader = decode_frame(w.frame()).unwrap();
        assert_eq!(reader.next(), Some(&[0x11][..]));
        assert_eq!(reader.next(), None);
    }

    #[test]
    fn decode_rejects_empty_and_unknown_version() {
        assert_eq!(decode_frame(&[]).err(), Some(FrameDecodeError::Empty));
        let bogus = [ESP_NOW_FRAME_VERSION.wrapping_add(7), 0x00, 0x01, 0xFF];
        assert_eq!(
            decode_frame(&bogus).err(),
            Some(FrameDecodeError::UnknownVersion(
                ESP_NOW_FRAME_VERSION.wrapping_add(7)
            )),
        );
    }

    #[test]
    fn a_truncated_trailing_record_ends_iteration_after_the_clean_packets() {
        // version | len=2, [0xAA,0xBB] | len=9 but only 1 byte follows.
        let frame = [
            ESP_NOW_FRAME_VERSION,
            0x00,
            0x02,
            0xAA,
            0xBB,
            0x00,
            0x09,
            0xCC,
        ];
        let mut reader = decode_frame(&frame).unwrap();
        assert_eq!(reader.next(), Some(&[0xAA, 0xBB][..]));
        assert_eq!(reader.next(), None); // truncated second record dropped
    }

    #[test]
    fn descriptor_is_a_repeating_shared_half_duplex_interface() {
        let d = descriptor(InterfaceId::new([0xE9; 16]));
        assert!(matches!(d.medium, MediumKind::SharedHalfDuplex));
        assert!(matches!(d.mode, InterfaceMode::Full));
        assert!(matches!(d.state, ConnectionState::Connected));
        assert!(d.capabilities.repeats);
        assert!(d.capabilities.receives && d.capabilities.transmits && d.capabilities.forwards);
    }
}
