use alloc::vec::Vec;
use chacha20poly1305::{aead::AeadInPlace, KeyInit, XChaCha20Poly1305, XNonce};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use super::{Catalog, Object, SecureLimits};
use crate::{CommitReceipt, OperationTicket, Revision, StoreIncarnation};

#[derive(Clone, Copy)]
pub(super) enum Error {
    Corrupt,
    Capacity,
    Allocation,
    Authentication,
}

pub(super) fn buffer(size: usize) -> Result<Zeroizing<Vec<u8>>, Error> {
    let mut out = Zeroizing::new(Vec::new());
    out.try_reserve_exact(size).map_err(|_| Error::Allocation)?;
    Ok(out)
}

pub(super) fn hash(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

pub(super) fn seal(
    key: &[u8],
    nonce: &[u8; 24],
    aad: &[u8],
    bytes: &[u8],
) -> Result<Zeroizing<Vec<u8>>, Error> {
    let mut out = buffer(bytes.len().checked_add(16).ok_or(Error::Capacity)?)?;
    out.extend_from_slice(bytes);
    let tag = XChaCha20Poly1305::new_from_slice(key)
        .map_err(|_| Error::Corrupt)?
        .encrypt_in_place_detached(XNonce::from_slice(nonce), aad, &mut out)
        .map_err(|_| Error::Authentication)?;
    out.extend_from_slice(&tag);
    Ok(out)
}

pub(super) fn open(
    key: &[u8],
    nonce: &[u8; 24],
    aad: &[u8],
    mut bytes: Zeroizing<Vec<u8>>,
) -> Result<Zeroizing<Vec<u8>>, Error> {
    let length = bytes.len().checked_sub(16).ok_or(Error::Corrupt)?;
    let mut tag = [0; 16];
    tag.copy_from_slice(&bytes[length..]);
    bytes.truncate(length);
    XChaCha20Poly1305::new_from_slice(key)
        .map_err(|_| Error::Corrupt)?
        .decrypt_in_place_detached(XNonce::from_slice(nonce), aad, &mut bytes, (&tag).into())
        .map_err(|_| Error::Authentication)?;
    Ok(bytes)
}

pub(super) fn catalog_aad(inc: StoreIncarnation, generation: u64, config: &[u8; 32]) -> [u8; 72] {
    let mut out = [0; 72];
    out[..8].copy_from_slice(b"PRNSCAT1");
    out[8..32].copy_from_slice(inc.as_bytes());
    out[32..40].copy_from_slice(&generation.to_le_bytes());
    out[40..].copy_from_slice(config);
    out
}

pub(super) fn object_aad(
    inc: StoreIncarnation,
    object: &Object,
) -> Result<Zeroizing<Vec<u8>>, Error> {
    let mut out = buffer(object.key.len().checked_add(56).ok_or(Error::Capacity)?)?;
    out.extend_from_slice(b"PRNSOBJ1");
    out.extend_from_slice(inc.as_bytes());
    out.extend_from_slice(&object.version);
    out.extend_from_slice(&(object.key.len() as u32).to_le_bytes());
    out.extend_from_slice(&(object.length as u32).to_le_bytes());
    out.extend_from_slice(&object.key);
    Ok(out)
}

pub(super) fn encoded_size(catalog: &Catalog) -> Result<usize, Error> {
    let mut size = 36usize
        .checked_add(
            catalog
                .receipts
                .len()
                .checked_mul(16)
                .ok_or(Error::Capacity)?,
        )
        .ok_or(Error::Capacity)?;
    for object in &catalog.objects {
        size = size
            .checked_add(object.key.len().checked_add(116).ok_or(Error::Capacity)?)
            .ok_or(Error::Capacity)?;
    }
    Ok(size)
}

pub(super) fn encode(catalog: &Catalog, limits: SecureLimits) -> Result<Zeroizing<Vec<u8>>, Error> {
    let size = encoded_size(catalog)?;
    if size > limits.max_catalog_bytes
        || catalog.objects.len() > limits.max_live_objects
        || catalog.receipts.len() > limits.max_receipts
    {
        return Err(Error::Capacity);
    }
    let mut out = buffer(size)?;
    out.extend_from_slice(b"PRNSCT01");
    out.extend_from_slice(&catalog.revision.0.to_le_bytes());
    out.extend_from_slice(&catalog.floor.to_le_bytes());
    out.extend_from_slice(&(catalog.arena as u32).to_le_bytes());
    out.extend_from_slice(&(catalog.objects.len() as u32).to_le_bytes());
    out.extend_from_slice(&(catalog.receipts.len() as u32).to_le_bytes());
    for object in &catalog.objects {
        out.extend_from_slice(&(object.key.len() as u32).to_le_bytes());
        out.extend_from_slice(&object.key);
        out.extend_from_slice(&object.version);
        out.extend_from_slice(object.dek.as_ref());
        out.extend_from_slice(&object.nonce);
        out.extend_from_slice(&object.location.to_le_bytes());
        out.extend_from_slice(&(object.length as u32).to_le_bytes());
        out.extend_from_slice(&object.digest);
    }
    for receipt in &catalog.receipts {
        let ticket =
            OperationTicket::from_operation_id(receipt.operation_id).map_err(|_| Error::Corrupt)?;
        out.extend_from_slice(&ticket.sequence().to_le_bytes());
        out.extend_from_slice(&receipt.revision.0.to_le_bytes());
    }
    Ok(out)
}

struct Reader<'a>(&'a [u8]);
impl<'a> Reader<'a> {
    fn take(&mut self, count: usize) -> Result<&'a [u8], Error> {
        if count > self.0.len() {
            return Err(Error::Corrupt);
        }
        let (out, tail) = self.0.split_at(count);
        self.0 = tail;
        Ok(out)
    }
    fn array<const N: usize>(&mut self) -> Result<[u8; N], Error> {
        self.take(N)?.try_into().map_err(|_| Error::Corrupt)
    }
    fn u32(&mut self) -> Result<u32, Error> {
        Ok(u32::from_le_bytes(self.array()?))
    }
    fn u64(&mut self) -> Result<u64, Error> {
        Ok(u64::from_le_bytes(self.array()?))
    }
}

pub(super) fn decode(
    bytes: &[u8],
    limits: SecureLimits,
    inc: StoreIncarnation,
) -> Result<Catalog, Error> {
    if bytes.len() > limits.max_catalog_bytes {
        return Err(Error::Capacity);
    }
    let mut input = Reader(bytes);
    if input.take(8)? != b"PRNSCT01" {
        return Err(Error::Corrupt);
    }
    let revision = Revision(input.u64()?);
    let floor = input.u64()?;
    let arena = input.u32()? as usize;
    let count = input.u32()? as usize;
    let receipts = input.u32()? as usize;
    if floor > revision.0
        || arena > 1
        || count > limits.max_live_objects
        || receipts > limits.max_receipts
        || revision.0 - floor != receipts as u64
    {
        return Err(Error::Corrupt);
    }
    let mut catalog = Catalog {
        revision,
        floor,
        arena,
        objects: Vec::new(),
        receipts: Vec::new(),
    };
    catalog
        .objects
        .try_reserve_exact(count)
        .map_err(|_| Error::Allocation)?;
    catalog
        .receipts
        .try_reserve_exact(receipts)
        .map_err(|_| Error::Allocation)?;
    for _ in 0..count {
        let length = input.u32()? as usize;
        if length > limits.max_key_bytes {
            return Err(Error::Corrupt);
        }
        let mut key = buffer(length)?;
        key.extend_from_slice(input.take(length)?);
        if catalog.objects.iter().any(|o| o.key == key) {
            return Err(Error::Corrupt);
        }
        let version = input.array()?;
        let mut dek = buffer(32)?;
        dek.extend_from_slice(input.take(32)?);
        let object = Object {
            key,
            version,
            dek,
            nonce: input.array()?,
            location: input.u32()?,
            length: input.u32()? as usize,
            digest: input.array()?,
        };
        if object.length > limits.max_value_bytes {
            return Err(Error::Corrupt);
        }
        catalog.objects.push(object);
    }
    for offset in 0..receipts {
        let sequence = input.u64()?;
        let rev = input.u64()?;
        if sequence != floor + offset as u64 + 1 || rev != sequence {
            return Err(Error::Corrupt);
        }
        let ticket = OperationTicket::new(inc, sequence).map_err(|_| Error::Corrupt)?;
        catalog.receipts.push(CommitReceipt {
            operation_id: ticket.operation_id(),
            revision: Revision(rev),
        });
    }
    if !input.0.is_empty() {
        return Err(Error::Corrupt);
    }
    Ok(catalog)
}
