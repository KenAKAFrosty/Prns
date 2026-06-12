//! The two msgpack shapes in the resource family: the advertisement (context
//! 0x02) `umsgpack.packb({"t","d","n","h","r","o","i","l","q","f","m"})`
//! plus the hashmap update (context 0x04), and the resource hash followed by
//! `umsgpack.packb([segment, hashmap])`. Both seal under the link key like
//! any link traffic. The writers reproduce the reference's bytes exactly
//! (insertion-ordered keys, minimal-width integers); the advertisement parser
//! accepts its eleven keys in any order, the way the reference's dict unpack
//! does, but refuses extra keys the reference would incidentally tolerate.

use crate::routing::links::request::RequestId;
use crate::routing::links::resources::{
    ResourceHash, HASHMAP_MAX_LEN, MAP_HASH_LEN, RESOURCE_HASH_LEN, RESOURCE_NONCE_LEN,
};
use crate::wire::TRUNCATED_HASH_BYTE_LEN;

const FIXMAP_11: u8 = 0x8B;
const FIXARRAY_2: u8 = 0x92;
const FIXSTR_1: u8 = 0xA1;
const NIL: u8 = 0xC0;
const UINT_8: u8 = 0xCC;
const UINT_16: u8 = 0xCD;
const UINT_32: u8 = 0xCE;
const UINT_64: u8 = 0xCF;
const BIN_8: u8 = 0xC4;
const BIN_16: u8 = 0xC5;

/// The six advertisement flag bits (RNS 1.3.1 `f = x<<5 | p<<4 | u<<3 | s<<2 | c<<1 | e`).
/// The reference reads exactly these bits and ignores the rest; so does [`ResourceFlags::from_byte`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceFlags {
    pub encrypted: bool,
    pub compressed: bool,
    pub split: bool,
    pub is_request: bool,
    pub is_response: bool,
    pub has_metadata: bool,
}

impl ResourceFlags {
    #[must_use]
    pub const fn to_byte(self) -> u8 {
        (self.has_metadata as u8) << 5
            | (self.is_response as u8) << 4
            | (self.is_request as u8) << 3
            | (self.split as u8) << 2
            | (self.compressed as u8) << 1
            | self.encrypted as u8
    }

    #[must_use]
    pub const fn from_byte(byte: u8) -> Self {
        Self {
            encrypted: byte & 0x01 != 0,
            compressed: byte >> 1 & 0x01 != 0,
            split: byte >> 2 & 0x01 != 0,
            is_request: byte >> 3 & 0x01 != 0,
            is_response: byte >> 4 & 0x01 != 0,
            has_metadata: byte >> 5 & 0x01 != 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceAdvertisement<'a> {
    pub transfer_size: u64,
    pub data_size: u64,
    pub part_count: u64,
    pub hash: ResourceHash,
    pub nonce: [u8; RESOURCE_NONCE_LEN],
    pub original_hash: ResourceHash,
    pub segment_index: u64,
    pub total_segments: u64,
    pub request_id: Option<RequestId>,
    pub flags: ResourceFlags,
    pub hashmap: &'a [u8],
}

fn put(buf: &mut [u8], at: usize, bytes: &[u8]) -> Option<usize> {
    let end = at.checked_add(bytes.len())?;
    buf.get_mut(at..end)?.copy_from_slice(bytes);
    Some(end)
}

fn put_key(buf: &mut [u8], at: usize, key: u8) -> Option<usize> {
    put(buf, at, &[FIXSTR_1, key])
}

fn put_uint(buf: &mut [u8], at: usize, value: u64) -> Option<usize> {
    match value {
        0..=0x7F => put(buf, at, &[value as u8]),
        0x80..=0xFF => put(buf, at, &[UINT_8, value as u8]),
        0x100..=0xFFFF => {
            let at = put(buf, at, &[UINT_16])?;
            put(buf, at, &(value as u16).to_be_bytes())
        }
        0x1_0000..=0xFFFF_FFFF => {
            let at = put(buf, at, &[UINT_32])?;
            put(buf, at, &(value as u32).to_be_bytes())
        }
        _ => {
            let at = put(buf, at, &[UINT_64])?;
            put(buf, at, &value.to_be_bytes())
        }
    }
}

fn put_bin(buf: &mut [u8], at: usize, bytes: &[u8]) -> Option<usize> {
    let at = match bytes.len() {
        0..=0xFF => put(buf, at, &[BIN_8, bytes.len() as u8])?,
        0x100..=0xFFFF => {
            let at = put(buf, at, &[BIN_16])?;
            put(buf, at, &(bytes.len() as u16).to_be_bytes())?
        }
        _ => return None,
    };
    put(buf, at, bytes)
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.at.checked_add(n)?;
        let taken = self.bytes.get(self.at..end)?;
        self.at = end;
        Some(taken)
    }

    fn byte(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.at).copied()
    }

    fn uint(&mut self) -> Option<u64> {
        match self.byte()? {
            value @ 0x00..=0x7F => Some(value as u64),
            UINT_8 => Some(self.byte()? as u64),
            UINT_16 => Some(u16::from_be_bytes(self.take(2)?.try_into().ok()?) as u64),
            UINT_32 => Some(u32::from_be_bytes(self.take(4)?.try_into().ok()?) as u64),
            UINT_64 => Some(u64::from_be_bytes(self.take(8)?.try_into().ok()?)),
            _ => None,
        }
    }

    fn bin(&mut self) -> Option<&'a [u8]> {
        let len = match self.byte()? {
            BIN_8 => self.byte()? as usize,
            BIN_16 => u16::from_be_bytes(self.take(2)?.try_into().ok()?) as usize,
            _ => return None,
        };
        self.take(len)
    }
}

fn fixed<const N: usize>(bytes: &[u8]) -> Option<[u8; N]> {
    bytes.try_into().ok()
}

fn store<T>(slot: &mut Option<T>, value: T) -> Option<()> {
    if slot.is_some() {
        return None;
    }
    *slot = Some(value);
    Some(())
}

impl<'a> ResourceAdvertisement<'a> {
    /// `ResourceAdvertisement.pack()` byte for byte: a fixmap of eleven
    /// one-character keys in the reference's insertion order, integers at
    /// umsgpack's minimal widths, an absent request id as nil.
    pub fn write(&self, buf: &mut [u8]) -> Option<usize> {
        if self.hashmap.len() > HASHMAP_MAX_LEN * MAP_HASH_LEN
            || !self.hashmap.len().is_multiple_of(MAP_HASH_LEN)
        {
            return None;
        }
        let mut at = put(buf, 0, &[FIXMAP_11])?;
        at = put_key(buf, at, b't')?;
        at = put_uint(buf, at, self.transfer_size)?;
        at = put_key(buf, at, b'd')?;
        at = put_uint(buf, at, self.data_size)?;
        at = put_key(buf, at, b'n')?;
        at = put_uint(buf, at, self.part_count)?;
        at = put_key(buf, at, b'h')?;
        at = put_bin(buf, at, self.hash.as_bytes())?;
        at = put_key(buf, at, b'r')?;
        at = put_bin(buf, at, &self.nonce)?;
        at = put_key(buf, at, b'o')?;
        at = put_bin(buf, at, self.original_hash.as_bytes())?;
        at = put_key(buf, at, b'i')?;
        at = put_uint(buf, at, self.segment_index)?;
        at = put_key(buf, at, b'l')?;
        at = put_uint(buf, at, self.total_segments)?;
        at = put_key(buf, at, b'q')?;
        at = match &self.request_id {
            None => put(buf, at, &[NIL])?,
            Some(id) => put_bin(buf, at, id.as_bytes())?,
        };
        at = put_key(buf, at, b'f')?;
        at = put_uint(buf, at, self.flags.to_byte() as u64)?;
        at = put_key(buf, at, b'm')?;
        put_bin(buf, at, self.hashmap)
    }

    /// `ResourceAdvertisement.unpack`'s read: exactly eleven entries, each of
    /// the known keys once, in any order; trailing bytes after the map are
    /// ignored the way umsgpack ignores them. A request id must be nil or
    /// sixteen bytes; flag bits past the known six fall away.
    pub fn parse(plaintext: &'a [u8]) -> Option<Self> {
        let mut reader = Reader {
            bytes: plaintext,
            at: 0,
        };
        if reader.byte()? != FIXMAP_11 {
            return None;
        }
        let mut transfer_size = None;
        let mut data_size = None;
        let mut part_count = None;
        let mut hash = None;
        let mut nonce = None;
        let mut original_hash = None;
        let mut segment_index = None;
        let mut total_segments = None;
        let mut request_id = None;
        let mut flags = None;
        let mut hashmap = None;
        for _ in 0..11 {
            if reader.byte()? != FIXSTR_1 {
                return None;
            }
            match reader.byte()? {
                b't' => store(&mut transfer_size, reader.uint()?)?,
                b'd' => store(&mut data_size, reader.uint()?)?,
                b'n' => store(&mut part_count, reader.uint()?)?,
                b'h' => store(&mut hash, ResourceHash::new(fixed(reader.bin()?)?))?,
                b'r' => store(&mut nonce, fixed::<RESOURCE_NONCE_LEN>(reader.bin()?)?)?,
                b'o' => store(&mut original_hash, ResourceHash::new(fixed(reader.bin()?)?))?,
                b'i' => store(&mut segment_index, reader.uint()?)?,
                b'l' => store(&mut total_segments, reader.uint()?)?,
                b'q' => {
                    let id = if reader.peek()? == NIL {
                        reader.byte()?;
                        None
                    } else {
                        Some(RequestId(fixed::<TRUNCATED_HASH_BYTE_LEN>(reader.bin()?)?))
                    };
                    store(&mut request_id, id)?
                }
                b'f' => store(&mut flags, ResourceFlags::from_byte(reader.uint()? as u8))?,
                b'm' => store(&mut hashmap, reader.bin()?)?,
                _ => return None,
            }
        }
        Some(Self {
            transfer_size: transfer_size?,
            data_size: data_size?,
            part_count: part_count?,
            hash: hash?,
            nonce: nonce?,
            original_hash: original_hash?,
            segment_index: segment_index?,
            total_segments: total_segments?,
            request_id: request_id?,
            flags: flags?,
            hashmap: hashmap?,
        })
    }
}

/// The sender's reply when the receiver's hashmap runs dry. RNS 1.3.1
/// `Resource.request`'s HMU branch: the resource hash, then
/// `umsgpack.packb([segment, hashmap])` carrying the next run of map hashes.
pub fn write_hashmap_update_plaintext(
    hash: &ResourceHash,
    segment: u64,
    hashmap: &[u8],
    buf: &mut [u8],
) -> Option<usize> {
    if hashmap.len() > HASHMAP_MAX_LEN * MAP_HASH_LEN || !hashmap.len().is_multiple_of(MAP_HASH_LEN)
    {
        return None;
    }
    let mut at = put(buf, 0, hash.as_bytes())?;
    at = put(buf, at, &[FIXARRAY_2])?;
    at = put_uint(buf, at, segment)?;
    put_bin(buf, at, hashmap)
}

pub struct ParsedHashmapUpdate<'a> {
    pub hash: ResourceHash,
    pub segment: u64,
    pub hashmap: &'a [u8],
}

pub fn parse_hashmap_update_plaintext(plaintext: &[u8]) -> Option<ParsedHashmapUpdate<'_>> {
    let mut reader = Reader {
        bytes: plaintext,
        at: 0,
    };
    let hash = ResourceHash::new(fixed::<RESOURCE_HASH_LEN>(reader.take(RESOURCE_HASH_LEN)?)?);
    if reader.byte()? != FIXARRAY_2 {
        return None;
    }
    let segment = reader.uint()?;
    let hashmap = reader.bin()?;
    Some(ParsedHashmapUpdate {
        hash,
        segment,
        hashmap,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routing::links::data::LINK_MDU;
    use crate::routing::links::resources::{ADVERTISEMENT_OVERHEAD, COLLISION_GUARD_SIZE};

    fn hx(s: &str) -> std::vec::Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("valid hex"))
            .collect()
    }

    fn full_hashmap() -> std::vec::Vec<u8> {
        (0..HASHMAP_MAX_LEN * MAP_HASH_LEN)
            .map(|i| (i * 7) as u8)
            .collect()
    }

    fn h() -> ResourceHash {
        let mut bytes = [0u8; RESOURCE_HASH_LEN];
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = i as u8;
        }
        ResourceHash::new(bytes)
    }

    fn o() -> ResourceHash {
        let mut bytes = [0u8; RESOURCE_HASH_LEN];
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = 32 + i as u8;
        }
        ResourceHash::new(bytes)
    }

    const NONCE: [u8; RESOURCE_NONCE_LEN] = [0xAA, 0xBB, 0xCC, 0xDD];

    // ResourceAdvertisement.pack() of {t: 345678, d: 1048575, n: 803, h, r, o,
    // i: 1, l: 1, q: None, f: 0x03, m: 296 bytes} under the reference's
    // vendored umsgpack — the leading 0x8b fixmap, keys in insertion order.
    const V1: &str = "8ba174ce0005464ea164ce000fffffa16ecd0323a168c420000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1fa172c404aabbccdda16fc420202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3fa16901a16c01a171c0a16603a16dc5012800070e151c232a31383f464d545b626970777e858c939aa1a8afb6bdc4cbd2d9e0e7eef5fc030a11181f262d343b424950575e656c737a81888f969da4abb2b9c0c7ced5dce3eaf1f8ff060d141b222930373e454c535a61686f767d848b9299a0a7aeb5bcc3cad1d8dfe6edf4fb020910171e252c333a41484f565d646b727980878e959ca3aab1b8bfc6cdd4dbe2e9f0f7fe050c131a21282f363d444b525960676e757c838a91989fa6adb4bbc2c9d0d7dee5ecf3fa01080f161d242b323940474e555c636a71787f868d949ba2a9b0b7bec5ccd3dae1e8eff6fd040b121920272e353c434a51585f666d747b828990979ea5acb3bac1c8cfd6dde4ebf2f900070e151c232a31383f464d545b626970777e858c939aa1a8afb6bdc4cbd2d9e0e7eef5fc030a11";

    // The response shape: q = bytes([0x7E]*16), f = metadata|response|encrypted
    // (0x31), an 8-byte hashmap riding bin8.
    const V2: &str = "8ba174cd01afa164cd0384a16e02a168c420000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1fa172c404aabbccdda16fc420000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1fa16901a16c01a171c4107e7e7e7e7e7e7e7e7e7e7e7e7e7e7e7ea16631a16dc4080102030405060708";

    // A giant split resource: d = 5_000_000_000 rides uint64, l = 5000 rides
    // uint16, segment 7, f = 0x07 (split|compressed|encrypted).
    const V3: &str = "8ba174ce001000aea164cf000000012a05f200a16ecd0982a168c420000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1fa172c404aabbccdda16fc420202122232425262728292a2b2c2d2e2f303132333435363738393a3b3c3d3e3fa16907a16ccd1388a171c0a16607a16dc5012800070e151c232a31383f464d545b626970777e858c939aa1a8afb6bdc4cbd2d9e0e7eef5fc030a11181f262d343b424950575e656c737a81888f969da4abb2b9c0c7ced5dce3eaf1f8ff060d141b222930373e454c535a61686f767d848b9299a0a7aeb5bcc3cad1d8dfe6edf4fb020910171e252c333a41484f565d646b727980878e959ca3aab1b8bfc6cdd4dbe2e9f0f7fe050c131a21282f363d444b525960676e757c838a91989fa6adb4bbc2c9d0d7dee5ecf3fa01080f161d242b323940474e555c636a71787f868d949ba2a9b0b7bec5ccd3dae1e8eff6fd040b121920272e353c434a51585f666d747b828990979ea5acb3bac1c8cfd6dde4ebf2f900070e151c232a31383f464d545b626970777e858c939aa1a8afb6bdc4cbd2d9e0e7eef5fc030a11";

    // umsgpack.packb([1, <296-byte hashmap>]) — fixarray(2), fixint segment,
    // bin16 hashes.
    const HMU1_BODY: &str = "9201c5012800070e151c232a31383f464d545b626970777e858c939aa1a8afb6bdc4cbd2d9e0e7eef5fc030a11181f262d343b424950575e656c737a81888f969da4abb2b9c0c7ced5dce3eaf1f8ff060d141b222930373e454c535a61686f767d848b9299a0a7aeb5bcc3cad1d8dfe6edf4fb020910171e252c333a41484f565d646b727980878e959ca3aab1b8bfc6cdd4dbe2e9f0f7fe050c131a21282f363d444b525960676e757c838a91989fa6adb4bbc2c9d0d7dee5ecf3fa01080f161d242b323940474e555c636a71787f868d949ba2a9b0b7bec5ccd3dae1e8eff6fd040b121920272e353c434a51585f666d747b828990979ea5acb3bac1c8cfd6dde4ebf2f900070e151c232a31383f464d545b626970777e858c939aa1a8afb6bdc4cbd2d9e0e7eef5fc030a11";

    fn v1_adv(hashmap: &[u8]) -> ResourceAdvertisement<'_> {
        ResourceAdvertisement {
            transfer_size: 345_678,
            data_size: 1_048_575,
            part_count: 803,
            hash: h(),
            nonce: NONCE,
            original_hash: o(),
            segment_index: 1,
            total_segments: 1,
            request_id: None,
            flags: ResourceFlags::from_byte(0x03),
            hashmap,
        }
    }

    #[test]
    fn the_advertisement_pack_is_byte_identical_to_the_reference() {
        let hashmap = full_hashmap();
        let adv = v1_adv(&hashmap);
        let mut buf = [0u8; LINK_MDU];
        let n = adv.write(&mut buf).unwrap();
        assert_eq!(&buf[..n], &hx(V1)[..]);
        assert!(n <= LINK_MDU);
        assert_eq!(ResourceAdvertisement::parse(&buf[..n]).unwrap(), adv);
    }

    #[test]
    fn a_response_advertisement_names_its_request_back() {
        let adv = ResourceAdvertisement {
            transfer_size: 431,
            data_size: 900,
            part_count: 2,
            hash: h(),
            nonce: NONCE,
            original_hash: h(),
            segment_index: 1,
            total_segments: 1,
            request_id: Some(RequestId([0x7E; 16])),
            flags: ResourceFlags::from_byte(0x31),
            hashmap: &[1, 2, 3, 4, 5, 6, 7, 8],
        };
        let mut buf = [0u8; LINK_MDU];
        let n = adv.write(&mut buf).unwrap();
        assert_eq!(&buf[..n], &hx(V2)[..]);
        let parsed = ResourceAdvertisement::parse(&buf[..n]).unwrap();
        assert_eq!(parsed, adv);
        assert!(parsed.flags.encrypted);
        assert!(parsed.flags.is_response);
        assert!(parsed.flags.has_metadata);
        assert!(!parsed.flags.compressed);
    }

    #[test]
    fn giant_split_resources_cross_the_u32_size_boundary() {
        let hashmap = full_hashmap();
        let adv = ResourceAdvertisement {
            transfer_size: 1_048_750,
            data_size: 5_000_000_000,
            part_count: 2_434,
            hash: h(),
            nonce: NONCE,
            original_hash: o(),
            segment_index: 7,
            total_segments: 5_000,
            request_id: None,
            flags: ResourceFlags::from_byte(0x07),
            hashmap: &hashmap,
        };
        let mut buf = [0u8; LINK_MDU];
        let n = adv.write(&mut buf).unwrap();
        assert_eq!(&buf[..n], &hx(V3)[..]);
        let parsed = ResourceAdvertisement::parse(&buf[..n]).unwrap();
        assert_eq!(parsed.data_size, 5_000_000_000);
        assert_eq!(parsed.total_segments, 5_000);
        assert!(parsed.flags.split);
    }

    #[test]
    fn the_parser_accepts_the_eleven_keys_in_any_order() {
        let wire = hx(V2);
        let expected = ResourceAdvertisement::parse(&wire).unwrap();
        let mut reversed = [0u8; LINK_MDU];
        let mut at = put(&mut reversed, 0, &[FIXMAP_11]).unwrap();
        at = put_key(&mut reversed, at, b'm').unwrap();
        at = put_bin(&mut reversed, at, expected.hashmap).unwrap();
        at = put_key(&mut reversed, at, b'f').unwrap();
        at = put_uint(&mut reversed, at, expected.flags.to_byte() as u64).unwrap();
        at = put_key(&mut reversed, at, b'q').unwrap();
        at = put_bin(&mut reversed, at, expected.request_id.unwrap().as_bytes()).unwrap();
        at = put_key(&mut reversed, at, b'l').unwrap();
        at = put_uint(&mut reversed, at, expected.total_segments).unwrap();
        at = put_key(&mut reversed, at, b'i').unwrap();
        at = put_uint(&mut reversed, at, expected.segment_index).unwrap();
        at = put_key(&mut reversed, at, b'o').unwrap();
        at = put_bin(&mut reversed, at, expected.original_hash.as_bytes()).unwrap();
        at = put_key(&mut reversed, at, b'r').unwrap();
        at = put_bin(&mut reversed, at, &expected.nonce).unwrap();
        at = put_key(&mut reversed, at, b'h').unwrap();
        at = put_bin(&mut reversed, at, expected.hash.as_bytes()).unwrap();
        at = put_key(&mut reversed, at, b'n').unwrap();
        at = put_uint(&mut reversed, at, expected.part_count).unwrap();
        at = put_key(&mut reversed, at, b'd').unwrap();
        at = put_uint(&mut reversed, at, expected.data_size).unwrap();
        at = put_key(&mut reversed, at, b't').unwrap();
        at = put_uint(&mut reversed, at, expected.transfer_size).unwrap();
        assert_eq!(
            ResourceAdvertisement::parse(&reversed[..at]).unwrap(),
            expected,
        );
    }

    #[test]
    fn malformed_advertisements_refuse() {
        let wire = hx(V1);
        for cut in [0, 1, 5, 100, wire.len() - 1] {
            assert!(ResourceAdvertisement::parse(&wire[..cut]).is_none());
        }
        let mut wrong_count = wire.clone();
        wrong_count[0] = 0x8A;
        assert!(ResourceAdvertisement::parse(&wrong_count).is_none());
        let d_key_at = wire
            .windows(2)
            .position(|pair| pair == [FIXSTR_1, b'd'])
            .unwrap()
            + 1;
        let mut duplicate_key = wire.clone();
        duplicate_key[d_key_at] = b't';
        assert!(ResourceAdvertisement::parse(&duplicate_key).is_none());
        let mut unknown_key = wire.clone();
        unknown_key[2] = b'z';
        assert!(ResourceAdvertisement::parse(&unknown_key).is_none());

        let hashmap = full_hashmap();
        let mut buf = [0u8; LINK_MDU];
        let mut short_request_id = v1_adv(&hashmap);
        short_request_id.request_id = Some(RequestId([0x7E; 16]));
        let n = short_request_id.write(&mut buf).unwrap();
        let mut wire = buf[..n].to_vec();
        let q_len_at = wire
            .windows(4)
            .position(|run| run == [FIXSTR_1, b'q', BIN_8, TRUNCATED_HASH_BYTE_LEN as u8])
            .unwrap()
            + 3;
        wire[q_len_at] = 15;
        assert!(ResourceAdvertisement::parse(&wire).is_none());
    }

    #[test]
    fn oversize_or_ragged_hashmaps_refuse_to_write() {
        let mut buf = [0u8; 2 * LINK_MDU];
        let too_long = std::vec![0u8; (HASHMAP_MAX_LEN + 1) * MAP_HASH_LEN];
        assert!(v1_adv(&too_long).write(&mut buf).is_none());
        let ragged = std::vec![0u8; MAP_HASH_LEN + 1];
        assert!(v1_adv(&ragged).write(&mut buf).is_none());
        assert!(write_hashmap_update_plaintext(&h(), 1, &too_long, &mut buf).is_none());
    }

    #[test]
    fn flag_bits_past_the_known_six_fall_away() {
        let all = ResourceFlags::from_byte(0xFF);
        assert!(
            all.encrypted
                && all.compressed
                && all.split
                && all.is_request
                && all.is_response
                && all.has_metadata
        );
        assert_eq!(all.to_byte(), 0x3F);
        let none = ResourceFlags::from_byte(0xC0);
        assert_eq!(none.to_byte(), 0x00);
    }

    #[test]
    fn the_hashmap_update_round_trips_the_reference_bytes() {
        let hashmap = full_hashmap();
        let mut buf = [0u8; 512];
        let n = write_hashmap_update_plaintext(&h(), 1, &hashmap, &mut buf).unwrap();
        let mut expected = std::vec::Vec::new();
        expected.extend_from_slice(h().as_bytes());
        expected.extend_from_slice(&hx(HMU1_BODY));
        assert_eq!(&buf[..n], expected.as_slice());

        let parsed = parse_hashmap_update_plaintext(&buf[..n]).unwrap();
        assert_eq!(parsed.hash, h());
        assert_eq!(parsed.segment, 1);
        assert_eq!(parsed.hashmap, hashmap.as_slice());

        let n =
            write_hashmap_update_plaintext(&h(), 0, &[1, 2, 3, 4, 5, 6, 7, 8], &mut buf).unwrap();
        assert_eq!(
            &buf[RESOURCE_HASH_LEN..n],
            &hx("9200c4080102030405060708")[..]
        );
        assert!(parse_hashmap_update_plaintext(&buf[..RESOURCE_HASH_LEN + 1]).is_none());
    }

    #[test]
    fn the_protocol_arithmetic_matches_the_reference() {
        assert_eq!(LINK_MDU, 431);
        assert_eq!(ADVERTISEMENT_OVERHEAD, 134);
        assert_eq!(HASHMAP_MAX_LEN, 74);
        assert_eq!(COLLISION_GUARD_SIZE, 224);
    }
}
