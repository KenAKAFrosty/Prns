//! Storage phases for native owners that must release their lane during network I/O.

use crate::{Error, SyncOptions};
use prns_groups_service::{
    Endpoint, EnvelopeId, EnvelopeInfo, GroupId, Incoming, InventoryCursor, InventoryKind,
    InventoryPage, InventorySnapshot, RecordId,
};
use prns_groups_store::StoreIncarnation;
use std::future::Future;

/// Each operation finishes its local transaction before returning. Implementors
/// may dispatch onto an existing bounded database lane. No network operation is
/// performed while that lane is borrowed.
pub trait SyncStorage: Send {
    type Error: Send;
    fn group(&self) -> GroupId;
    fn local_endpoint(&self) -> Endpoint;
    fn is_reader(&self, endpoint: Endpoint) -> bool;
    fn snapshot(
        &mut self,
    ) -> impl Future<Output = Result<Option<InventorySnapshot>, Error<Self::Error>>> + Send;
    fn inventory(
        &mut self,
        snapshot: InventorySnapshot,
        after: InventoryCursor,
        kind: InventoryKind,
        recipient: Endpoint,
        scan_limit: usize,
    ) -> impl Future<Output = Result<InventoryPage, Error<Self::Error>>> + Send;
    fn envelope(
        &mut self,
        id: EnvelopeId,
    ) -> impl Future<Output = Result<Option<Vec<u8>>, Error<Self::Error>>> + Send;
    /// Validate remote hints against durable local facts and repair a pending
    /// receipt for a known record before reporting it as already present.
    fn known(
        &mut self,
        info: EnvelopeInfo,
        incarnation: StoreIncarnation,
    ) -> impl Future<Output = Result<bool, Error<Self::Error>>> + Send;
    fn receive(
        &mut self,
        wire: &[u8],
        options: SyncOptions,
    ) -> impl Future<Output = Result<Incoming, Error<Self::Error>>> + Send;
    /// Exact retained local MLS receipt. Never re-encrypt a retry.
    fn receipt(
        &mut self,
        record: RecordId,
    ) -> impl Future<Output = Result<Option<Vec<u8>>, Error<Self::Error>>> + Send {
        let _ = record;
        async { Ok(None) }
    }
    fn envelope_id(&self, wire: &[u8]) -> EnvelopeId;
}

#[cfg(feature = "network")]
pub(crate) struct DirectStorage<'s, 'd, B>(pub &'s mut prns_groups_service::GroupService<'d, B>);
#[cfg(feature = "network")]
impl<B: prns_groups_store::AtomicBackend + Send> SyncStorage for DirectStorage<'_, '_, B>
where
    B::Error: Send,
{
    type Error = B::Error;
    fn group(&self) -> GroupId {
        self.0.group()
    }
    fn local_endpoint(&self) -> Endpoint {
        self.0.local_endpoint()
    }
    fn is_reader(&self, endpoint: Endpoint) -> bool {
        self.0.readers().contains(&endpoint)
    }
    async fn snapshot(&mut self) -> Result<Option<InventorySnapshot>, Error<Self::Error>> {
        Ok(self.0.snapshot()?)
    }
    async fn inventory(
        &mut self,
        snapshot: InventorySnapshot,
        after: InventoryCursor,
        kind: InventoryKind,
        recipient: Endpoint,
        scan_limit: usize,
    ) -> Result<InventoryPage, Error<Self::Error>> {
        Ok(self
            .0
            .inventory(snapshot, after, kind, recipient, scan_limit)?)
    }
    async fn envelope(&mut self, id: EnvelopeId) -> Result<Option<Vec<u8>>, Error<Self::Error>> {
        Ok(self.0.envelope(id)?.map(|bytes| bytes.to_vec()))
    }
    async fn known(
        &mut self,
        info: EnvelopeInfo,
        incarnation: StoreIncarnation,
    ) -> Result<bool, Error<Self::Error>> {
        let Some(local) = self.0.envelope_info(info.id)? else {
            return Ok(false);
        };
        if local.kind != info.kind || local.record != info.record || local.bytes != info.bytes {
            return Err(crate::ProtocolError::WrongEnvelope.into());
        }
        if local.kind == InventoryKind::Record {
            crate::sync::ensure_receipt(self.0, local.record, incarnation)?;
        }
        Ok(true)
    }
    async fn receive(
        &mut self,
        wire: &[u8],
        options: SyncOptions,
    ) -> Result<Incoming, Error<Self::Error>> {
        let operation = crate::sync::next_operation(self.0, options.incarnation)?;
        let incoming = self.0.receive(operation, wire, options.clock)?;
        if let Incoming::Record(record) = incoming {
            crate::sync::ensure_receipt(self.0, record, options.incarnation)?;
        }
        Ok(incoming)
    }
    async fn receipt(&mut self, record: RecordId) -> Result<Option<Vec<u8>>, Error<Self::Error>> {
        Ok(self.0.local_receipt(record)?.map(|bytes| bytes.to_vec()))
    }
    fn envelope_id(&self, wire: &[u8]) -> EnvelopeId {
        self.0.envelope_id(wire)
    }
}
