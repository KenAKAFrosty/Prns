use core::cell::RefCell;

use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_sync::blocking_mutex::Mutex as BlockingMutex;
use embassy_sync::channel::Sender;
use embassy_sync::signal::Signal;
use portable_atomic::{AtomicU64, Ordering};

use crate::engine::{
    CloseLink, CommandId, IssuedCommand, Journaled, PacketReceiptDelivered, PrnsCommand,
    RequestResponseTimeout, Respond, RespondData, RespondPayload, SendGroup, SendGroupFailure,
    SendGroupPayload, SendPlainPacket, SendPlainPacketFailure, SendPlainPacketPayload, SendRequest,
    SendRequestData, SendRequestFailure, SendSinglePacket, SendSinglePacketFailure,
    SendSinglePacketPayload, Settlement,
};
use crate::routing::links::LinkId;
use crate::routing::request_handlers::RequestPathHash;
use crate::units::{ByteLimit, RttMillis};
use crate::wire::DestinationHash;

use super::super::request_endpoints::RespondToken;
use super::super::{AnnounceNowError, PrnsNodeApi, SendError};

const NO_AWAITER: u64 = u64::MAX;

pub struct CompletionPool<
    M: RawMutex,
    const COMPLETIONS: usize,
    const REQUEST_COMPLETIONS: usize = 0,
    const RESPONSE_BYTES: usize = 0,
> {
    next_id: AtomicU64,
    awaited: BlockingMutex<M, RefCell<[u64; COMPLETIONS]>>,
    slots: [Signal<M, Settlement>; COMPLETIONS],
    requests: BlockingMutex<M, RefCell<[RequestAwaited<RESPONSE_BYTES>; REQUEST_COMPLETIONS]>>,
    request_slots: [Signal<M, Settlement>; REQUEST_COMPLETIONS],
}

enum RequestAwaited<const RESPONSE_BYTES: usize> {
    Available,
    Awaiting {
        id: CommandId,
        response: RequestResponse<RESPONSE_BYTES>,
    },
}

enum RequestResponse<const RESPONSE_BYTES: usize> {
    Awaiting,
    Received(RequestResponseData<RESPONSE_BYTES>),
    TooLarge,
}

pub type RequestResponseData<const RESPONSE_BYTES: usize> = heapless::Vec<u8, RESPONSE_BYTES>;

pub(super) enum JournalRoute {
    Application,
    Awaiter,
}

impl<const RESPONSE_BYTES: usize> RequestAwaited<RESPONSE_BYTES> {
    fn awaits(&self, id: CommandId) -> bool {
        match self {
            Self::Available => false,
            Self::Awaiting { id: awaited, .. } => *awaited == id,
        }
    }
}

impl<
        M: RawMutex,
        const COMPLETIONS: usize,
        const REQUEST_COMPLETIONS: usize,
        const RESPONSE_BYTES: usize,
    > Default for CompletionPool<M, COMPLETIONS, REQUEST_COMPLETIONS, RESPONSE_BYTES>
{
    fn default() -> Self {
        Self::new()
    }
}

impl<
        M: RawMutex,
        const COMPLETIONS: usize,
        const REQUEST_COMPLETIONS: usize,
        const RESPONSE_BYTES: usize,
    > CompletionPool<M, COMPLETIONS, REQUEST_COMPLETIONS, RESPONSE_BYTES>
{
    #[must_use]
    pub const fn new() -> Self {
        Self {
            next_id: AtomicU64::new(0),
            awaited: BlockingMutex::new(RefCell::new([NO_AWAITER; COMPLETIONS])),
            slots: [const { Signal::new() }; COMPLETIONS],
            requests: BlockingMutex::new(RefCell::new(
                [const { RequestAwaited::Available }; REQUEST_COMPLETIONS],
            )),
            request_slots: [const { Signal::new() }; REQUEST_COMPLETIONS],
        }
    }

    fn mint(&self) -> CommandId {
        loop {
            let id = self.next_id.fetch_add(1, Ordering::Relaxed);
            if id != NO_AWAITER {
                return CommandId(id);
            }
        }
    }

    fn claim_settlement(&self, id: CommandId) -> Option<usize> {
        self.awaited.lock(|cell| {
            let mut awaited = cell.borrow_mut();
            let slot = awaited.iter().position(|entry| *entry == NO_AWAITER)?;
            self.slots[slot].reset();
            awaited[slot] = id.0;
            Some(slot)
        })
    }

    fn claim_request(&self, id: CommandId) -> Option<usize> {
        self.requests.lock(|cell| {
            let mut requests = cell.borrow_mut();
            let slot = requests
                .iter()
                .position(|entry| matches!(entry, RequestAwaited::Available))?;
            self.request_slots[slot].reset();
            requests[slot] = RequestAwaited::Awaiting {
                id,
                response: RequestResponse::Awaiting,
            };
            Some(slot)
        })
    }

    fn release(&self, slot: usize, id: CommandId) {
        self.awaited.lock(|cell| {
            let mut awaited = cell.borrow_mut();
            if awaited.get(slot).is_some_and(|awaited| *awaited == id.0) {
                awaited[slot] = NO_AWAITER;
                self.slots[slot].reset();
            }
        });
    }

    fn settle(&self, id: CommandId, settlement: Settlement) -> bool {
        self.awaited.lock(|cell| {
            let awaited = cell.borrow();
            match awaited.iter().position(|awaited| *awaited == id.0) {
                Some(slot) => {
                    self.slots[slot].signal(settlement);
                    true
                }
                None => false,
            }
        })
    }

    async fn parked(&self, slot: usize) -> Settlement {
        self.slots[slot].wait().await
    }

    fn release_request(&self, slot: usize, id: CommandId) {
        self.requests.lock(|cell| {
            let mut requests = cell.borrow_mut();
            if requests.get(slot).is_some_and(|entry| entry.awaits(id)) {
                requests[slot] = RequestAwaited::Available;
                self.request_slots[slot].reset();
            }
        });
    }

    fn settle_request(&self, id: CommandId, settlement: Settlement) -> bool {
        self.requests.lock(|cell| {
            let requests = cell.borrow();
            match requests.iter().position(|entry| entry.awaits(id)) {
                Some(slot) => {
                    self.request_slots[slot].signal(settlement);
                    true
                }
                None => false,
            }
        })
    }

    async fn parked_request(&self, slot: usize) -> Settlement {
        self.request_slots[slot].wait().await
    }

    fn capture_response(&self, id: CommandId, data: &[u8]) -> JournalRoute {
        self.requests.lock(|cell| {
            let mut requests = cell.borrow_mut();
            let Some(RequestAwaited::Awaiting { response, .. }) =
                requests.iter_mut().find(|entry| entry.awaits(id))
            else {
                return JournalRoute::Application;
            };
            match response {
                RequestResponse::Awaiting => {
                    let mut received = RequestResponseData::new();
                    if received.extend_from_slice(data).is_err() {
                        *response = RequestResponse::TooLarge;
                    } else {
                        *response = RequestResponse::Received(received);
                    }
                }
                RequestResponse::Received(received) => {
                    if received.extend_from_slice(data).is_err() {
                        *response = RequestResponse::TooLarge;
                    }
                }
                RequestResponse::TooLarge => {}
            }
            JournalRoute::Awaiter
        })
    }

    fn take_request_response(
        &self,
        slot: usize,
        id: CommandId,
    ) -> Result<RequestResponseData<RESPONSE_BYTES>, SendRequestFailure> {
        self.requests.lock(|cell| {
            let mut requests = cell.borrow_mut();
            let Some(RequestAwaited::Awaiting {
                id: awaited_id,
                response,
            }) = requests.get_mut(slot)
            else {
                return Err(SendRequestFailure::WriteFailed);
            };
            if *awaited_id != id {
                return Err(SendRequestFailure::WriteFailed);
            }
            match core::mem::replace(response, RequestResponse::Awaiting) {
                RequestResponse::Awaiting => Err(SendRequestFailure::WriteFailed),
                RequestResponse::Received(response) => Ok(response),
                RequestResponse::TooLarge => Err(SendRequestFailure::ResponseTooLarge),
            }
        })
    }
}

pub struct PrnsNodeHandle<
    'a,
    M: RawMutex,
    const COMMANDS: usize,
    const COMPLETIONS: usize,
    const REQUEST_COMPLETIONS: usize = 0,
    const RESPONSE_BYTES: usize = 0,
> {
    commands: Sender<'a, M, IssuedCommand, COMMANDS>,
    pool: &'a CompletionPool<M, COMPLETIONS, REQUEST_COMPLETIONS, RESPONSE_BYTES>,
}

impl<
        M: RawMutex,
        const COMMANDS: usize,
        const COMPLETIONS: usize,
        const REQUEST_COMPLETIONS: usize,
        const RESPONSE_BYTES: usize,
    > Clone for PrnsNodeHandle<'_, M, COMMANDS, COMPLETIONS, REQUEST_COMPLETIONS, RESPONSE_BYTES>
{
    fn clone(&self) -> Self {
        *self
    }
}

impl<
        M: RawMutex,
        const COMMANDS: usize,
        const COMPLETIONS: usize,
        const REQUEST_COMPLETIONS: usize,
        const RESPONSE_BYTES: usize,
    > Copy for PrnsNodeHandle<'_, M, COMMANDS, COMPLETIONS, REQUEST_COMPLETIONS, RESPONSE_BYTES>
{
}

impl<
        'a,
        M: RawMutex,
        const COMMANDS: usize,
        const COMPLETIONS: usize,
        const REQUEST_COMPLETIONS: usize,
        const RESPONSE_BYTES: usize,
    > PrnsNodeHandle<'a, M, COMMANDS, COMPLETIONS, REQUEST_COMPLETIONS, RESPONSE_BYTES>
{
    #[must_use]
    pub fn new(
        commands: Sender<'a, M, IssuedCommand, COMMANDS>,
        pool: &'a CompletionPool<M, COMPLETIONS, REQUEST_COMPLETIONS, RESPONSE_BYTES>,
    ) -> Self {
        Self { commands, pool }
    }

    /// Queues a command without awaiting settlement and returns its ID, or `None` when the command lane is full.
    pub fn issue(&self, command: PrnsCommand) -> Option<CommandId> {
        let id = self.pool.mint();
        self.commands.try_send(IssuedCommand { id, command }).ok()?;
        Some(id)
    }

    pub async fn send_single_packet(
        &self,
        destination: DestinationHash,
        data: &[u8],
    ) -> Result<PacketReceiptDelivered, SendError<SendSinglePacketFailure>> {
        let payload =
            SendSinglePacketPayload::from_slice(data).map_err(|()| SendError::PayloadTooLarge)?;
        let id = self.pool.mint();
        let slot = self.pool.claim_settlement(id).ok_or(SendError::Busy)?;
        let _guard = SlotGuard {
            pool: self.pool,
            slot,
            id,
        };
        self.commands
            .try_send(IssuedCommand {
                id,
                command: PrnsCommand::SendSinglePacket(SendSinglePacket {
                    destination,
                    payload,
                }),
            })
            .map_err(|_| SendError::NodeStopped)?;
        match self.pool.parked(slot).await {
            Settlement::SendSinglePacket(result) => result.map_err(SendError::Failed),
            _ => Err(SendError::NodeStopped),
        }
    }

    pub async fn send_plain_packet(
        &self,
        destination: DestinationHash,
        data: &[u8],
    ) -> Result<(), SendError<SendPlainPacketFailure>> {
        let payload =
            SendPlainPacketPayload::from_slice(data).map_err(|()| SendError::PayloadTooLarge)?;
        let id = self.pool.mint();
        let slot = self.pool.claim_settlement(id).ok_or(SendError::Busy)?;
        let _guard = SlotGuard {
            pool: self.pool,
            slot,
            id,
        };
        self.commands
            .try_send(IssuedCommand {
                id,
                command: PrnsCommand::SendPlainPacket(SendPlainPacket {
                    destination,
                    payload,
                }),
            })
            .map_err(|_| SendError::NodeStopped)?;
        match self.pool.parked(slot).await {
            Settlement::SendPlainPacket(result) => result.map_err(SendError::Failed),
            _ => Err(SendError::NodeStopped),
        }
    }

    pub async fn send_group_packet(
        &self,
        destination: DestinationHash,
        data: &[u8],
    ) -> Result<(), SendError<SendGroupFailure>> {
        let payload =
            SendGroupPayload::from_slice(data).map_err(|()| SendError::PayloadTooLarge)?;
        let id = self.pool.mint();
        let slot = self.pool.claim_settlement(id).ok_or(SendError::Busy)?;
        let _guard = SlotGuard {
            pool: self.pool,
            slot,
            id,
        };
        self.commands
            .try_send(IssuedCommand {
                id,
                command: PrnsCommand::SendGroup(SendGroup {
                    destination,
                    payload,
                }),
            })
            .map_err(|_| SendError::NodeStopped)?;
        match self.pool.parked(slot).await {
            Settlement::SendGroup(result) => result.map_err(SendError::Failed),
            _ => Err(SendError::NodeStopped),
        }
    }

    pub async fn announce_now(
        &self,
        announce: crate::engine::AnnounceNow,
    ) -> Result<(), AnnounceNowError> {
        let id = self.pool.mint();
        let slot = self
            .pool
            .claim_settlement(id)
            .ok_or(AnnounceNowError::Busy)?;
        let _guard = SlotGuard {
            pool: self.pool,
            slot,
            id,
        };
        self.commands
            .try_send(IssuedCommand {
                id,
                command: PrnsCommand::AnnounceNow(announce),
            })
            .map_err(|_| AnnounceNowError::NodeStopped)?;
        match self.pool.parked(slot).await {
            Settlement::AnnounceNow(Ok(())) => Ok(()),
            Settlement::AnnounceNow(Err(failure)) => Err(AnnounceNowError::from_failure(failure)),
            _ => Err(AnnounceNowError::NodeStopped),
        }
    }

    /// Responds inline; returns `false` when the body exceeds the link MDU or the command lane is full.
    pub fn respond_packed(&self, responder: RespondToken, packed: &[u8]) -> bool {
        match RespondData::from_slice(packed) {
            Ok(data) => self.respond_owned_packed(responder, data),
            Err(_) => false,
        }
    }

    /// Moves a prebuilt response into the command lane, returning `false` when full.
    pub fn respond_owned_packed(&self, responder: RespondToken, data: RespondData) -> bool {
        self.issue(PrnsCommand::Respond(Respond {
            link_id: responder.link_id,
            request_id: responder.request_id,
            payload: RespondPayload::Packed(data),
        }))
        .is_some()
    }

    pub fn respond_static_bytes(&self, responder: RespondToken, data: &'static [u8]) -> bool {
        self.issue(PrnsCommand::Respond(Respond {
            link_id: responder.link_id,
            request_id: responder.request_id,
            payload: RespondPayload::StaticBytes(data),
        }))
        .is_some()
    }

    #[cfg(feature = "large-static-responses")]
    pub fn respond_static_file(
        &self,
        responder: RespondToken,
        name: &'static str,
        bytes: &'static [u8],
    ) -> bool {
        self.issue(PrnsCommand::Respond(Respond {
            link_id: responder.link_id,
            request_id: responder.request_id,
            payload: RespondPayload::StaticFile { name, bytes },
        }))
        .is_some()
    }

    /// Sever an active link. Returns `false` if the command lane is full.
    pub fn close_link(&self, link_id: LinkId) -> bool {
        self.issue(PrnsCommand::CloseLink(CloseLink { link_id }))
            .is_some()
    }

    pub async fn request(
        &self,
        link_id: LinkId,
        path_hash: RequestPathHash,
        data: &[u8],
    ) -> Result<(RequestResponseData<RESPONSE_BYTES>, RttMillis), SendError<SendRequestFailure>>
    {
        self.request_with_response_timeout(
            link_id,
            path_hash,
            data,
            RequestResponseTimeout::LinkDefault,
        )
        .await
    }

    pub async fn request_with_response_timeout(
        &self,
        link_id: LinkId,
        path_hash: RequestPathHash,
        data: &[u8],
        response_timeout: RequestResponseTimeout,
    ) -> Result<(RequestResponseData<RESPONSE_BYTES>, RttMillis), SendError<SendRequestFailure>>
    {
        self.request_with_maximum_response_bytes::<RESPONSE_BYTES>(
            link_id,
            path_hash,
            data,
            response_timeout,
        )
        .await
    }

    pub(super) async fn request_with_maximum_response_bytes<const MAXIMUM_RESPONSE_BYTES: usize>(
        &self,
        link_id: LinkId,
        path_hash: RequestPathHash,
        data: &[u8],
        response_timeout: RequestResponseTimeout,
    ) -> Result<(RequestResponseData<RESPONSE_BYTES>, RttMillis), SendError<SendRequestFailure>>
    {
        const {
            assert!(
                REQUEST_COMPLETIONS > 0,
                "CompletionPool needs at least one request completion slot"
            );
            assert!(
                MAXIMUM_RESPONSE_BYTES <= RESPONSE_BYTES,
                "CompletionPool response capacity is smaller than the requested bound"
            );
        }
        let data = SendRequestData::from_slice(data).map_err(|()| SendError::PayloadTooLarge)?;
        let id = self.pool.mint();
        let slot = self.pool.claim_request(id).ok_or(SendError::Busy)?;
        let _guard = RequestSlotGuard {
            pool: self.pool,
            slot,
            id,
        };
        self.commands
            .try_send(IssuedCommand {
                id,
                command: PrnsCommand::SendRequest(SendRequest {
                    link_id,
                    path_hash,
                    data,
                    response_timeout,
                    maximum_response_bytes: ByteLimit::Maximum(MAXIMUM_RESPONSE_BYTES as u64),
                }),
            })
            .map_err(|_| SendError::NodeStopped)?;
        match self.pool.parked_request(slot).await {
            Settlement::SendRequest(Ok(delivered)) => self
                .pool
                .take_request_response(slot, id)
                .map(|response| (response, delivered.rtt))
                .map_err(SendError::Failed),
            Settlement::SendRequest(Err(failure)) => Err(SendError::Failed(failure)),
            _ => Err(SendError::NodeStopped),
        }
    }

    pub(super) fn route_journaled(&self, journaled: &Journaled<'_>) -> JournalRoute {
        match journaled {
            Journaled::ResponseReceived {
                command_id, data, ..
            }
            | Journaled::ResponseSegmentReceived {
                command_id, data, ..
            } => self.pool.capture_response(*command_id, data),
            Journaled::CommandSettled { id, settlement } => {
                if self.pool.settle_request(*id, settlement.clone())
                    || self.pool.settle(*id, settlement.clone())
                {
                    JournalRoute::Awaiter
                } else {
                    JournalRoute::Application
                }
            }
            _ => JournalRoute::Application,
        }
    }
}

struct SlotGuard<
    'a,
    M: RawMutex,
    const COMPLETIONS: usize,
    const REQUEST_COMPLETIONS: usize,
    const RESPONSE_BYTES: usize,
> {
    pool: &'a CompletionPool<M, COMPLETIONS, REQUEST_COMPLETIONS, RESPONSE_BYTES>,
    slot: usize,
    id: CommandId,
}

impl<
        M: RawMutex,
        const COMPLETIONS: usize,
        const REQUEST_COMPLETIONS: usize,
        const RESPONSE_BYTES: usize,
    > Drop for SlotGuard<'_, M, COMPLETIONS, REQUEST_COMPLETIONS, RESPONSE_BYTES>
{
    fn drop(&mut self) {
        self.pool.release(self.slot, self.id);
    }
}

struct RequestSlotGuard<
    'a,
    M: RawMutex,
    const COMPLETIONS: usize,
    const REQUEST_COMPLETIONS: usize,
    const RESPONSE_BYTES: usize,
> {
    pool: &'a CompletionPool<M, COMPLETIONS, REQUEST_COMPLETIONS, RESPONSE_BYTES>,
    slot: usize,
    id: CommandId,
}

impl<
        M: RawMutex,
        const COMPLETIONS: usize,
        const REQUEST_COMPLETIONS: usize,
        const RESPONSE_BYTES: usize,
    > Drop for RequestSlotGuard<'_, M, COMPLETIONS, REQUEST_COMPLETIONS, RESPONSE_BYTES>
{
    fn drop(&mut self) {
        self.pool.release_request(self.slot, self.id);
    }
}

impl<
        M: RawMutex,
        const COMMANDS: usize,
        const COMPLETIONS: usize,
        const REQUEST_COMPLETIONS: usize,
        const RESPONSE_BYTES: usize,
    > PrnsNodeApi
    for PrnsNodeHandle<'_, M, COMMANDS, COMPLETIONS, REQUEST_COMPLETIONS, RESPONSE_BYTES>
{
    fn issue(&self, command: PrnsCommand) -> Option<CommandId> {
        self.issue(command)
    }

    async fn announce_now(
        &self,
        announce: crate::engine::AnnounceNow,
    ) -> Result<(), AnnounceNowError> {
        self.announce_now(announce).await
    }

    async fn send_single_packet(
        &self,
        destination: DestinationHash,
        data: &[u8],
    ) -> Result<PacketReceiptDelivered, SendError<SendSinglePacketFailure>> {
        self.send_single_packet(destination, data).await
    }

    async fn send_plain_packet(
        &self,
        destination: DestinationHash,
        data: &[u8],
    ) -> Result<(), SendError<SendPlainPacketFailure>> {
        self.send_plain_packet(destination, data).await
    }

    async fn send_group_packet(
        &self,
        destination: DestinationHash,
        data: &[u8],
    ) -> Result<(), SendError<SendGroupFailure>> {
        self.send_group_packet(destination, data).await
    }

    fn respond_packed(&self, responder: RespondToken, packed: &[u8]) -> bool {
        self.respond_packed(responder, packed)
    }

    fn close_link(&self, link_id: LinkId) -> bool {
        self.close_link(link_id)
    }
}

#[cfg(test)]
mod tests;
