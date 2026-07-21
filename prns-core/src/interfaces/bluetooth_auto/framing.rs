use heapless::Vec as HVec;

use crate::routing::links::MAX_LINK_MTU;

pub const FRAGMENT_HEADER_LEN: usize = 5;
pub const BLE_HW_MTU: usize = if 500 < MAX_LINK_MTU {
    500
} else {
    MAX_LINK_MTU
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FragmentKind {
    Start,
    Continue,
    End,
}

impl FragmentKind {
    const fn as_u8(self) -> u8 {
        match self {
            FragmentKind::Start => 0x01,
            FragmentKind::Continue => 0x02,
            FragmentKind::End => 0x03,
        }
    }

    const fn from_u8(byte: u8) -> Option<Self> {
        match byte {
            0x01 => Some(FragmentKind::Start),
            0x02 => Some(FragmentKind::Continue),
            0x03 => Some(FragmentKind::End),
            _ => None,
        }
    }
}

pub struct Fragment<'a> {
    pub kind: FragmentKind,
    pub seq: u16,
    pub total: u16,
    pub data: &'a [u8],
}

impl<'a> Fragment<'a> {
    pub fn encode(&self, out: &mut [u8]) -> Option<usize> {
        let need = FRAGMENT_HEADER_LEN + self.data.len();
        let slot = out.get_mut(..need)?;
        slot[0] = self.kind.as_u8();
        slot[1..3].copy_from_slice(&self.seq.to_be_bytes());
        slot[3..5].copy_from_slice(&self.total.to_be_bytes());
        slot[FRAGMENT_HEADER_LEN..].copy_from_slice(self.data);
        Some(need)
    }

    pub fn decode(bytes: &'a [u8]) -> Option<Fragment<'a>> {
        let kind = FragmentKind::from_u8(*bytes.first()?)?;
        let seq = u16::from_be_bytes(bytes.get(1..3)?.try_into().ok()?);
        let total = u16::from_be_bytes(bytes.get(3..5)?.try_into().ok()?);
        let data = bytes.get(FRAGMENT_HEADER_LEN..)?;
        Some(Fragment {
            kind,
            seq,
            total,
            data,
        })
    }
}

pub fn fragments_of(payload: &[u8], mtu: usize) -> impl Iterator<Item = Fragment<'_>> {
    let cap = mtu.saturating_sub(FRAGMENT_HEADER_LEN).max(1);
    let total = payload.len().div_ceil(cap).max(1);
    payload.chunks(cap).enumerate().map(move |(index, chunk)| {
        let kind = if index == 0 {
            FragmentKind::Start
        } else if index + 1 == total {
            FragmentKind::End
        } else {
            FragmentKind::Continue
        };
        Fragment {
            kind,
            seq: index as u16,
            total: total as u16,
            data: chunk,
        }
    })
}

pub struct Reassembler<const N: usize> {
    buf: HVec<u8, N>,
    next_seq: u16,
    total: u16,
    active: bool,
}

impl<const N: usize> Reassembler<N> {
    pub fn new() -> Self {
        Self {
            buf: HVec::new(),
            next_seq: 0,
            total: 0,
            active: false,
        }
    }

    pub fn absorb(&mut self, fragment: &Fragment<'_>) -> Option<&[u8]> {
        if fragment.seq == 0 {
            self.buf.clear();
            self.total = fragment.total;
            self.next_seq = 0;
            self.active = true;
        }
        if !self.active || fragment.seq != self.next_seq || fragment.total != self.total {
            self.active = false;
            return None;
        }
        if self.buf.extend_from_slice(fragment.data).is_err() {
            self.active = false;
            return None;
        }
        self.next_seq += 1;
        if self.next_seq == self.total {
            self.active = false;
            return Some(&self.buf[..]);
        }
        None
    }
}

impl<const N: usize> Default for Reassembler<N> {
    fn default() -> Self {
        Self::new()
    }
}

pub const STREAM_FRAME_PREFIX_LEN: usize = 2;

pub fn encode_stream_frame(frame: &[u8], out: &mut [u8]) -> Option<usize> {
    let len = u16::try_from(frame.len()).ok()?;
    let total = STREAM_FRAME_PREFIX_LEN + frame.len();
    let slot = out.get_mut(..total)?;
    slot[..STREAM_FRAME_PREFIX_LEN].copy_from_slice(&len.to_be_bytes());
    slot[STREAM_FRAME_PREFIX_LEN..].copy_from_slice(frame);
    Some(total)
}

pub struct StreamDeframer<const N: usize> {
    buf: HVec<u8, N>,
}

impl<const N: usize> StreamDeframer<N> {
    pub fn new() -> Self {
        Self { buf: HVec::new() }
    }

    pub fn absorb(&mut self, bytes: &[u8]) -> bool {
        self.buf.extend_from_slice(bytes).is_ok()
    }

    pub fn next_frame(&mut self, out: &mut [u8]) -> Option<usize> {
        let prefix: [u8; STREAM_FRAME_PREFIX_LEN] =
            self.buf.get(..STREAM_FRAME_PREFIX_LEN)?.try_into().ok()?;
        let len = u16::from_be_bytes(prefix) as usize;
        let total = STREAM_FRAME_PREFIX_LEN + len;
        if self.buf.len() < total {
            return None;
        }
        let dst = out.get_mut(..len)?;
        dst.copy_from_slice(&self.buf[STREAM_FRAME_PREFIX_LEN..total]);
        self.buf.copy_within(total.., 0);
        self.buf.truncate(self.buf.len() - total);
        Some(len)
    }
}

impl<const N: usize> Default for StreamDeframer<N> {
    fn default() -> Self {
        Self::new()
    }
}
