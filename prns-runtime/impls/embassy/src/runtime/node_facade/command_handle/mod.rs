use core::cell::RefCell;

use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_sync::blocking_mutex::Mutex as BlockingMutex;
use embassy_sync::channel::Sender;
use embassy_sync::signal::Signal;
use portable_atomic::{AtomicU64, Ordering};

use crate::engine::{
    ApproveRemoteControlControllerPairing, ApproveRemoteControlTargetPairing,
    BeginRemoteControlControllerPairing, CloseLink, CloseRemoteControlPairing,
    CloseRemoteControlPairingOutcome, CommandId, EgressTarget, EstablishLink, EstablishLinkFailure,
    Identify, IdentifyFailure, IssuedCommand, Journaled, OpenRemoteControlPairing,
    PacketReceiptDelivered, PrnsCommand, RejectRemoteControlControllerPairing,
    RejectRemoteControlTargetPairing, RemoteControlPairingOpened, RequestResponseTimeout, Respond,
    RespondData, RespondPayload, SendGroup, SendGroupFailure, SendGroupPayload, SendPlainPacket,
    SendPlainPacketFailure, SendPlainPacketPayload, SendRequest, SendRequestData,
    SendRequestFailure, SendSinglePacket, SendSinglePacketFailure, SendSinglePacketPayload,
    SetRegisteredAnnounceAppData, Settleable, Settlement,
};
use crate::identity::IdentityHash;
use crate::remote_control::{
    ForgetRemoteControlTargetOutcome, RemoteControlControllerGrant,
    RemoteControlControllerIdentity, RemoteControlTargetAccess, RemoteControlTargetIdentity,
    RevokeRemoteControlControllerOutcome, SetRemoteControlControllerGrantOutcome,
    SetRemoteControlTargetAccessOutcome,
};
use crate::routing::links::LinkId;
use crate::routing::request_handlers::RequestPathHash;
use crate::units::{ByteLimit, RttMillis};
use crate::wire::DestinationHash;

use super::super::remote_control_controller_grants::{
    RemoteControlControllerGrantCommand, RemoteControlControllerGrantCompletion,
    RemoteControlControllerGrantExchange,
};
use super::super::remote_control_pairing_authorizations::{
    RemoteControlPairingAuthorization, RemoteControlPairingAuthorizationCommand,
    RemoteControlPairingAuthorizationCompletion, RemoteControlPairingAuthorizationExchange,
    RemoteControlPairingAuthorizationTransactionFailure,
};
use super::super::remote_control_target_accesses::{
    RemoteControlTargetAccessCommand, RemoteControlTargetAccessCompletion,
    RemoteControlTargetAccessExchange,
};
use super::super::request_endpoints::RespondToken;
use super::super::{
    AnnounceNowError, ApproveRemoteControlControllerPairingControlError,
    ApproveRemoteControlControllerPairingControlFailure,
    ApproveRemoteControlTargetPairingControlError, BeginRemoteControlControllerPairingControlError,
    BeginRemoteControlControllerPairingControlFailure, CloseRemoteControlPairingControlError,
    ForgetRemoteControlTargetControlError, OpenRemoteControlPairingControlError, PrnsNodeApi,
    RejectRemoteControlControllerPairingControlError, RejectRemoteControlTargetPairingControlError,
    RemoteControlControllerGrantControl, RemoteControlControllerPairingInitiationTransport,
    RemoteControlPairingControl, RemoteControlPairingControlError,
    RemoteControlPairingLinkCleanupOutcome, RemoteControlTargetAccessControl,
    RemoteControlTargetInventory, RemoteControlTargetInventoryControlError,
    ResolveRemoteControlTargetControlError, ResolvedRemoteControlTarget,
    RevokeRemoteControlControllerControlError, SendError, SetRegisteredAnnounceAppDataError,
    SetRemoteControlControllerGrantControlError, SetRemoteControlTargetAccessControlError,
};

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
    remote_control_controller_grants: RemoteControlControllerGrantExchange<M>,
    remote_control_target_accesses: RemoteControlTargetAccessExchange<M>,
    remote_control_pairing_authorizations: RemoteControlPairingAuthorizationExchange<M>,
    remote_control_pairing_settlement: RemoteControlPairingSettlementAwaiter<M>,
}

enum RemoteControlPairingSettlementState {
    Available,
    Awaiting(CommandId),
    Settled(CommandId),
    Completing(CommandId),
}

struct RemoteControlPairingSettlementAwaiter<M: RawMutex> {
    state: BlockingMutex<M, RefCell<RemoteControlPairingSettlementState>>,
    ready: Signal<M, Settlement>,
}

impl<M: RawMutex> RemoteControlPairingSettlementAwaiter<M> {
    const fn new() -> Self {
        Self {
            state: BlockingMutex::new(RefCell::new(RemoteControlPairingSettlementState::Available)),
            ready: Signal::new(),
        }
    }

    fn claim(&self, id: CommandId) -> bool {
        self.state.lock(|state| {
            let mut state = state.borrow_mut();
            if !matches!(*state, RemoteControlPairingSettlementState::Available) {
                return false;
            }
            self.ready.reset();
            *state = RemoteControlPairingSettlementState::Awaiting(id);
            true
        })
    }

    fn route(
        &self,
        id: CommandId,
        settlement: Settlement,
        on_unclaimed: impl FnOnce(Settlement),
    ) -> JournalRoute {
        let settled = self.state.lock(|state| {
            let mut state = state.borrow_mut();
            if !matches!(*state, RemoteControlPairingSettlementState::Awaiting(awaited) if awaited == id)
            {
                return false;
            }
            *state = RemoteControlPairingSettlementState::Settled(id);
            true
        });
        if !settled {
            on_unclaimed(settlement);
            return JournalRoute::Application;
        }
        self.ready.signal(settlement);
        JournalRoute::Awaiter
    }

    async fn completion(&self, id: CommandId) -> Settlement {
        let settlement = self.ready.wait().await;
        self.state.lock(|state| {
            let mut state = state.borrow_mut();
            if matches!(*state, RemoteControlPairingSettlementState::Settled(settled) if settled == id)
            {
                *state = RemoteControlPairingSettlementState::Completing(id);
            }
        });
        settlement
    }

    fn release(&self, id: CommandId) {
        self.state.lock(|state| {
            let mut state = state.borrow_mut();
            let belongs = match &*state {
                RemoteControlPairingSettlementState::Available => false,
                RemoteControlPairingSettlementState::Awaiting(awaited)
                | RemoteControlPairingSettlementState::Settled(awaited)
                | RemoteControlPairingSettlementState::Completing(awaited) => *awaited == id,
            };
            if belongs {
                *state = RemoteControlPairingSettlementState::Available;
                self.ready.reset();
            }
        });
    }
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

enum ResponseCapture {
    NotAwaited,
    Captured,
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
            remote_control_controller_grants: RemoteControlControllerGrantExchange::new(),
            remote_control_target_accesses: RemoteControlTargetAccessExchange::new(),
            remote_control_pairing_authorizations: RemoteControlPairingAuthorizationExchange::new(),
            remote_control_pairing_settlement: RemoteControlPairingSettlementAwaiter::new(),
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

    #[cfg(test)]
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

    #[cfg(test)]
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

    fn route_settlement(
        &self,
        id: CommandId,
        settlement: Settlement,
        on_unclaimed: impl FnOnce(Settlement),
    ) -> JournalRoute {
        let request_slot = self
            .requests
            .lock(|cell| cell.borrow().iter().position(|entry| entry.awaits(id)));
        if let Some(slot) = request_slot {
            self.request_slots[slot].signal(settlement);
            return JournalRoute::Awaiter;
        }
        let completion_slot = self
            .awaited
            .lock(|cell| cell.borrow().iter().position(|awaited| *awaited == id.0));
        if let Some(slot) = completion_slot {
            self.slots[slot].signal(settlement);
            return JournalRoute::Awaiter;
        }
        self.remote_control_pairing_settlement
            .route(id, settlement, on_unclaimed)
    }

    fn capture_response(&self, id: CommandId, data: &[u8]) -> ResponseCapture {
        self.requests.lock(|cell| {
            let mut requests = cell.borrow_mut();
            let Some(RequestAwaited::Awaiting { response, .. }) =
                requests.iter_mut().find(|entry| entry.awaits(id))
            else {
                return ResponseCapture::NotAwaited;
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
            ResponseCapture::Captured
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

    async fn settle_command<C>(&self, command: C) -> Result<C::Success, SendError<C::Failure>>
    where
        C: Settleable,
    {
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
                command: command.into_command(),
            })
            .map_err(|_| SendError::NodeStopped)?;
        C::from_settlement(self.pool.parked(slot).await)
            .ok_or(SendError::NodeStopped)?
            .map_err(SendError::Failed)
    }

    pub async fn establish_link(
        &self,
        destination: DestinationHash,
    ) -> Result<LinkId, SendError<EstablishLinkFailure>> {
        self.settle_command(EstablishLink { destination })
            .await
            .map(|established| established.link_id)
    }

    pub async fn identify(
        &self,
        link_id: LinkId,
        identity: IdentityHash,
    ) -> Result<(), SendError<IdentifyFailure>> {
        self.settle_command(Identify { link_id, identity }).await
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
                    target: EgressTarget::AllInterfaces,
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

    pub async fn set_registered_announce_app_data(
        &self,
        set: SetRegisteredAnnounceAppData,
    ) -> Result<(), SetRegisteredAnnounceAppDataError> {
        let id = self.pool.mint();
        let slot = self
            .pool
            .claim_settlement(id)
            .ok_or(SetRegisteredAnnounceAppDataError::Busy)?;
        let _guard = SlotGuard {
            pool: self.pool,
            slot,
            id,
        };
        self.commands
            .try_send(IssuedCommand {
                id,
                command: PrnsCommand::SetRegisteredAnnounceAppData(set),
            })
            .map_err(|_| SetRegisteredAnnounceAppDataError::NodeStopped)?;
        match self.pool.parked(slot).await {
            Settlement::SetRegisteredAnnounceAppData(Ok(())) => Ok(()),
            Settlement::SetRegisteredAnnounceAppData(Err(failure)) => {
                Err(SetRegisteredAnnounceAppDataError::from_failure(failure))
            }
            _ => Err(SetRegisteredAnnounceAppDataError::NodeStopped),
        }
    }

    pub async fn open_remote_control_pairing(
        &self,
        open: OpenRemoteControlPairing,
    ) -> Result<RemoteControlPairingOpened, OpenRemoteControlPairingControlError> {
        let id = self.pool.mint();
        let slot = self
            .pool
            .claim_settlement(id)
            .ok_or(RemoteControlPairingControlError::Busy)?;
        let _guard = SlotGuard {
            pool: self.pool,
            slot,
            id,
        };
        self.commands
            .try_send(IssuedCommand {
                id,
                command: PrnsCommand::OpenRemoteControlPairing(open),
            })
            .map_err(|_| RemoteControlPairingControlError::NodeStopped)?;
        match self.pool.parked(slot).await {
            Settlement::OpenRemoteControlPairing(result) => {
                result.map_err(RemoteControlPairingControlError::Failed)
            }
            _ => Err(RemoteControlPairingControlError::NodeStopped),
        }
    }

    pub async fn close_remote_control_pairing(
        &self,
    ) -> Result<CloseRemoteControlPairingOutcome, CloseRemoteControlPairingControlError> {
        let id = self.pool.mint();
        let slot = self
            .pool
            .claim_settlement(id)
            .ok_or(RemoteControlPairingControlError::Busy)?;
        let _guard = SlotGuard {
            pool: self.pool,
            slot,
            id,
        };
        self.commands
            .try_send(IssuedCommand {
                id,
                command: PrnsCommand::CloseRemoteControlPairing(CloseRemoteControlPairing),
            })
            .map_err(|_| RemoteControlPairingControlError::NodeStopped)?;
        match self.pool.parked(slot).await {
            Settlement::CloseRemoteControlPairing(result) => {
                result.map_err(RemoteControlPairingControlError::Failed)
            }
            _ => Err(RemoteControlPairingControlError::NodeStopped),
        }
    }

    pub(in crate::runtime) async fn settle_pairing_command<C>(
        &self,
        command: C,
    ) -> Result<C::Success, RemoteControlPairingControlError<C::Failure>>
    where
        C: Settleable,
    {
        let id = self.pool.mint();
        if !self.pool.remote_control_pairing_settlement.claim(id) {
            return Err(RemoteControlPairingControlError::Busy);
        }
        let _guard = RemoteControlPairingSettlementGuard {
            pool: self.pool,
            id,
        };
        self.commands
            .send(IssuedCommand {
                id,
                command: command.into_command(),
            })
            .await;
        let settlement = self
            .pool
            .remote_control_pairing_settlement
            .completion(id)
            .await;
        let Some(result) = C::from_settlement(settlement) else {
            return Err(RemoteControlPairingControlError::NodeStopped);
        };
        result.map_err(RemoteControlPairingControlError::Failed)
    }

    async fn run_remote_control_pairing_authorization_command(
        &self,
        build: impl FnOnce(CommandId) -> RemoteControlPairingAuthorizationCommand,
    ) -> Result<
        RemoteControlPairingAuthorizationCompletion,
        RemoteControlPairingAuthorizationTransactionFailure,
    > {
        let id = self.pool.mint();
        if !self
            .pool
            .remote_control_pairing_authorizations
            .submit(build(id))
        {
            return Err(RemoteControlPairingAuthorizationTransactionFailure::RuntimeState);
        }
        let _guard = RemoteControlPairingAuthorizationSlotGuard {
            pool: self.pool,
            id,
        };
        Ok(self
            .pool
            .remote_control_pairing_authorizations
            .completion(id)
            .await)
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

    pub(super) fn route_journaled<'event, A>(
        &self,
        journaled: Journaled<'event>,
        on_application: A,
    ) -> JournalRoute
    where
        A: FnOnce(Journaled<'event>),
    {
        let response = match &journaled {
            Journaled::ResponseReceived {
                command_id, data, ..
            }
            | Journaled::ResponseSegmentReceived {
                command_id, data, ..
            } => Some((*command_id, *data)),
            _ => None,
        };
        if let Some((command_id, data)) = response {
            match self.pool.capture_response(command_id, data) {
                ResponseCapture::Captured => return JournalRoute::Awaiter,
                ResponseCapture::NotAwaited => {}
            }
        }
        match journaled {
            Journaled::CommandSettled { id, settlement } => {
                self.pool.route_settlement(id, settlement, |settlement| {
                    on_application(Journaled::CommandSettled { id, settlement })
                })
            }
            journaled => {
                on_application(journaled);
                JournalRoute::Application
            }
        }
    }

    pub(in crate::runtime) async fn next_remote_control_controller_grant_command(
        &self,
    ) -> RemoteControlControllerGrantCommand {
        self.pool
            .remote_control_controller_grants
            .next_command()
            .await
    }

    pub(in crate::runtime) fn settle_remote_control_controller_grant(
        &self,
        id: CommandId,
        completion: RemoteControlControllerGrantCompletion,
    ) -> bool {
        self.pool
            .remote_control_controller_grants
            .settle(id, completion)
    }

    pub(in crate::runtime) async fn next_remote_control_target_access_command(
        &self,
    ) -> RemoteControlTargetAccessCommand {
        self.pool
            .remote_control_target_accesses
            .next_command()
            .await
    }

    pub(in crate::runtime) fn settle_remote_control_target_access(
        &self,
        id: CommandId,
        completion: RemoteControlTargetAccessCompletion,
    ) -> bool {
        self.pool
            .remote_control_target_accesses
            .settle(id, completion)
    }

    pub(in crate::runtime) async fn next_remote_control_pairing_authorization_command(
        &self,
    ) -> RemoteControlPairingAuthorizationCommand {
        self.pool
            .remote_control_pairing_authorizations
            .next_command()
            .await
    }

    pub(in crate::runtime) fn settle_remote_control_pairing_authorization(
        &self,
        id: CommandId,
        completion: RemoteControlPairingAuthorizationCompletion,
    ) -> bool {
        self.pool
            .remote_control_pairing_authorizations
            .settle(id, completion)
    }

    pub(in crate::runtime) async fn prepare_remote_control_pairing_authorization(
        &self,
        attempt_id: crate::remote_control::RemoteControlPairingAttemptId,
        authorization: RemoteControlPairingAuthorization,
    ) -> Result<
        super::super::embedded_persistence::RemoteControlAuthorizationSnapshot,
        RemoteControlPairingAuthorizationTransactionFailure,
    > {
        match self
            .run_remote_control_pairing_authorization_command(|id| {
                RemoteControlPairingAuthorizationCommand::Prepare {
                    id,
                    attempt_id,
                    authorization,
                }
            })
            .await?
        {
            RemoteControlPairingAuthorizationCompletion::Prepared(result) => result,
            RemoteControlPairingAuthorizationCompletion::RollbackSnapshot(_)
            | RemoteControlPairingAuthorizationCompletion::Activated(_)
            | RemoteControlPairingAuthorizationCompletion::RolledBack(_)
            | RemoteControlPairingAuthorizationCompletion::Released(_) => {
                Err(RemoteControlPairingAuthorizationTransactionFailure::RuntimeState)
            }
        }
    }

    pub(in crate::runtime) async fn snapshot_remote_control_pairing_authorization_rollback(
        &self,
        attempt_id: crate::remote_control::RemoteControlPairingAttemptId,
    ) -> Result<
        super::super::embedded_persistence::RemoteControlAuthorizationSnapshot,
        RemoteControlPairingAuthorizationTransactionFailure,
    > {
        match self
            .run_remote_control_pairing_authorization_command(|id| {
                RemoteControlPairingAuthorizationCommand::SnapshotRollback { id, attempt_id }
            })
            .await?
        {
            RemoteControlPairingAuthorizationCompletion::RollbackSnapshot(result) => result,
            RemoteControlPairingAuthorizationCompletion::Prepared(_)
            | RemoteControlPairingAuthorizationCompletion::Activated(_)
            | RemoteControlPairingAuthorizationCompletion::RolledBack(_)
            | RemoteControlPairingAuthorizationCompletion::Released(_) => {
                Err(RemoteControlPairingAuthorizationTransactionFailure::RuntimeState)
            }
        }
    }

    pub(in crate::runtime) async fn activate_remote_control_pairing_authorization(
        &self,
        attempt_id: crate::remote_control::RemoteControlPairingAttemptId,
    ) -> Result<(), RemoteControlPairingAuthorizationTransactionFailure> {
        match self
            .run_remote_control_pairing_authorization_command(|id| {
                RemoteControlPairingAuthorizationCommand::Activate { id, attempt_id }
            })
            .await?
        {
            RemoteControlPairingAuthorizationCompletion::Activated(result) => result,
            RemoteControlPairingAuthorizationCompletion::Prepared(_)
            | RemoteControlPairingAuthorizationCompletion::RollbackSnapshot(_)
            | RemoteControlPairingAuthorizationCompletion::RolledBack(_)
            | RemoteControlPairingAuthorizationCompletion::Released(_) => {
                Err(RemoteControlPairingAuthorizationTransactionFailure::RuntimeState)
            }
        }
    }

    pub(in crate::runtime) async fn roll_back_remote_control_pairing_authorization(
        &self,
        attempt_id: crate::remote_control::RemoteControlPairingAttemptId,
    ) -> Result<
        super::super::embedded_persistence::RemoteControlAuthorizationSnapshot,
        RemoteControlPairingAuthorizationTransactionFailure,
    > {
        match self
            .run_remote_control_pairing_authorization_command(|id| {
                RemoteControlPairingAuthorizationCommand::RollBack { id, attempt_id }
            })
            .await?
        {
            RemoteControlPairingAuthorizationCompletion::RolledBack(result) => result,
            RemoteControlPairingAuthorizationCompletion::Prepared(_)
            | RemoteControlPairingAuthorizationCompletion::RollbackSnapshot(_)
            | RemoteControlPairingAuthorizationCompletion::Activated(_)
            | RemoteControlPairingAuthorizationCompletion::Released(_) => {
                Err(RemoteControlPairingAuthorizationTransactionFailure::RuntimeState)
            }
        }
    }

    pub(in crate::runtime) async fn release_remote_control_pairing_authorization(
        &self,
        attempt_id: crate::remote_control::RemoteControlPairingAttemptId,
    ) -> Result<(), RemoteControlPairingAuthorizationTransactionFailure> {
        match self
            .run_remote_control_pairing_authorization_command(|id| {
                RemoteControlPairingAuthorizationCommand::Release { id, attempt_id }
            })
            .await?
        {
            RemoteControlPairingAuthorizationCompletion::Released(result) => result,
            RemoteControlPairingAuthorizationCompletion::Prepared(_)
            | RemoteControlPairingAuthorizationCompletion::RollbackSnapshot(_)
            | RemoteControlPairingAuthorizationCompletion::Activated(_)
            | RemoteControlPairingAuthorizationCompletion::RolledBack(_) => {
                Err(RemoteControlPairingAuthorizationTransactionFailure::RuntimeState)
            }
        }
    }
}

struct RemoteControlPairingSettlementGuard<
    'a,
    M: RawMutex,
    const COMPLETIONS: usize,
    const REQUEST_COMPLETIONS: usize,
    const RESPONSE_BYTES: usize,
> {
    pool: &'a CompletionPool<M, COMPLETIONS, REQUEST_COMPLETIONS, RESPONSE_BYTES>,
    id: CommandId,
}

impl<
        M: RawMutex,
        const COMPLETIONS: usize,
        const REQUEST_COMPLETIONS: usize,
        const RESPONSE_BYTES: usize,
    > Drop
    for RemoteControlPairingSettlementGuard<'_, M, COMPLETIONS, REQUEST_COMPLETIONS, RESPONSE_BYTES>
{
    fn drop(&mut self) {
        self.pool.remote_control_pairing_settlement.release(self.id);
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

struct RemoteControlControllerGrantSlotGuard<
    'a,
    M: RawMutex,
    const COMPLETIONS: usize,
    const REQUEST_COMPLETIONS: usize,
    const RESPONSE_BYTES: usize,
> {
    pool: &'a CompletionPool<M, COMPLETIONS, REQUEST_COMPLETIONS, RESPONSE_BYTES>,
    id: CommandId,
}

struct RemoteControlTargetAccessSlotGuard<
    'a,
    M: RawMutex,
    const COMPLETIONS: usize,
    const REQUEST_COMPLETIONS: usize,
    const RESPONSE_BYTES: usize,
> {
    pool: &'a CompletionPool<M, COMPLETIONS, REQUEST_COMPLETIONS, RESPONSE_BYTES>,
    id: CommandId,
}

struct RemoteControlPairingAuthorizationSlotGuard<
    'a,
    M: RawMutex,
    const COMPLETIONS: usize,
    const REQUEST_COMPLETIONS: usize,
    const RESPONSE_BYTES: usize,
> {
    pool: &'a CompletionPool<M, COMPLETIONS, REQUEST_COMPLETIONS, RESPONSE_BYTES>,
    id: CommandId,
}

impl<
        M: RawMutex,
        const COMPLETIONS: usize,
        const REQUEST_COMPLETIONS: usize,
        const RESPONSE_BYTES: usize,
    > Drop
    for RemoteControlControllerGrantSlotGuard<
        '_,
        M,
        COMPLETIONS,
        REQUEST_COMPLETIONS,
        RESPONSE_BYTES,
    >
{
    fn drop(&mut self) {
        self.pool.remote_control_controller_grants.release(self.id);
    }
}

impl<
        M: RawMutex,
        const COMPLETIONS: usize,
        const REQUEST_COMPLETIONS: usize,
        const RESPONSE_BYTES: usize,
    > Drop
    for RemoteControlPairingAuthorizationSlotGuard<
        '_,
        M,
        COMPLETIONS,
        REQUEST_COMPLETIONS,
        RESPONSE_BYTES,
    >
{
    fn drop(&mut self) {
        self.pool
            .remote_control_pairing_authorizations
            .release(self.id);
    }
}

impl<
        M: RawMutex,
        const COMPLETIONS: usize,
        const REQUEST_COMPLETIONS: usize,
        const RESPONSE_BYTES: usize,
    > Drop
    for RemoteControlTargetAccessSlotGuard<'_, M, COMPLETIONS, REQUEST_COMPLETIONS, RESPONSE_BYTES>
{
    fn drop(&mut self) {
        self.pool.remote_control_target_accesses.release(self.id);
    }
}

impl<
        M: RawMutex + Sync,
        const COMMANDS: usize,
        const COMPLETIONS: usize,
        const REQUEST_COMPLETIONS: usize,
        const RESPONSE_BYTES: usize,
    > RemoteControlControllerGrantControl
    for PrnsNodeHandle<'_, M, COMMANDS, COMPLETIONS, REQUEST_COMPLETIONS, RESPONSE_BYTES>
{
    async fn set_remote_control_controller_grant(
        &self,
        grant: RemoteControlControllerGrant,
    ) -> Result<SetRemoteControlControllerGrantOutcome, SetRemoteControlControllerGrantControlError>
    {
        let id = self.pool.mint();
        let command = RemoteControlControllerGrantCommand::SetControllerGrant { id, grant };
        if !self.pool.remote_control_controller_grants.submit(command) {
            return Err(SetRemoteControlControllerGrantControlError::Busy);
        }
        let _guard = RemoteControlControllerGrantSlotGuard {
            pool: self.pool,
            id,
        };
        match self
            .pool
            .remote_control_controller_grants
            .completion(id)
            .await
        {
            RemoteControlControllerGrantCompletion::ControllerGrantSet(result) => {
                result.map_err(Into::into)
            }
            RemoteControlControllerGrantCompletion::ControllerRevoked(_) => {
                Err(SetRemoteControlControllerGrantControlError::NodeStopped)
            }
        }
    }

    async fn revoke_remote_control_controller(
        &self,
        controller: RemoteControlControllerIdentity,
    ) -> Result<RevokeRemoteControlControllerOutcome, RevokeRemoteControlControllerControlError>
    {
        let id = self.pool.mint();
        let command = RemoteControlControllerGrantCommand::RevokeController { id, controller };
        if !self.pool.remote_control_controller_grants.submit(command) {
            return Err(RevokeRemoteControlControllerControlError::Busy);
        }
        let _guard = RemoteControlControllerGrantSlotGuard {
            pool: self.pool,
            id,
        };
        match self
            .pool
            .remote_control_controller_grants
            .completion(id)
            .await
        {
            RemoteControlControllerGrantCompletion::ControllerRevoked(outcome) => {
                outcome.map_err(Into::into)
            }
            RemoteControlControllerGrantCompletion::ControllerGrantSet(_) => {
                Err(RevokeRemoteControlControllerControlError::NodeStopped)
            }
        }
    }
}

impl<
        M: RawMutex + Sync,
        const COMMANDS: usize,
        const COMPLETIONS: usize,
        const REQUEST_COMPLETIONS: usize,
        const RESPONSE_BYTES: usize,
    > RemoteControlPairingControl
    for PrnsNodeHandle<'_, M, COMMANDS, COMPLETIONS, REQUEST_COMPLETIONS, RESPONSE_BYTES>
{
    async fn begin_remote_control_controller_pairing(
        &self,
        begin: BeginRemoteControlControllerPairing,
    ) -> Result<
        crate::engine::RemoteControlControllerPairingResponseReceived,
        BeginRemoteControlControllerPairingControlError,
    > {
        let link_id = begin.context.link_id();
        let begun = self.settle_pairing_command(begin).await.map_err(|error| {
            error.map_failure(BeginRemoteControlControllerPairingControlFailure::Begin)
        })?;
        if let Err(error) = self
            .identify(link_id, begun.controller_identity_hash())
            .await
        {
            let cleanup = match self.close_link(link_id) {
                true => RemoteControlPairingLinkCleanupOutcome::Queued,
                false => RemoteControlPairingLinkCleanupOutcome::NotQueued,
            };
            return Err(RemoteControlPairingControlError::Failed(
                BeginRemoteControlControllerPairingControlFailure::Identify {
                    failure: error,
                    cleanup,
                },
            ));
        }
        self.settle_pairing_command(begun.into_request())
            .await
            .map_err(|error| {
                error.map_failure(BeginRemoteControlControllerPairingControlFailure::Request)
            })
    }

    async fn approve_remote_control_controller_pairing(
        &self,
        approve: ApproveRemoteControlControllerPairing,
    ) -> Result<
        crate::engine::RemoteControlControllerPairingResponseReceived,
        ApproveRemoteControlControllerPairingControlError,
    > {
        let approval = self
            .settle_pairing_command(approve)
            .await
            .map_err(|error| {
                error.map_failure(ApproveRemoteControlControllerPairingControlFailure::Approve)
            })?;
        self.settle_pairing_command(approval.into_request())
            .await
            .map_err(|error| {
                error.map_failure(ApproveRemoteControlControllerPairingControlFailure::Request)
            })
    }

    async fn reject_remote_control_controller_pairing(
        &self,
        reject: RejectRemoteControlControllerPairing,
    ) -> Result<
        crate::engine::RemoteControlControllerPairingRejection,
        RejectRemoteControlControllerPairingControlError,
    > {
        self.settle_pairing_command(reject).await
    }

    async fn approve_remote_control_target_pairing(
        &self,
        approve: ApproveRemoteControlTargetPairing,
    ) -> Result<
        crate::engine::RemoteControlTargetPairingApproval,
        ApproveRemoteControlTargetPairingControlError,
    > {
        self.settle_pairing_command(approve).await
    }

    async fn reject_remote_control_target_pairing(
        &self,
        reject: RejectRemoteControlTargetPairing,
    ) -> Result<
        crate::engine::RemoteControlTargetPairingRejection,
        RejectRemoteControlTargetPairingControlError,
    > {
        self.settle_pairing_command(reject).await
    }
}

impl<
        M: RawMutex + Sync,
        const COMMANDS: usize,
        const COMPLETIONS: usize,
        const REQUEST_COMPLETIONS: usize,
        const RESPONSE_BYTES: usize,
    > RemoteControlControllerPairingInitiationTransport
    for PrnsNodeHandle<'_, M, COMMANDS, COMPLETIONS, REQUEST_COMPLETIONS, RESPONSE_BYTES>
{
    async fn establish_remote_control_pairing_link(
        &self,
        destination: DestinationHash,
    ) -> Result<LinkId, SendError<EstablishLinkFailure>> {
        self.establish_link(destination).await
    }

    fn close_remote_control_pairing_link(
        &self,
        link_id: LinkId,
    ) -> RemoteControlPairingLinkCleanupOutcome {
        match self.close_link(link_id) {
            true => RemoteControlPairingLinkCleanupOutcome::Queued,
            false => RemoteControlPairingLinkCleanupOutcome::NotQueued,
        }
    }
}

impl<
        M: RawMutex + Sync,
        const COMMANDS: usize,
        const COMPLETIONS: usize,
        const REQUEST_COMPLETIONS: usize,
        const RESPONSE_BYTES: usize,
    > RemoteControlTargetAccessControl
    for PrnsNodeHandle<'_, M, COMMANDS, COMPLETIONS, REQUEST_COMPLETIONS, RESPONSE_BYTES>
{
    async fn remote_control_target_inventory(
        &self,
    ) -> Result<RemoteControlTargetInventory, RemoteControlTargetInventoryControlError> {
        let id = self.pool.mint();
        let command = RemoteControlTargetAccessCommand::Inventory { id };
        if !self.pool.remote_control_target_accesses.submit(command) {
            return Err(RemoteControlTargetInventoryControlError::Busy);
        }
        let _guard = RemoteControlTargetAccessSlotGuard {
            pool: self.pool,
            id,
        };
        match self
            .pool
            .remote_control_target_accesses
            .completion(id)
            .await
        {
            RemoteControlTargetAccessCompletion::Inventory(result) => result.map_err(Into::into),
            RemoteControlTargetAccessCompletion::Resolved(_)
            | RemoteControlTargetAccessCompletion::AccessSet(_)
            | RemoteControlTargetAccessCompletion::Forgotten(_) => {
                Err(RemoteControlTargetInventoryControlError::NodeStopped)
            }
        }
    }

    async fn resolve_remote_control_target(
        &self,
        target: IdentityHash,
    ) -> Result<ResolvedRemoteControlTarget, ResolveRemoteControlTargetControlError> {
        let id = self.pool.mint();
        let command = RemoteControlTargetAccessCommand::ResolveTarget { id, target };
        if !self.pool.remote_control_target_accesses.submit(command) {
            return Err(ResolveRemoteControlTargetControlError::Busy);
        }
        let _guard = RemoteControlTargetAccessSlotGuard {
            pool: self.pool,
            id,
        };
        match self
            .pool
            .remote_control_target_accesses
            .completion(id)
            .await
        {
            RemoteControlTargetAccessCompletion::Resolved(result) => result.map_err(Into::into),
            RemoteControlTargetAccessCompletion::Inventory(_)
            | RemoteControlTargetAccessCompletion::AccessSet(_)
            | RemoteControlTargetAccessCompletion::Forgotten(_) => {
                Err(ResolveRemoteControlTargetControlError::NodeStopped)
            }
        }
    }

    async fn set_remote_control_target_access(
        &self,
        access: RemoteControlTargetAccess,
    ) -> Result<SetRemoteControlTargetAccessOutcome, SetRemoteControlTargetAccessControlError> {
        let id = self.pool.mint();
        let command = RemoteControlTargetAccessCommand::SetTargetAccess { id, access };
        if !self.pool.remote_control_target_accesses.submit(command) {
            return Err(SetRemoteControlTargetAccessControlError::Busy);
        }
        let _guard = RemoteControlTargetAccessSlotGuard {
            pool: self.pool,
            id,
        };
        match self
            .pool
            .remote_control_target_accesses
            .completion(id)
            .await
        {
            RemoteControlTargetAccessCompletion::AccessSet(result) => result.map_err(Into::into),
            RemoteControlTargetAccessCompletion::Inventory(_)
            | RemoteControlTargetAccessCompletion::Resolved(_)
            | RemoteControlTargetAccessCompletion::Forgotten(_) => {
                Err(SetRemoteControlTargetAccessControlError::NodeStopped)
            }
        }
    }

    async fn forget_remote_control_target(
        &self,
        target: RemoteControlTargetIdentity,
    ) -> Result<ForgetRemoteControlTargetOutcome, ForgetRemoteControlTargetControlError> {
        let id = self.pool.mint();
        let command = RemoteControlTargetAccessCommand::ForgetTarget { id, target };
        if !self.pool.remote_control_target_accesses.submit(command) {
            return Err(ForgetRemoteControlTargetControlError::Busy);
        }
        let _guard = RemoteControlTargetAccessSlotGuard {
            pool: self.pool,
            id,
        };
        match self
            .pool
            .remote_control_target_accesses
            .completion(id)
            .await
        {
            RemoteControlTargetAccessCompletion::Forgotten(result) => result.map_err(Into::into),
            RemoteControlTargetAccessCompletion::Inventory(_)
            | RemoteControlTargetAccessCompletion::Resolved(_)
            | RemoteControlTargetAccessCompletion::AccessSet(_) => {
                Err(ForgetRemoteControlTargetControlError::NodeStopped)
            }
        }
    }
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

    async fn set_registered_announce_app_data(
        &self,
        set: SetRegisteredAnnounceAppData,
    ) -> Result<(), SetRegisteredAnnounceAppDataError> {
        self.set_registered_announce_app_data(set).await
    }

    async fn open_remote_control_pairing(
        &self,
        open: OpenRemoteControlPairing,
    ) -> Result<RemoteControlPairingOpened, OpenRemoteControlPairingControlError> {
        self.open_remote_control_pairing(open).await
    }

    async fn close_remote_control_pairing(
        &self,
    ) -> Result<CloseRemoteControlPairingOutcome, CloseRemoteControlPairingControlError> {
        self.close_remote_control_pairing().await
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
