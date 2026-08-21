use core::sync::atomic::{AtomicU8, Ordering};

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use nrf_softdevice::Flash;
use personal_hopspot_core::PersistenceState;
use personal_rns::runtime::{
    EmbeddedCompactionPolicy, EmbeddedFlashPersistence, EmbeddedPersistenceDiagnostic,
    EmbeddedPersistencePolicy, FixedRouteSnapshotKeys, SharedNorFlash,
};

const PENDING: usize = 8;
const JOURNAL_WRITE_ALIGNMENT_BYTES: usize = 4;
const MAX_JOURNAL_RECORD_PADDING_BYTES: usize = JOURNAL_WRITE_ALIGNMENT_BYTES - 1;
const ROUTE_APP_DATA_BUDGETED_SEPARATELY_BYTES: usize = 0;
const COMPACTED_ROUTE_ANNOUNCE_HISTORY_DEPTH: usize = 0;
const ARENA_COMMIT_PAYLOAD_BYTES: usize = 0;

const COMPACTED_ROUTE_BASE_PAYLOAD_BYTES: usize =
    personal_rns::persistence::maximum_route_upsert_payload_len(
        ROUTE_APP_DATA_BUDGETED_SEPARATELY_BYTES,
        COMPACTED_ROUTE_ANNOUNCE_HISTORY_DEPTH,
    );
const COMPACTED_ROUTE_BASE_RECORD_BYTES: usize =
    personal_rns::persistence::flash_journal_record_storage_len(
        COMPACTED_ROUTE_BASE_PAYLOAD_BYTES,
        JOURNAL_WRITE_ALIGNMENT_BYTES,
    );
const MAX_COMPACTED_ROUTE_RECORD_BYTES: usize =
    COMPACTED_ROUTE_BASE_RECORD_BYTES + MAX_JOURNAL_RECORD_PADDING_BYTES;
const MAX_COMPACTED_ROUTES_BYTES: usize = super::Storage::TRACKED_DESTINATIONS
    * MAX_COMPACTED_ROUTE_RECORD_BYTES
    + super::Storage::RETAINED_ANNOUNCE_APP_DATA_BYTES;

const SELF_RATCHET_PAYLOAD_BYTES: usize = personal_rns::wire::TRUNCATED_HASH_BYTE_LEN
    + personal_rns::persistence::self_ratchets_snapshot_len(
        super::Storage::RETAINED_RATCHETS_PER_DESTINATION,
    );
const SELF_RATCHET_RECORD_BYTES: usize =
    personal_rns::persistence::flash_journal_record_storage_len(
        SELF_RATCHET_PAYLOAD_BYTES,
        JOURNAL_WRITE_ALIGNMENT_BYTES,
    );
const MAX_CRITICAL_FLASH_JOURNAL_BYTES: usize =
    super::Storage::UPSTREAM_APP_DESTINATIONS * SELF_RATCHET_RECORD_BYTES;

const ARENA_COMMIT_RECORD_BYTES: usize =
    personal_rns::persistence::flash_journal_record_storage_len(
        ARENA_COMMIT_PAYLOAD_BYTES,
        JOURNAL_WRITE_ALIGNMENT_BYTES,
    );
const MAX_COMPACTED_FLASH_JOURNAL_BYTES: usize =
    MAX_COMPACTED_ROUTES_BYTES + MAX_CRITICAL_FLASH_JOURNAL_BYTES + ARENA_COMMIT_RECORD_BYTES;
const _: () = assert!(
    MAX_COMPACTED_FLASH_JOURNAL_BYTES <= personal_hopspot_core::HEADLESS_NRF52840_MIN_ARENA_BYTES
);

pub type SharedFlash = SharedNorFlash<'static, CriticalSectionRawMutex, Flash>;
pub type Persistence = EmbeddedFlashPersistence<
    SharedFlash,
    FixedRouteSnapshotKeys<{ super::Storage::TRACKED_DESTINATIONS }>,
    fn(EmbeddedPersistenceDiagnostic),
    PENDING,
>;

static PERSISTENCE_STATE: AtomicU8 = AtomicU8::new(PersistenceState::Durable.encode());

pub fn new(flash: SharedFlash) -> Persistence {
    EmbeddedFlashPersistence::new(
        flash,
        personal_hopspot_core::T096_JOURNAL_LAYOUT,
        EmbeddedPersistencePolicy::hopspot_default(EmbeddedCompactionPolicy::hopspot(
            MAX_CRITICAL_FLASH_JOURNAL_BYTES,
        )),
        FixedRouteSnapshotKeys::new(),
        observe as fn(EmbeddedPersistenceDiagnostic),
    )
}

pub fn persistence_state() -> PersistenceState {
    PersistenceState::decode(PERSISTENCE_STATE.load(Ordering::Acquire))
}

fn observe(diagnostic: EmbeddedPersistenceDiagnostic) {
    match diagnostic {
        EmbeddedPersistenceDiagnostic::Restored(_) => {
            PERSISTENCE_STATE.store(PersistenceState::Durable.encode(), Ordering::Release);
        }
        EmbeddedPersistenceDiagnostic::BatchPersisted {
            state_not_saved, ..
        }
        | EmbeddedPersistenceDiagnostic::CompactionCompleted {
            state_not_saved, ..
        } => {
            let state = if state_not_saved {
                PersistenceState::Deferred
            } else {
                PersistenceState::Durable
            };
            PERSISTENCE_STATE.store(state.encode(), Ordering::Release);
        }
        EmbeddedPersistenceDiagnostic::CompactionStarted { .. } => {}
        EmbeddedPersistenceDiagnostic::DurabilityDeferred { .. } => {
            PERSISTENCE_STATE.store(PersistenceState::Deferred.encode(), Ordering::Release);
        }
        EmbeddedPersistenceDiagnostic::WriteFailed { .. } => {
            PERSISTENCE_STATE.store(PersistenceState::Failed.encode(), Ordering::Release);
        }
    }
}
