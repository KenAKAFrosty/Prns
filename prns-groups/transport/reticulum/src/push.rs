//! Exact-envelope prompt delivery. Polling remains the repair path.

use crate::storage::DirectStorage;
use crate::wire::{Request, Response, SUBMITTED_OVERHEAD, SUBMIT_OVERHEAD};
use crate::*;
use prns_groups_service::{EnvelopeInfo, GroupService, Incoming, InventoryKind};
use prns_groups_store::AtomicBackend;

/// Larger receipts remain fetchable through recovery inventory.
pub const MAX_INLINE_RECEIPT_BYTES: usize = 2048;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PushReport {
    pub path: prns_groups_core::sync::PathCapabilities,
    /// False means no Submit was attempted. It never establishes acceptance.
    pub supported: bool,
    /// Authenticated carrier acceptance, not device delivery evidence.
    pub submitted: usize,
    pub accepted_receipts: usize,
    pub rejected: usize,
    pub requests: usize,
    /// Submitted ciphertext plus optional returned MLS receipt ciphertext.
    pub bytes: usize,
}

pub async fn push<B: AtomicBackend + Send>(
    network: &Network,
    service: &mut GroupService<'_, B>,
    peer: Endpoint,
    group: GroupId,
    info: EnvelopeInfo,
    options: SyncOptions,
) -> Result<PushReport, Error<B::Error>>
where
    B::Error: Send,
{
    push_storage(
        network,
        &mut DirectStorage(service),
        peer,
        group,
        info,
        options,
    )
    .await
}

/// Submit one exact retained envelope; a lost response is safe to retry with the
/// same ID and bytes. Only a locally validated MLS receipt updates delivery.
pub async fn push_storage<S: SyncStorage>(
    network: &Network,
    service: &mut S,
    peer: Endpoint,
    group: GroupId,
    info: EnvelopeInfo,
    options: SyncOptions,
) -> Result<PushReport, Error<S::Error>> {
    if options.budget.max_requests < 2 || options.budget.max_envelopes == 0 {
        return Err(Error::InvalidBudget);
    }
    if group != service.group() || !service.is_reader(peer) || peer == service.local_endpoint() {
        return Err(Error::InvalidPeer);
    }
    let wire = service
        .envelope(info.id)
        .await?
        .ok_or(ProtocolError::WrongEnvelope)?;
    if wire.len() != info.bytes || service.envelope_id(&wire) != info.id {
        return Err(ProtocolError::WrongEnvelope.into());
    }
    if wire.len() > options.budget.max_bytes
        || wire.len()
            > network
                .limits()
                .max_frame_bytes
                .saturating_sub(SUBMIT_OVERHEAD)
    {
        return Err(ProtocolError::Oversized.into());
    }
    let mut report = PushReport {
        path: prns_groups_core::sync::PathCapabilities::default(),
        supported: false,
        submitted: 0,
        accepted_receipts: 0,
        rejected: 0,
        requests: 0,
        bytes: 0,
    };
    if !network.push_enabled() {
        return Ok(report);
    }
    let connection = network
        .connect(network.peer(peer).ok_or(Error::InvalidPeer)?)
        .await?;
    report.path = connection.capabilities();
    let result = async {
        report.requests += 1;
        let Some((_, maximum)) = capabilities(&connection, group, network.limits()).await? else {
            return Ok(report);
        };
        report.supported = true;
        if wire.len() > maximum {
            return Err(Error::Remote(RemoteError::OverBudget));
        }
        let allowance = if info.kind == InventoryKind::Record {
            options
                .budget
                .max_bytes
                .saturating_sub(wire.len())
                .min(
                    network
                        .limits()
                        .max_frame_bytes
                        .saturating_sub(SUBMITTED_OVERHEAD),
                )
                .min(MAX_INLINE_RECEIPT_BYTES)
        } else {
            0
        };
        report.requests += 1;
        let response = submit(
            &connection,
            network.limits(),
            group,
            info.id,
            &wire,
            allowance,
        )
        .await?;
        report.bytes += wire.len();
        match Response::decode(&response, network.limits())? {
            Response::Submitted {
                id,
                accepted,
                receipt,
            } if id == info.id => {
                report.submitted = usize::from(accepted);
                report.rejected = usize::from(!accepted);
                if !receipt.is_empty() {
                    if info.kind != InventoryKind::Record || receipt.len() > allowance {
                        return Err(ProtocolError::WrongEnvelope.into());
                    }
                    match service.receive(receipt, options).await? {
                        Incoming::Receipt(value)
                            if value.group == group
                                && value.record == info.record
                                && value.target == peer =>
                        {
                            report.accepted_receipts += 1;
                            report.bytes += receipt.len();
                        }
                        Incoming::Rejected(_) => report.rejected += 1,
                        _ => return Err(ProtocolError::WrongEnvelope.into()),
                    }
                }
            }
            Response::Error(error) => return Err(Error::Remote(error)),
            _ => return Err(ProtocolError::WrongEnvelope.into()),
        }
        report.path = connection.capabilities();
        Ok(report)
    }
    .await;
    let _closed = connection.close().await;
    result
}

/// Combined snapshot/capability exchange avoids adding a round trip to new
/// peers' initial pull. Only explicit old-format rejection or a read-only probe
/// timeout falls back; neither result authorizes Submit or confirms delivery.
pub(crate) async fn capabilities<E>(
    connection: &Connection,
    group: GroupId,
    limits: Limits,
) -> Result<Option<(Option<InventorySnapshot>, usize)>, Error<E>> {
    let frame = Request::Capabilities { group }.encode(limits)?;
    let bytes = match connection.request_bounded(&frame, 50).await {
        Ok(bytes) => bytes,
        Err(
            NetworkError::Timeout
            | NetworkError::Request(personal_rns::SendError::Failed(
                personal_rns::engine::SendRequestFailure::Timeout,
            )),
        ) => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    match Response::decode(&bytes, limits)? {
        Response::Capabilities {
            snapshot,
            max_submit_bytes,
        } if max_submit_bytes > 0 => Ok(Some((snapshot, max_submit_bytes))),
        Response::Error(RemoteError::InvalidRequest) => Ok(None),
        Response::Error(error) => Err(Error::Remote(error)),
        _ => Err(ProtocolError::InvalidEncoding.into()),
    }
}

pub(crate) async fn submit<E>(
    connection: &Connection,
    limits: Limits,
    group: GroupId,
    id: prns_groups_service::EnvelopeId,
    bytes: &[u8],
    receipt_bytes: usize,
) -> Result<Vec<u8>, Error<E>> {
    let frame = Request::Submit {
        group,
        id,
        bytes,
        receipt_bytes,
    }
    .encode(limits)?;
    Ok(connection
        .request_bounded(&frame, SUBMITTED_OVERHEAD.saturating_add(receipt_bytes))
        .await?)
}
