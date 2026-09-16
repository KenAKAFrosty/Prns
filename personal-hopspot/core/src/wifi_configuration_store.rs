use embedded_storage_async::nor_flash::NorFlash;
use personal_rns::crypto::{sealed_len, token_open, token_seal, TokenKey, TokenOpenError};
use personal_rns::identity::IdentityHash;
use personal_rns::remote_control::{
    RemoteControlTargetSealingKey, RemoteControlWifiCredentialRevision, RemoteControlWifiStation,
    REMOTE_CONTROL_WIFI_PASSWORD_CAP, REMOTE_CONTROL_WIFI_SSID_CAP,
};
use prns_core::entropy::{EntropySource, RuntimeEntropy};
use zeroize::{Zeroize, Zeroizing};

/// Supplies fresh bytes without exposing or copying the runtime's authoritative random stream.
pub trait WifiConfigurationEntropy {
    fn fill_random(&mut self, output: &mut [u8]);
}

impl<S: EntropySource> WifiConfigurationEntropy for RuntimeEntropy<S> {
    fn fill_random(&mut self, output: &mut [u8]) {
        Self::fill_random(self, output);
    }
}

#[cfg(feature = "embedded")]
impl<M, S> WifiConfigurationEntropy for personal_rns::runtime::EntropyHandle<M, S>
where
    M: embassy_sync::blocking_mutex::raw::RawMutex + Sync + 'static,
    S: EntropySource + Send + 'static,
{
    fn fill_random(&mut self, output: &mut [u8]) {
        (*self).fill_random(output);
    }
}

pub const WIFI_CONFIGURATION_SEALING_DOMAIN: &[u8] =
    b"personal-hopspot/remote-control/wifi-configuration/v1";

const MAGIC: [u8; 4] = *b"HWC1";
const SCHEMA_VERSION: u16 = 1;
const HEADER_LEN: usize = 16;
const COMMIT_OFFSET: usize = HEADER_LEN;
const COMMIT_LEN: usize = 16;
const TOKEN_OFFSET: usize = COMMIT_OFFSET + COMMIT_LEN;
const COMMIT_MARKER: [u8; COMMIT_LEN] = [
    0x48, 0x4f, 0x50, 0x57, 0x49, 0x46, 0x49, 0x2d, 0x43, 0x4f, 0x4d, 0x4d, 0x49, 0x54, 0x31, 0x21,
];
const REVISION_LEN: usize = 4;
const CONTROLLER_LEN: usize = 16;
const CREDENTIAL_LEN: usize =
    1 + REMOTE_CONTROL_WIFI_SSID_CAP + 1 + REMOTE_CONTROL_WIFI_PASSWORD_CAP;
const PLAIN_LEN: usize = 1
    + 8 // generation
    + REVISION_LEN // next revision
    + 1 // confirmed present
    + REVISION_LEN
    + CREDENTIAL_LEN
    + 1 // candidate present
    + REVISION_LEN
    + CONTROLLER_LEN
    + CREDENTIAL_LEN;
const TOKEN_LEN: usize = sealed_len(PLAIN_LEN);
const RECORD_LEN: usize = TOKEN_OFFSET + TOKEN_LEN;
const IV_LEN: usize = 16;

const STATE_FACTORY: u8 = 0;
const STATE_CONFIRMED: u8 = 1;
const STATE_STAGED: u8 = 2;
const STATE_AWAITING_CONFIRMATION: u8 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WifiConfigurationFlashOperation {
    Read,
    Erase,
    WriteRecord,
    VerifyRecord,
    WriteCommit,
    VerifyCommit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WifiConfigurationStoreError<E> {
    Flash {
        operation: WifiConfigurationFlashOperation,
        error: E,
    },
    InvalidLayout,
    AuthenticationFailed,
    CorruptRecord,
    VerificationFailed,
    Busy,
    NoTransaction,
    ControllerMismatch,
    RevisionMismatch,
    RevisionExhausted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WifiConfigurationCommitOutcome<E, T> {
    Committed(T),
    NotCommitted(WifiConfigurationStoreError<E>),
    Indeterminate(WifiConfigurationStoreError<E>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WifiConfigurationTransactionPhase {
    Staged,
    AwaitingConfirmation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WifiConfigurationStatus {
    FactoryProvisioning,
    Confirmed {
        revision: RemoteControlWifiCredentialRevision,
    },
    Transaction {
        revision: RemoteControlWifiCredentialRevision,
        controller: IdentityHash,
        phase: WifiConfigurationTransactionPhase,
    },
}

pub struct LoadedWifiConfiguration {
    pub active: Option<RemoteControlWifiStation>,
    pub status: WifiConfigurationStatus,
    pub recovered_unconfirmed_transaction: bool,
}

impl core::fmt::Debug for LoadedWifiConfiguration {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("LoadedWifiConfiguration")
            .field(
                "active",
                &self.active.as_ref().map(|station| station.ssid()),
            )
            .field("status", &self.status)
            .field(
                "recovered_unconfirmed_transaction",
                &self.recovered_unconfirmed_transaction,
            )
            .finish()
    }
}

struct StoredCredential {
    revision: RemoteControlWifiCredentialRevision,
    station: RemoteControlWifiStation,
}

struct CandidateCredential {
    controller: IdentityHash,
    credential: StoredCredential,
}

struct StoredState {
    generation: u64,
    next_revision: u32,
    confirmed: Option<StoredCredential>,
    candidate: Option<CandidateCredential>,
    phase: Option<WifiConfigurationTransactionPhase>,
}

impl StoredState {
    const fn factory() -> Self {
        Self {
            generation: 0,
            next_revision: 1,
            confirmed: None,
            candidate: None,
            phase: None,
        }
    }

    fn status(
        &self,
    ) -> Result<WifiConfigurationStatus, WifiConfigurationStoreError<core::convert::Infallible>>
    {
        match (&self.candidate, self.phase) {
            (Some(candidate), Some(phase)) => Ok(WifiConfigurationStatus::Transaction {
                revision: candidate.credential.revision,
                controller: candidate.controller,
                phase,
            }),
            (None, None) => Ok(self.confirmed.as_ref().map_or(
                WifiConfigurationStatus::FactoryProvisioning,
                |confirmed| WifiConfigurationStatus::Confirmed {
                    revision: confirmed.revision,
                },
            )),
            _ => Err(WifiConfigurationStoreError::CorruptRecord),
        }
    }

    fn without_candidate(mut self) -> Self {
        self.candidate = None;
        self.phase = None;
        self
    }
}

// Keeping the decoded record inline avoids allocator requirements and makes peak embedded memory
// statically visible; only two slots are ever held while selecting the newest valid generation.
#[allow(clippy::large_enum_variant)]
enum Slot {
    Erased,
    Invalid,
    Valid(StoredState),
}

pub struct WifiConfigurationStore<F> {
    flash: F,
    pages: [u32; 2],
}

impl<F> WifiConfigurationStore<F>
where
    F: NorFlash,
{
    #[must_use]
    pub const fn new(flash: F, pages: [u32; 2]) -> Self {
        Self { flash, pages }
    }

    pub fn into_flash(self) -> F {
        self.flash
    }

    pub async fn load(
        &mut self,
        key: &RemoteControlTargetSealingKey,
        entropy: &mut impl WifiConfigurationEntropy,
    ) -> Result<LoadedWifiConfiguration, WifiConfigurationStoreError<F::Error>> {
        let state = self
            .read_active(key)
            .await?
            .unwrap_or_else(StoredState::factory);
        let recovered = state.candidate.is_some();
        let mut state = if recovered {
            state.without_candidate()
        } else {
            state
        };
        if recovered {
            state.generation = state.generation.wrapping_add(1);
            let outcome = self.commit_state(&state, key, entropy).await;
            match outcome {
                WifiConfigurationCommitOutcome::Committed(()) => {}
                WifiConfigurationCommitOutcome::NotCommitted(error)
                | WifiConfigurationCommitOutcome::Indeterminate(error) => return Err(error),
            }
        }
        let status = status_infallible(&state)?;
        Ok(LoadedWifiConfiguration {
            active: state.confirmed.map(|confirmed| confirmed.station),
            status,
            recovered_unconfirmed_transaction: recovered,
        })
    }

    pub async fn status(
        &mut self,
        controller: IdentityHash,
        key: &RemoteControlTargetSealingKey,
    ) -> Result<WifiConfigurationStatus, WifiConfigurationStoreError<F::Error>> {
        let state = self
            .read_active(key)
            .await?
            .unwrap_or_else(StoredState::factory);
        let status = status_infallible(&state)?;
        if let WifiConfigurationStatus::Transaction {
            controller: owner, ..
        } = status
        {
            if owner != controller {
                return Err(WifiConfigurationStoreError::ControllerMismatch);
            }
        }
        Ok(status)
    }

    pub async fn stage(
        &mut self,
        controller: IdentityHash,
        station: RemoteControlWifiStation,
        key: &RemoteControlTargetSealingKey,
        entropy: &mut impl WifiConfigurationEntropy,
    ) -> WifiConfigurationCommitOutcome<F::Error, RemoteControlWifiCredentialRevision> {
        let mut state = match self.read_active(key).await {
            Ok(Some(state)) => state,
            Ok(None) => StoredState::factory(),
            Err(error) => return WifiConfigurationCommitOutcome::NotCommitted(error),
        };
        if state
            .candidate
            .as_ref()
            .is_some_and(|candidate| candidate.controller != controller)
        {
            return WifiConfigurationCommitOutcome::NotCommitted(WifiConfigurationStoreError::Busy);
        }
        let Some(revision) = RemoteControlWifiCredentialRevision::new(state.next_revision) else {
            return WifiConfigurationCommitOutcome::NotCommitted(
                WifiConfigurationStoreError::RevisionExhausted,
            );
        };
        let Some(next_revision) = state.next_revision.checked_add(1) else {
            return WifiConfigurationCommitOutcome::NotCommitted(
                WifiConfigurationStoreError::RevisionExhausted,
            );
        };
        state.next_revision = next_revision;
        state.generation = state.generation.wrapping_add(1);
        state.candidate = Some(CandidateCredential {
            controller,
            credential: StoredCredential { revision, station },
        });
        state.phase = Some(WifiConfigurationTransactionPhase::Staged);
        map_commit(self.commit_state(&state, key, entropy).await, revision)
    }

    pub async fn activate(
        &mut self,
        controller: IdentityHash,
        revision: RemoteControlWifiCredentialRevision,
        key: &RemoteControlTargetSealingKey,
        entropy: &mut impl WifiConfigurationEntropy,
    ) -> WifiConfigurationCommitOutcome<F::Error, RemoteControlWifiStation> {
        let mut state = match self.transaction(controller, revision, key).await {
            Ok(state) => state,
            Err(error) => return WifiConfigurationCommitOutcome::NotCommitted(error),
        };
        if state.phase == Some(WifiConfigurationTransactionPhase::AwaitingConfirmation) {
            let Some(candidate) = state.candidate else {
                return WifiConfigurationCommitOutcome::NotCommitted(
                    WifiConfigurationStoreError::CorruptRecord,
                );
            };
            return WifiConfigurationCommitOutcome::Committed(candidate.credential.station);
        }
        state.phase = Some(WifiConfigurationTransactionPhase::AwaitingConfirmation);
        state.generation = state.generation.wrapping_add(1);
        match self.commit_state(&state, key, entropy).await {
            WifiConfigurationCommitOutcome::Committed(()) => {
                let Some(candidate) = state.candidate else {
                    return WifiConfigurationCommitOutcome::Indeterminate(
                        WifiConfigurationStoreError::CorruptRecord,
                    );
                };
                WifiConfigurationCommitOutcome::Committed(candidate.credential.station)
            }
            WifiConfigurationCommitOutcome::NotCommitted(error) => {
                WifiConfigurationCommitOutcome::NotCommitted(error)
            }
            WifiConfigurationCommitOutcome::Indeterminate(error) => {
                WifiConfigurationCommitOutcome::Indeterminate(error)
            }
        }
    }

    pub async fn confirm(
        &mut self,
        controller: IdentityHash,
        revision: RemoteControlWifiCredentialRevision,
        key: &RemoteControlTargetSealingKey,
        entropy: &mut impl WifiConfigurationEntropy,
    ) -> WifiConfigurationCommitOutcome<F::Error, ()> {
        let mut state = match self.transaction(controller, revision, key).await {
            Ok(state) => state,
            Err(error) => return WifiConfigurationCommitOutcome::NotCommitted(error),
        };
        if state.phase != Some(WifiConfigurationTransactionPhase::AwaitingConfirmation) {
            return WifiConfigurationCommitOutcome::NotCommitted(
                WifiConfigurationStoreError::NoTransaction,
            );
        }
        let Some(candidate) = state.candidate.take() else {
            return WifiConfigurationCommitOutcome::NotCommitted(
                WifiConfigurationStoreError::CorruptRecord,
            );
        };
        state.confirmed = Some(candidate.credential);
        state.phase = None;
        state.generation = state.generation.wrapping_add(1);
        self.commit_state(&state, key, entropy).await
    }

    pub async fn cancel(
        &mut self,
        controller: IdentityHash,
        revision: RemoteControlWifiCredentialRevision,
        key: &RemoteControlTargetSealingKey,
        entropy: &mut impl WifiConfigurationEntropy,
    ) -> WifiConfigurationCommitOutcome<F::Error, Option<RemoteControlWifiStation>> {
        let mut state = match self.transaction(controller, revision, key).await {
            Ok(state) => state,
            Err(error) => return WifiConfigurationCommitOutcome::NotCommitted(error),
        };
        state = state.without_candidate();
        state.generation = state.generation.wrapping_add(1);
        match self.commit_state(&state, key, entropy).await {
            WifiConfigurationCommitOutcome::Committed(()) => {
                WifiConfigurationCommitOutcome::Committed(
                    state.confirmed.map(|confirmed| confirmed.station),
                )
            }
            WifiConfigurationCommitOutcome::NotCommitted(error) => {
                WifiConfigurationCommitOutcome::NotCommitted(error)
            }
            WifiConfigurationCommitOutcome::Indeterminate(error) => {
                WifiConfigurationCommitOutcome::Indeterminate(error)
            }
        }
    }

    async fn transaction(
        &mut self,
        controller: IdentityHash,
        revision: RemoteControlWifiCredentialRevision,
        key: &RemoteControlTargetSealingKey,
    ) -> Result<StoredState, WifiConfigurationStoreError<F::Error>> {
        let Some(state) = self.read_active(key).await? else {
            return Err(WifiConfigurationStoreError::NoTransaction);
        };
        let Some(candidate) = state.candidate.as_ref() else {
            return Err(WifiConfigurationStoreError::NoTransaction);
        };
        if candidate.controller != controller {
            return Err(WifiConfigurationStoreError::ControllerMismatch);
        }
        if candidate.credential.revision != revision {
            return Err(WifiConfigurationStoreError::RevisionMismatch);
        }
        Ok(state)
    }

    async fn commit_state(
        &mut self,
        state: &StoredState,
        key: &RemoteControlTargetSealingKey,
        entropy: &mut impl WifiConfigurationEntropy,
    ) -> WifiConfigurationCommitOutcome<F::Error, ()> {
        if let Err(error) = self.validate_layout() {
            return WifiConfigurationCommitOutcome::NotCommitted(error);
        }
        let active = match self.read_slots(key).await {
            Ok(slots) => select_active_index(&slots),
            Err(error) => return WifiConfigurationCommitOutcome::NotCommitted(error),
        };
        let target = active.map_or(0, |index| 1 - index);
        let mut record = Zeroizing::new([0xFF; RECORD_LEN]);
        let mut iv = [0u8; IV_LEN];
        entropy.fill_random(&mut iv);
        if encode_record(state, key, &iv, &mut record).is_err() {
            iv.zeroize();
            return WifiConfigurationCommitOutcome::NotCommitted(
                WifiConfigurationStoreError::CorruptRecord,
            );
        }
        iv.zeroize();
        let page = self.pages[target];
        if let Err(error) = self.flash.erase(page, page + F::ERASE_SIZE as u32).await {
            return WifiConfigurationCommitOutcome::NotCommitted(
                WifiConfigurationStoreError::Flash {
                    operation: WifiConfigurationFlashOperation::Erase,
                    error,
                },
            );
        }
        if let Err(error) = self.flash.write(page, &record[..HEADER_LEN]).await {
            return WifiConfigurationCommitOutcome::NotCommitted(
                WifiConfigurationStoreError::Flash {
                    operation: WifiConfigurationFlashOperation::WriteRecord,
                    error,
                },
            );
        }
        if let Err(error) = self
            .flash
            .write(page + TOKEN_OFFSET as u32, &record[TOKEN_OFFSET..])
            .await
        {
            return WifiConfigurationCommitOutcome::NotCommitted(
                WifiConfigurationStoreError::Flash {
                    operation: WifiConfigurationFlashOperation::WriteRecord,
                    error,
                },
            );
        }
        let mut verification = Zeroizing::new([0u8; RECORD_LEN]);
        if let Err(error) = self.flash.read(page, &mut verification[..]).await {
            return WifiConfigurationCommitOutcome::NotCommitted(
                WifiConfigurationStoreError::Flash {
                    operation: WifiConfigurationFlashOperation::VerifyRecord,
                    error,
                },
            );
        }
        if verification[..HEADER_LEN] != record[..HEADER_LEN]
            || verification[TOKEN_OFFSET..] != record[TOKEN_OFFSET..]
            || verification[COMMIT_OFFSET..TOKEN_OFFSET] != [0xFF; COMMIT_LEN]
        {
            return WifiConfigurationCommitOutcome::NotCommitted(
                WifiConfigurationStoreError::VerificationFailed,
            );
        }
        if let Err(error) = self
            .flash
            .write(page + COMMIT_OFFSET as u32, &COMMIT_MARKER)
            .await
        {
            return WifiConfigurationCommitOutcome::Indeterminate(
                WifiConfigurationStoreError::Flash {
                    operation: WifiConfigurationFlashOperation::WriteCommit,
                    error,
                },
            );
        }
        if let Err(error) = self.flash.read(page, &mut verification[..]).await {
            return WifiConfigurationCommitOutcome::Indeterminate(
                WifiConfigurationStoreError::Flash {
                    operation: WifiConfigurationFlashOperation::VerifyCommit,
                    error,
                },
            );
        }
        match decode_record(&verification, key) {
            Ok(decoded) if equivalent_state(&decoded, state) => {
                WifiConfigurationCommitOutcome::Committed(())
            }
            Ok(_) | Err(_) => WifiConfigurationCommitOutcome::Indeterminate(
                WifiConfigurationStoreError::VerificationFailed,
            ),
        }
    }

    async fn read_active(
        &mut self,
        key: &RemoteControlTargetSealingKey,
    ) -> Result<Option<StoredState>, WifiConfigurationStoreError<F::Error>> {
        let [first, second] = self.read_slots(key).await?;
        if !matches!(&first, Slot::Valid(_))
            && !matches!(&second, Slot::Valid(_))
            && (matches!(&first, Slot::Invalid) || matches!(&second, Slot::Invalid))
        {
            return Err(WifiConfigurationStoreError::AuthenticationFailed);
        }
        Ok(select_active(first, second))
    }

    async fn read_slots(
        &mut self,
        key: &RemoteControlTargetSealingKey,
    ) -> Result<[Slot; 2], WifiConfigurationStoreError<F::Error>> {
        self.validate_layout()?;
        let mut first = Zeroizing::new([0u8; RECORD_LEN]);
        let mut second = Zeroizing::new([0u8; RECORD_LEN]);
        self.flash
            .read(self.pages[0], &mut first[..])
            .await
            .map_err(|error| WifiConfigurationStoreError::Flash {
                operation: WifiConfigurationFlashOperation::Read,
                error,
            })?;
        self.flash
            .read(self.pages[1], &mut second[..])
            .await
            .map_err(|error| WifiConfigurationStoreError::Flash {
                operation: WifiConfigurationFlashOperation::Read,
                error,
            })?;
        Ok([decode_slot(&first, key), decode_slot(&second, key)])
    }

    fn validate_layout(&self) -> Result<(), WifiConfigurationStoreError<F::Error>> {
        if F::ERASE_SIZE == 0
            || F::WRITE_SIZE == 0
            || RECORD_LEN > F::ERASE_SIZE
            || !HEADER_LEN.is_multiple_of(F::WRITE_SIZE)
            || !COMMIT_LEN.is_multiple_of(F::WRITE_SIZE)
            || !(RECORD_LEN - TOKEN_OFFSET).is_multiple_of(F::WRITE_SIZE)
            || self.pages[0] == self.pages[1]
            || self
                .pages
                .iter()
                .any(|page| !(*page as usize).is_multiple_of(F::ERASE_SIZE))
        {
            return Err(WifiConfigurationStoreError::InvalidLayout);
        }
        Ok(())
    }
}

fn map_commit<E, T: Copy>(
    outcome: WifiConfigurationCommitOutcome<E, ()>,
    value: T,
) -> WifiConfigurationCommitOutcome<E, T> {
    match outcome {
        WifiConfigurationCommitOutcome::Committed(()) => {
            WifiConfigurationCommitOutcome::Committed(value)
        }
        WifiConfigurationCommitOutcome::NotCommitted(error) => {
            WifiConfigurationCommitOutcome::NotCommitted(error)
        }
        WifiConfigurationCommitOutcome::Indeterminate(error) => {
            WifiConfigurationCommitOutcome::Indeterminate(error)
        }
    }
}

fn status_infallible<E>(
    state: &StoredState,
) -> Result<WifiConfigurationStatus, WifiConfigurationStoreError<E>> {
    state.status().map_err(|error| match error {
        WifiConfigurationStoreError::CorruptRecord => WifiConfigurationStoreError::CorruptRecord,
        _ => WifiConfigurationStoreError::CorruptRecord,
    })
}

fn select_active(first: Slot, second: Slot) -> Option<StoredState> {
    match (first, second) {
        (Slot::Valid(first), Slot::Valid(second)) => {
            if generation_is_newer(second.generation, first.generation) {
                Some(second)
            } else {
                Some(first)
            }
        }
        (Slot::Valid(state), Slot::Erased | Slot::Invalid)
        | (Slot::Erased | Slot::Invalid, Slot::Valid(state)) => Some(state),
        (Slot::Erased | Slot::Invalid, Slot::Erased | Slot::Invalid) => None,
    }
}

fn select_active_index(slots: &[Slot; 2]) -> Option<usize> {
    match (&slots[0], &slots[1]) {
        (Slot::Valid(first), Slot::Valid(second)) => Some(
            if generation_is_newer(second.generation, first.generation) {
                1
            } else {
                0
            },
        ),
        (Slot::Valid(_), Slot::Erased | Slot::Invalid) => Some(0),
        (Slot::Erased | Slot::Invalid, Slot::Valid(_)) => Some(1),
        (Slot::Erased | Slot::Invalid, Slot::Erased | Slot::Invalid) => None,
    }
}

fn generation_is_newer(candidate: u64, current: u64) -> bool {
    candidate != current && candidate.wrapping_sub(current) < (1u64 << 63)
}

fn decode_slot(bytes: &[u8; RECORD_LEN], key: &RemoteControlTargetSealingKey) -> Slot {
    if bytes.iter().all(|byte| *byte == 0xFF) {
        return Slot::Erased;
    }
    decode_record(bytes, key).map_or(Slot::Invalid, Slot::Valid)
}

fn encode_record(
    state: &StoredState,
    key: &RemoteControlTargetSealingKey,
    iv: &[u8; IV_LEN],
    out: &mut [u8; RECORD_LEN],
) -> Result<(), ()> {
    out.fill(0xFF);
    out[..4].copy_from_slice(&MAGIC);
    out[4..6].copy_from_slice(&SCHEMA_VERSION.to_be_bytes());
    out[6..8].fill(0);
    out[8..16].copy_from_slice(&state.generation.to_be_bytes());
    let mut plain = Zeroizing::new([0u8; PLAIN_LEN]);
    encode_plain(state, &mut plain)?;
    let token_key = TokenKey::from_aes256(key.as_bytes());
    let written =
        token_seal(&token_key, iv, &plain[..], &mut out[TOKEN_OFFSET..]).map_err(|_| ())?;
    if written != TOKEN_LEN {
        return Err(());
    }
    Ok(())
}

fn decode_record(
    bytes: &[u8; RECORD_LEN],
    key: &RemoteControlTargetSealingKey,
) -> Result<StoredState, TokenOpenError> {
    if bytes[..4] != MAGIC
        || bytes[4..6] != SCHEMA_VERSION.to_be_bytes()
        || bytes[6..8] != [0, 0]
        || bytes[COMMIT_OFFSET..TOKEN_OFFSET] != COMMIT_MARKER
    {
        return Err(TokenOpenError::Malformed);
    }
    let generation = u64::from_be_bytes(
        bytes[8..16]
            .try_into()
            .map_err(|_| TokenOpenError::Malformed)?,
    );
    let token_key = TokenKey::from_aes256(key.as_bytes());
    let mut plain = Zeroizing::new([0u8; PLAIN_LEN + 16]);
    let plain_len = token_open(&token_key, &bytes[TOKEN_OFFSET..], &mut plain[..])?;
    if plain_len != PLAIN_LEN {
        return Err(TokenOpenError::Malformed);
    }
    decode_plain(generation, &plain[..PLAIN_LEN]).ok_or(TokenOpenError::Malformed)
}

fn encode_plain(state: &StoredState, out: &mut [u8; PLAIN_LEN]) -> Result<(), ()> {
    out.fill(0);
    out[0] = match (&state.candidate, state.phase, &state.confirmed) {
        (None, None, None) => STATE_FACTORY,
        (None, None, Some(_)) => STATE_CONFIRMED,
        (Some(_), Some(WifiConfigurationTransactionPhase::Staged), _) => STATE_STAGED,
        (Some(_), Some(WifiConfigurationTransactionPhase::AwaitingConfirmation), _) => {
            STATE_AWAITING_CONFIRMATION
        }
        _ => return Err(()),
    };
    out[1..9].copy_from_slice(&state.generation.to_be_bytes());
    out[9..13].copy_from_slice(&state.next_revision.to_be_bytes());
    let mut offset = 13;
    if let Some(confirmed) = &state.confirmed {
        out[offset] = 1;
        write_credential(
            confirmed,
            &mut out[offset + 1..offset + 1 + REVISION_LEN + CREDENTIAL_LEN],
        )?;
    }
    offset += 1 + REVISION_LEN + CREDENTIAL_LEN;
    if let Some(candidate) = &state.candidate {
        out[offset] = 1;
        offset += 1;
        out[offset..offset + REVISION_LEN]
            .copy_from_slice(&candidate.credential.revision.get().to_be_bytes());
        offset += REVISION_LEN;
        out[offset..offset + CONTROLLER_LEN].copy_from_slice(candidate.controller.as_bytes());
        offset += CONTROLLER_LEN;
        write_station(
            &candidate.credential.station,
            &mut out[offset..offset + CREDENTIAL_LEN],
        )?;
    }
    Ok(())
}

fn decode_plain(generation: u64, plain: &[u8]) -> Option<StoredState> {
    if plain.len() != PLAIN_LEN || u64::from_be_bytes(plain[1..9].try_into().ok()?) != generation {
        return None;
    }
    let state_tag = plain[0];
    let next_revision = u32::from_be_bytes(plain[9..13].try_into().ok()?);
    if next_revision == 0 {
        return None;
    }
    let mut offset = 13;
    let confirmed_present = *plain.get(offset)?;
    offset += 1;
    let confirmed_region = plain.get(offset..offset + REVISION_LEN + CREDENTIAL_LEN)?;
    let confirmed = match confirmed_present {
        0 if confirmed_region.iter().all(|byte| *byte == 0) => None,
        1 => Some(read_credential(confirmed_region)?),
        _ => return None,
    };
    offset += REVISION_LEN + CREDENTIAL_LEN;
    let candidate_present = *plain.get(offset)?;
    offset += 1;
    let candidate_region =
        plain.get(offset..offset + REVISION_LEN + CONTROLLER_LEN + CREDENTIAL_LEN)?;
    let candidate = match candidate_present {
        0 if candidate_region.iter().all(|byte| *byte == 0) => None,
        1 => {
            let revision = RemoteControlWifiCredentialRevision::new(u32::from_be_bytes(
                candidate_region[..REVISION_LEN].try_into().ok()?,
            ))?;
            let controller = IdentityHash::new(
                candidate_region[REVISION_LEN..REVISION_LEN + CONTROLLER_LEN]
                    .try_into()
                    .ok()?,
            );
            let station = read_station(&candidate_region[REVISION_LEN + CONTROLLER_LEN..])?;
            Some(CandidateCredential {
                controller,
                credential: StoredCredential { revision, station },
            })
        }
        _ => return None,
    };
    let phase = match state_tag {
        STATE_FACTORY if confirmed.is_none() && candidate.is_none() => None,
        STATE_CONFIRMED if confirmed.is_some() && candidate.is_none() => None,
        STATE_STAGED if candidate.is_some() => Some(WifiConfigurationTransactionPhase::Staged),
        STATE_AWAITING_CONFIRMATION if candidate.is_some() => {
            Some(WifiConfigurationTransactionPhase::AwaitingConfirmation)
        }
        _ => return None,
    };
    Some(StoredState {
        generation,
        next_revision,
        confirmed,
        candidate,
        phase,
    })
}

fn write_credential(credential: &StoredCredential, out: &mut [u8]) -> Result<(), ()> {
    if out.len() != REVISION_LEN + CREDENTIAL_LEN {
        return Err(());
    }
    out[..REVISION_LEN].copy_from_slice(&credential.revision.get().to_be_bytes());
    write_station(&credential.station, &mut out[REVISION_LEN..])
}

fn read_credential(bytes: &[u8]) -> Option<StoredCredential> {
    if bytes.len() != REVISION_LEN + CREDENTIAL_LEN {
        return None;
    }
    let revision = RemoteControlWifiCredentialRevision::new(u32::from_be_bytes(
        bytes[..REVISION_LEN].try_into().ok()?,
    ))?;
    let station = read_station(&bytes[REVISION_LEN..])?;
    Some(StoredCredential { revision, station })
}

fn write_station(station: &RemoteControlWifiStation, out: &mut [u8]) -> Result<(), ()> {
    if out.len() != CREDENTIAL_LEN {
        return Err(());
    }
    out.fill(0);
    let ssid = station.ssid_bytes();
    let password = station.password_bytes();
    out[0] = u8::try_from(ssid.len()).map_err(|_| ())?;
    out[1..1 + ssid.len()].copy_from_slice(ssid);
    let password_len_offset = 1 + REMOTE_CONTROL_WIFI_SSID_CAP;
    out[password_len_offset] = u8::try_from(password.len()).map_err(|_| ())?;
    out[password_len_offset + 1..password_len_offset + 1 + password.len()]
        .copy_from_slice(password);
    Ok(())
}

fn read_station(bytes: &[u8]) -> Option<RemoteControlWifiStation> {
    if bytes.len() != CREDENTIAL_LEN {
        return None;
    }
    let ssid_len = usize::from(bytes[0]);
    let password_len_offset = 1 + REMOTE_CONTROL_WIFI_SSID_CAP;
    let password_len = usize::from(*bytes.get(password_len_offset)?);
    if ssid_len == 0
        || ssid_len > REMOTE_CONTROL_WIFI_SSID_CAP
        || password_len > REMOTE_CONTROL_WIFI_PASSWORD_CAP
        || bytes[1 + ssid_len..password_len_offset]
            .iter()
            .any(|byte| *byte != 0)
        || bytes[password_len_offset + 1 + password_len..]
            .iter()
            .any(|byte| *byte != 0)
    {
        return None;
    }
    let ssid = core::str::from_utf8(&bytes[1..1 + ssid_len]).ok()?;
    let password = core::str::from_utf8(
        &bytes[password_len_offset + 1..password_len_offset + 1 + password_len],
    )
    .ok()?;
    RemoteControlWifiStation::parse(ssid, password).ok()
}

fn equivalent_state(left: &StoredState, right: &StoredState) -> bool {
    left.generation == right.generation
        && left.next_revision == right.next_revision
        && equivalent_credential(left.confirmed.as_ref(), right.confirmed.as_ref())
        && equivalent_candidate(left.candidate.as_ref(), right.candidate.as_ref())
        && left.phase == right.phase
}

fn equivalent_credential(
    left: Option<&StoredCredential>,
    right: Option<&StoredCredential>,
) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => {
            left.revision == right.revision && left.station == right.station
        }
        (None, None) => true,
        (Some(_), None) | (None, Some(_)) => false,
    }
}

fn equivalent_candidate(
    left: Option<&CandidateCredential>,
    right: Option<&CandidateCredential>,
) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => {
            left.controller == right.controller
                && equivalent_credential(Some(&left.credential), Some(&right.credential))
        }
        (None, None) => true,
        (Some(_), None) | (None, Some(_)) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::future::Future;
    use core::task::{Context, Poll, Waker};
    use embedded_storage::nor_flash::{ErrorType, NorFlashError, NorFlashErrorKind};
    use embedded_storage_async::nor_flash::ReadNorFlash;
    use personal_rns::identity::vault::IdentitySecretKey;
    use personal_rns::identity::IDENTITY_SECRET_KEY_LEN;
    use personal_rns::remote_control::{
        RemoteControlControllerIdentitySecret, RemoteControlNodeIdentitySecrets,
        RemoteControlTargetIdentitySecret,
    };
    use std::boxed::Box;

    const PAGE_BYTES: usize = 4096;
    const CAPACITY: usize = PAGE_BYTES * 2;
    const PAGES: [u32; 2] = [0, PAGE_BYTES as u32];

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum FakeError {
        Bounds,
        PowerCut,
    }

    impl NorFlashError for FakeError {
        fn kind(&self) -> NorFlashErrorKind {
            match self {
                Self::Bounds => NorFlashErrorKind::OutOfBounds,
                Self::PowerCut => NorFlashErrorKind::Other,
            }
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Fault {
        Read(usize),
        Write(usize),
        PartialWrite(usize, usize),
        Erase(usize),
    }

    struct FakeFlash {
        bytes: Box<[u8; CAPACITY]>,
        faults: std::vec::Vec<Fault>,
        reads: usize,
        writes: usize,
        erases: usize,
    }

    impl FakeFlash {
        fn erased() -> Self {
            Self {
                bytes: Box::new([0xFF; CAPACITY]),
                faults: std::vec::Vec::new(),
                reads: 0,
                writes: 0,
                erases: 0,
            }
        }

        fn fault(mut self, fault: Fault) -> Self {
            self.faults.push(fault);
            self
        }

        fn fault_from_now(&mut self, fault: Fault) {
            let fault = match fault {
                Fault::Read(offset) => Fault::Read(self.reads + offset),
                Fault::Write(offset) => Fault::Write(self.writes + offset),
                Fault::PartialWrite(offset, written) => {
                    Fault::PartialWrite(self.writes + offset, written)
                }
                Fault::Erase(offset) => Fault::Erase(self.erases + offset),
            };
            self.faults.push(fault);
        }

        fn take_fault(&mut self, expected: Fault) -> bool {
            self.faults
                .iter()
                .position(|fault| *fault == expected)
                .map(|index| self.faults.remove(index))
                .is_some()
        }

        fn write_bytes(&mut self, offset: u32, bytes: &[u8]) -> Result<(), FakeError> {
            let target = self
                .bytes
                .get_mut(offset as usize..offset as usize + bytes.len())
                .ok_or(FakeError::Bounds)?;
            for (stored, requested) in target.iter_mut().zip(bytes) {
                if (*stored & *requested) != *requested {
                    return Err(FakeError::Bounds);
                }
                *stored &= *requested;
            }
            Ok(())
        }
    }

    impl ErrorType for FakeFlash {
        type Error = FakeError;
    }

    impl ReadNorFlash for FakeFlash {
        const READ_SIZE: usize = 1;

        async fn read(&mut self, offset: u32, bytes: &mut [u8]) -> Result<(), Self::Error> {
            let call = self.reads;
            self.reads += 1;
            if self.take_fault(Fault::Read(call)) {
                return Err(FakeError::PowerCut);
            }
            bytes.copy_from_slice(
                self.bytes
                    .get(offset as usize..offset as usize + bytes.len())
                    .ok_or(FakeError::Bounds)?,
            );
            Ok(())
        }

        fn capacity(&self) -> usize {
            CAPACITY
        }
    }

    impl NorFlash for FakeFlash {
        const WRITE_SIZE: usize = 4;
        const ERASE_SIZE: usize = PAGE_BYTES;

        async fn write(&mut self, offset: u32, bytes: &[u8]) -> Result<(), Self::Error> {
            let call = self.writes;
            self.writes += 1;
            if self.take_fault(Fault::Write(call)) {
                return Err(FakeError::PowerCut);
            }
            if let Some(index) = self.faults.iter().position(
                |fault| matches!(fault, Fault::PartialWrite(candidate, _) if *candidate == call),
            ) {
                let Fault::PartialWrite(_, written) = self.faults.remove(index) else {
                    unreachable!()
                };
                self.write_bytes(offset, &bytes[..written.min(bytes.len())])?;
                return Err(FakeError::PowerCut);
            }
            self.write_bytes(offset, bytes)
        }

        async fn erase(&mut self, from: u32, to: u32) -> Result<(), Self::Error> {
            let call = self.erases;
            self.erases += 1;
            if self.take_fault(Fault::Erase(call)) {
                return Err(FakeError::PowerCut);
            }
            self.bytes
                .get_mut(from as usize..to as usize)
                .ok_or(FakeError::Bounds)?
                .fill(0xFF);
            Ok(())
        }
    }

    fn block_on<F: Future>(future: F) -> F::Output {
        let mut future = Box::pin(future);
        let mut context = Context::from_waker(Waker::noop());
        loop {
            match future.as_mut().poll(&mut context) {
                Poll::Ready(value) => return value,
                Poll::Pending => std::thread::yield_now(),
            }
        }
    }

    fn sealing_key(fill: u8) -> RemoteControlTargetSealingKey {
        let controller = RemoteControlControllerIdentitySecret::from(IdentitySecretKey::new(
            [fill; IDENTITY_SECRET_KEY_LEN],
        ));
        let target = RemoteControlTargetIdentitySecret::from(IdentitySecretKey::new(
            [fill.wrapping_add(1); IDENTITY_SECRET_KEY_LEN],
        ));
        RemoteControlNodeIdentitySecrets::new(controller, target)
            .unwrap()
            .target_sealing_key(WIFI_CONFIGURATION_SEALING_DOMAIN)
    }

    fn entropy(fill: u8) -> RuntimeEntropy<impl EntropySource<Error = ()>> {
        RuntimeEntropy::try_new(move |output: &mut [u8]| {
            output.fill(fill);
            Ok::<(), ()>(())
        })
        .unwrap()
    }

    fn station(ssid: &str, password: &str) -> RemoteControlWifiStation {
        RemoteControlWifiStation::parse(ssid, password).unwrap()
    }

    fn committed<T>(outcome: WifiConfigurationCommitOutcome<FakeError, T>) -> T {
        match outcome {
            WifiConfigurationCommitOutcome::Committed(value) => value,
            WifiConfigurationCommitOutcome::NotCommitted(error) => {
                panic!("commit was not made: {error:?}")
            }
            WifiConfigurationCommitOutcome::Indeterminate(error) => {
                panic!("commit was indeterminate: {error:?}")
            }
        }
    }

    fn confirm_station(
        store: &mut WifiConfigurationStore<FakeFlash>,
        key: &RemoteControlTargetSealingKey,
        entropy: &mut impl WifiConfigurationEntropy,
        controller: IdentityHash,
        ssid: &str,
        password: &str,
    ) -> RemoteControlWifiCredentialRevision {
        let revision = committed(block_on(store.stage(
            controller,
            station(ssid, password),
            key,
            entropy,
        )));
        let _active = committed(block_on(store.activate(controller, revision, key, entropy)));
        committed(block_on(store.confirm(controller, revision, key, entropy)));
        revision
    }

    const COMMIT_BOUNDARY_FAULTS: [Fault; 13] = [
        Fault::Read(0),
        Fault::Read(1),
        Fault::Read(2),
        Fault::Read(3),
        Fault::Erase(0),
        Fault::Write(0),
        Fault::PartialWrite(0, 8),
        Fault::Write(1),
        Fault::PartialWrite(1, 64),
        Fault::Read(4),
        Fault::Write(2),
        Fault::PartialWrite(2, COMMIT_LEN),
        Fault::Read(5),
    ];

    #[test]
    fn staged_activation_and_confirmation_round_trip_without_exposing_passwords() {
        let key = sealing_key(0x21);
        let mut entropy = entropy(0x31);
        let controller = IdentityHash::new([0x41; CONTROLLER_LEN]);
        let mut store = WifiConfigurationStore::new(FakeFlash::erased(), PAGES);

        let initial = block_on(store.load(&key, &mut entropy)).unwrap();
        assert!(initial.active.is_none());
        assert_eq!(initial.status, WifiConfigurationStatus::FactoryProvisioning);

        let revision = committed(block_on(store.stage(
            controller,
            station("new-network", "sensitive-password"),
            &key,
            &mut entropy,
        )));
        assert_eq!(revision.get(), 1);
        assert_eq!(
            block_on(store.status(controller, &key)).unwrap(),
            WifiConfigurationStatus::Transaction {
                revision,
                controller,
                phase: WifiConfigurationTransactionPhase::Staged,
            }
        );
        let activated = committed(block_on(store.activate(
            controller,
            revision,
            &key,
            &mut entropy,
        )));
        assert_eq!(activated.ssid(), "new-network");
        assert_eq!(activated.password(), "sensitive-password");
        assert!(!std::format!("{activated:?}").contains("sensitive-password"));
        committed(block_on(store.confirm(
            controller,
            revision,
            &key,
            &mut entropy,
        )));

        let flash = store.into_flash();
        assert!(!flash
            .bytes
            .windows(b"sensitive-password".len())
            .any(|window| window == b"sensitive-password"));
        let mut rebooted = WifiConfigurationStore::new(flash, PAGES);
        let loaded = block_on(rebooted.load(&key, &mut entropy)).unwrap();
        assert_eq!(loaded.active.as_ref().unwrap().ssid(), "new-network");
        assert_eq!(
            loaded.status,
            WifiConfigurationStatus::Confirmed { revision }
        );
        assert!(!loaded.recovered_unconfirmed_transaction);
    }

    #[test]
    fn reboot_before_confirmation_discards_the_candidate_and_restores_confirmed_credentials() {
        let key = sealing_key(0x22);
        let mut entropy = entropy(0x32);
        let controller = IdentityHash::new([0x42; CONTROLLER_LEN]);
        let mut store = WifiConfigurationStore::new(FakeFlash::erased(), PAGES);

        let first = committed(block_on(store.stage(
            controller,
            station("known-good", "first-password"),
            &key,
            &mut entropy,
        )));
        let _ = committed(block_on(store.activate(
            controller,
            first,
            &key,
            &mut entropy,
        )));
        committed(block_on(store.confirm(
            controller,
            first,
            &key,
            &mut entropy,
        )));
        let second = committed(block_on(store.stage(
            controller,
            station("bad-candidate", "second-password"),
            &key,
            &mut entropy,
        )));
        let _ = committed(block_on(store.activate(
            controller,
            second,
            &key,
            &mut entropy,
        )));

        let mut rebooted = WifiConfigurationStore::new(store.into_flash(), PAGES);
        let loaded = block_on(rebooted.load(&key, &mut entropy)).unwrap();
        assert_eq!(loaded.active.as_ref().unwrap().ssid(), "known-good");
        assert!(loaded.recovered_unconfirmed_transaction);
        assert_eq!(
            loaded.status,
            WifiConfigurationStatus::Confirmed { revision: first }
        );
    }

    #[test]
    fn transactions_are_controller_bound_and_competing_stages_are_busy() {
        let key = sealing_key(0x23);
        let mut entropy = entropy(0x33);
        let owner = IdentityHash::new([0x43; CONTROLLER_LEN]);
        let stranger = IdentityHash::new([0x44; CONTROLLER_LEN]);
        let mut store = WifiConfigurationStore::new(FakeFlash::erased(), PAGES);
        let revision = committed(block_on(store.stage(
            owner,
            station("owned", "password"),
            &key,
            &mut entropy,
        )));

        assert_eq!(
            block_on(store.status(stranger, &key)),
            Err(WifiConfigurationStoreError::ControllerMismatch)
        );
        assert!(matches!(
            block_on(store.stage(stranger, station("other", "password"), &key, &mut entropy,)),
            WifiConfigurationCommitOutcome::NotCommitted(WifiConfigurationStoreError::Busy)
        ));
        assert!(matches!(
            block_on(store.activate(stranger, revision, &key, &mut entropy)),
            WifiConfigurationCommitOutcome::NotCommitted(
                WifiConfigurationStoreError::ControllerMismatch
            )
        ));
    }

    #[test]
    fn the_wrong_identity_key_cannot_open_committed_credentials() {
        let key = sealing_key(0x24);
        let wrong_key = sealing_key(0x25);
        let mut entropy = entropy(0x34);
        let controller = IdentityHash::new([0x45; CONTROLLER_LEN]);
        let mut store = WifiConfigurationStore::new(FakeFlash::erased(), PAGES);
        let _ = committed(block_on(store.stage(
            controller,
            station("sealed", "password"),
            &key,
            &mut entropy,
        )));
        let mut reopened = WifiConfigurationStore::new(store.into_flash(), PAGES);
        assert_eq!(
            block_on(reopened.status(controller, &wrong_key)),
            Err(WifiConfigurationStoreError::AuthenticationFailed)
        );
    }

    #[test]
    fn interrupted_stage_boundaries_never_replace_the_previous_committed_state() {
        let key = sealing_key(0x26);
        let controller = IdentityHash::new([0x46; CONTROLLER_LEN]);

        for fault in [
            Fault::Erase(0),
            Fault::Write(0),
            Fault::PartialWrite(0, 8),
            Fault::Write(1),
            Fault::PartialWrite(1, 64),
            Fault::Read(2),
            Fault::Write(2),
            Fault::Read(3),
        ] {
            let mut entropy = entropy(0x36);
            let flash = FakeFlash::erased().fault(fault);
            let mut store = WifiConfigurationStore::new(flash, PAGES);
            let outcome = block_on(store.stage(
                controller,
                station("candidate", "password"),
                &key,
                &mut entropy,
            ));
            assert!(!matches!(
                outcome,
                WifiConfigurationCommitOutcome::Committed(_)
            ));
            let mut rebooted = WifiConfigurationStore::new(store.into_flash(), PAGES);
            match block_on(rebooted.load(&key, &mut entropy)) {
                Ok(loaded) => assert!(loaded.active.is_none()),
                Err(WifiConfigurationStoreError::AuthenticationFailed) => {}
                Err(error) => panic!("unexpected reboot result after {fault:?}: {error:?}"),
            }
        }
    }

    #[test]
    fn interrupted_activation_boundaries_restore_the_last_confirmed_credentials() {
        let key = sealing_key(0x27);
        let controller = IdentityHash::new([0x47; CONTROLLER_LEN]);

        for fault in COMMIT_BOUNDARY_FAULTS {
            let mut entropy = entropy(0x37);
            let mut store = WifiConfigurationStore::new(FakeFlash::erased(), PAGES);
            let confirmed = confirm_station(
                &mut store,
                &key,
                &mut entropy,
                controller,
                "known-good",
                "first-password",
            );
            let candidate = committed(block_on(store.stage(
                controller,
                station("candidate", "second-password"),
                &key,
                &mut entropy,
            )));
            store.flash.fault_from_now(fault);

            assert!(!matches!(
                block_on(store.activate(controller, candidate, &key, &mut entropy)),
                WifiConfigurationCommitOutcome::Committed(_)
            ));
            let mut rebooted = WifiConfigurationStore::new(store.into_flash(), PAGES);
            let loaded = block_on(rebooted.load(&key, &mut entropy)).unwrap();
            assert_eq!(loaded.active.as_ref().unwrap().ssid(), "known-good");
            assert_eq!(
                loaded.status,
                WifiConfigurationStatus::Confirmed {
                    revision: confirmed
                }
            );
            assert!(loaded.recovered_unconfirmed_transaction, "fault={fault:?}");
        }
    }

    #[test]
    fn interrupted_confirmation_is_resolved_to_exactly_one_confirmed_revision() {
        let key = sealing_key(0x28);
        let controller = IdentityHash::new([0x48; CONTROLLER_LEN]);

        for fault in COMMIT_BOUNDARY_FAULTS {
            let mut entropy = entropy(0x38);
            let mut store = WifiConfigurationStore::new(FakeFlash::erased(), PAGES);
            let confirmed = confirm_station(
                &mut store,
                &key,
                &mut entropy,
                controller,
                "known-good",
                "first-password",
            );
            let candidate = committed(block_on(store.stage(
                controller,
                station("candidate", "second-password"),
                &key,
                &mut entropy,
            )));
            let _active = committed(block_on(store.activate(
                controller,
                candidate,
                &key,
                &mut entropy,
            )));
            store.flash.fault_from_now(fault);

            assert!(!matches!(
                block_on(store.confirm(controller, candidate, &key, &mut entropy)),
                WifiConfigurationCommitOutcome::Committed(())
            ));
            let mut rebooted = WifiConfigurationStore::new(store.into_flash(), PAGES);
            let loaded = block_on(rebooted.load(&key, &mut entropy)).unwrap();
            let commit_marker_landed = matches!(fault, Fault::PartialWrite(2, _) | Fault::Read(5));
            let (expected_revision, expected_ssid) = if commit_marker_landed {
                (candidate, "candidate")
            } else {
                (confirmed, "known-good")
            };
            assert_eq!(loaded.active.as_ref().unwrap().ssid(), expected_ssid);
            assert_eq!(
                loaded.status,
                WifiConfigurationStatus::Confirmed {
                    revision: expected_revision
                },
                "fault={fault:?}"
            );
        }
    }

    #[test]
    fn interrupted_cancellation_never_leaves_the_candidate_active_after_reboot() {
        let key = sealing_key(0x29);
        let controller = IdentityHash::new([0x49; CONTROLLER_LEN]);

        for fault in COMMIT_BOUNDARY_FAULTS {
            let mut entropy = entropy(0x39);
            let mut store = WifiConfigurationStore::new(FakeFlash::erased(), PAGES);
            let confirmed = confirm_station(
                &mut store,
                &key,
                &mut entropy,
                controller,
                "known-good",
                "first-password",
            );
            let candidate = committed(block_on(store.stage(
                controller,
                station("candidate", "second-password"),
                &key,
                &mut entropy,
            )));
            let _active = committed(block_on(store.activate(
                controller,
                candidate,
                &key,
                &mut entropy,
            )));
            store.flash.fault_from_now(fault);

            assert!(!matches!(
                block_on(store.cancel(controller, candidate, &key, &mut entropy)),
                WifiConfigurationCommitOutcome::Committed(_)
            ));
            let mut rebooted = WifiConfigurationStore::new(store.into_flash(), PAGES);
            let loaded = block_on(rebooted.load(&key, &mut entropy)).unwrap();
            assert_eq!(loaded.active.as_ref().unwrap().ssid(), "known-good");
            assert_eq!(
                loaded.status,
                WifiConfigurationStatus::Confirmed {
                    revision: confirmed
                },
                "fault={fault:?}"
            );
        }
    }

    #[test]
    fn interrupted_reboot_recovery_is_retryable_at_every_commit_boundary() {
        let key = sealing_key(0x2A);
        let controller = IdentityHash::new([0x4A; CONTROLLER_LEN]);

        for fault in COMMIT_BOUNDARY_FAULTS {
            let mut entropy = entropy(0x3A);
            let mut store = WifiConfigurationStore::new(FakeFlash::erased(), PAGES);
            let confirmed = confirm_station(
                &mut store,
                &key,
                &mut entropy,
                controller,
                "known-good",
                "first-password",
            );
            let candidate = committed(block_on(store.stage(
                controller,
                station("candidate", "second-password"),
                &key,
                &mut entropy,
            )));
            let _active = committed(block_on(store.activate(
                controller,
                candidate,
                &key,
                &mut entropy,
            )));
            store.flash.fault_from_now(fault);

            assert!(block_on(store.load(&key, &mut entropy)).is_err());
            let mut rebooted = WifiConfigurationStore::new(store.into_flash(), PAGES);
            let loaded = block_on(rebooted.load(&key, &mut entropy)).unwrap();
            assert_eq!(loaded.active.as_ref().unwrap().ssid(), "known-good");
            assert_eq!(
                loaded.status,
                WifiConfigurationStatus::Confirmed {
                    revision: confirmed
                },
                "fault={fault:?}"
            );
        }
    }
}
