//! A/B firmware installs, independent of how the bytes arrive.
//!
//! The sender is already named by the firmware-update grant. This engine checks the transfer: a
//! SHA-256 of the image is staged first, the image then streams into the inactive slot one flash
//! sector at a time, and the boot selection moves only after that digest matches both the streamed
//! bytes and the bytes read back from flash. A mismatched image can reach the inactive slot, which
//! is inert, but cannot be selected for boot.
//!
//! Nothing here names a socket, a link or an executor. The caller hands over `&[u8]` and gets back
//! typed refusals, so the same engine serves a bench listener, an HTTP endpoint or a remote-control
//! request without changing.
//!
//! Every address comes from the board's compiled [`MemoryProfile`](personal_hopspot_memory::MemoryProfile).
//! The flashed partition table has to agree with it byte for byte or the install is refused: a
//! table that put an application slot over the identity head, the radio profile or the route
//! journal would otherwise be obeyed, not caught.

use core::cell::RefCell;

use crate::firmware_update_plan as plan;
use crate::flash::{EspRomFlash, EspRomFlashError};
use crate::memory::EspFirmwareMemory;
use alloc::boxed::Box;
use alloc::vec::Vec;
use embassy_futures::yield_now;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::blocking_mutex::Mutex as BlockingMutex;
use embedded_storage::nor_flash::{NorFlash, ReadNorFlash, RmwNorFlashStorage};
use esp_bootloader_esp_idf::ota::{Ota, OtaImageState};
use esp_bootloader_esp_idf::partitions::{self, AppPartitionSubType};
use portable_atomic::{AtomicBool, Ordering};
use sha2::{Digest, Sha256};

const FLASH_SECTOR_LEN: usize = 4096;
/// First byte of every ESP-IDF application image.
const ESP_IMAGE_MAGIC: u8 = 0xE9;
const IMAGE_MIN_LEN: usize = FLASH_SECTOR_LEN;
const SHA256_DIGEST_LEN: usize = 32;
const INSTALL_PROGRESS_LOG_BYTES: usize = 16 * 1024;
const READBACK_YIELD_SECTORS: usize = 16;
/// Core 1 heartbeats (one per second) a fresh boot accumulates before the running slot is marked
/// valid: proof the engine is alive, not just that the bootloader found an image.
pub(crate) const VALIDATE_HEARTBEATS: u64 = 30;

/// Prns declares its own flash slots at application-defined partition types, 0x40 through 0x45,
/// which the ESP-IDF partition format reserves for exactly that use. `partition_type()` in
/// esp-bootloader-esp-idf 0.5.0 reaches `unreachable!()` on any type outside 0..=3, and every
/// table helper in that crate calls it, so `find_partition` panics as soon as it steps over one of
/// ours. The raw type and subtype bytes are public, so the table is searched with those instead.
/// Everything downstream, including all boot-selection handling, is the crate's own.
const RAW_TYPE_APP: u8 = 0x00;
const RAW_TYPE_DATA: u8 = 0x01;
const RAW_SUBTYPE_DATA_OTA: u8 = 0x00;
const RAW_SUBTYPE_OTA_0: u8 = 0x10;
const RAW_SUBTYPE_OTA_1: u8 = 0x11;
/// ota_0 and ota_1. Not counting factory or test slots, which this table does not carry.
const OTA_SLOT_COUNT: usize = 2;

static STAGED_DIGEST: BlockingMutex<
    CriticalSectionRawMutex,
    RefCell<Option<[u8; SHA256_DIGEST_LEN]>>,
> = BlockingMutex::new(RefCell::new(None));
static INSTALL_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

/// Exclusive right to write the inactive slot and move the boot selection.
///
/// The transport takes it before the first byte, hands it to [`begin`], and gets it back inside
/// [`InstalledImage`] so it spans the success reply and the reset as well. Releasing it any earlier
/// would let a second upload start while the first image is still being activated.
pub(crate) struct InstallGuard;

impl InstallGuard {
    pub(crate) fn acquire() -> Result<Self, InstallError> {
        if INSTALL_IN_PROGRESS
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(InstallError::InstallInProgress);
        }
        Ok(Self)
    }
}

impl Drop for InstallGuard {
    fn drop(&mut self) {
        INSTALL_IN_PROGRESS.store(false, Ordering::Release);
    }
}

pub(crate) fn install_in_progress() -> bool {
    INSTALL_IN_PROGRESS.load(Ordering::Acquire)
}

/// Hold the SHA-256 the sender claims for the image. The install refuses to start without one.
pub(crate) fn stage_digest(digest: [u8; SHA256_DIGEST_LEN]) -> Result<(), InstallError> {
    STAGED_DIGEST.lock(|slot| *slot.borrow_mut() = Some(digest));
    Ok(())
}

fn staged_digest() -> Option<[u8; SHA256_DIGEST_LEN]> {
    STAGED_DIGEST.lock(|staged| *staged.borrow())
}

pub(crate) struct InstalledImage {
    /// The install guard, handed back rather than released. The boot selection now names a slot
    /// that has never run, and the caller still has a reply to send and a reset to perform: a
    /// second upload starting inside that window would target the slot the node is about to boot
    /// from. Keep this value alive until the reset.
    _guard: InstallGuard,
    pub(crate) slot: AppPartitionSubType,
    pub(crate) image_len: usize,
}

pub(crate) struct InstallStatus {
    pub(crate) busy: bool,
    pub(crate) selected: &'static str,
    pub(crate) state: &'static str,
    pub(crate) booted: &'static str,
    pub(crate) digest_staged: bool,
}

/// An install in flight: the target slot, the sector buffer it fills, and the digest of everything
/// handed to it. Dropping one without [`FirmwareInstall::finish`] is the abort path, and it is safe
/// by construction: the boot selection has not moved, so the bytes in the inactive slot are inert.
pub(crate) struct FirmwareInstall {
    _guard: InstallGuard,
    flash: EspRomFlash,
    flash_capacity: usize,
    boot_selection: [u32; 2],
    target: AppPartitionSubType,
    slot_offset: u32,
    slot_len: usize,
    declared_len: usize,
    received: usize,
    buffered: usize,
    sector: Vec<u8>,
    streamed: Sha256,
    expected: [u8; SHA256_DIGEST_LEN],
}

/// Locate the inactive slot and prove the flashed table is the one this firmware was compiled
/// against, before a single byte is written.
pub(crate) fn begin(
    memory: &EspFirmwareMemory,
    guard: InstallGuard,
    declared_len: usize,
) -> Result<FirmwareInstall, InstallError> {
    let Some(expected) = staged_digest() else {
        return Err(InstallError::DigestNotStaged);
    };
    if declared_len < IMAGE_MIN_LEN {
        return Err(InstallError::ImageTooSmall {
            image_len: declared_len,
        });
    }
    let Some(update_slot) = memory.update_slot() else {
        return Err(InstallError::NoUpdateSlot);
    };
    let Some(boot_selection) = memory.boot_selection() else {
        return Err(InstallError::NoBootSelection);
    };
    let firmware_owned = memory.firmware_owned();
    let flash_capacity = memory.flash_capacity();

    let mut merge = alloc::vec![0u8; FLASH_SECTOR_LEN];
    let mut storage =
        RmwNorFlashStorage::new(EspRomFlash::new(flash_capacity), merge.as_mut_slice());
    let mut scratch = Box::new([0u8; partitions::PARTITION_TABLE_MAX_LEN]);
    let table = partitions::read_partition_table(&mut storage, &mut scratch[..])
        .map_err(InstallError::Partitions)?;

    let ota_0 =
        find_raw(&table, RAW_TYPE_APP, RAW_SUBTYPE_OTA_0).ok_or(InstallError::SlotMissing {
            slot: AppPartitionSubType::Ota0,
        })?;
    let ota_1 =
        find_raw(&table, RAW_TYPE_APP, RAW_SUBTYPE_OTA_1).ok_or(InstallError::SlotMissing {
            slot: AppPartitionSubType::Ota1,
        })?;
    let ota_data = find_raw(&table, RAW_TYPE_DATA, RAW_SUBTYPE_DATA_OTA)
        .ok_or(InstallError::BootSelectionMissing)?;
    let layout = plan::Table {
        ota_0: row_of(&ota_0),
        ota_1: row_of(&ota_1),
        ota_data: row_of(&ota_data),
        firmware_owned,
        update_slot,
        boot_selection,
    };
    // Refuse a table that is not the one this firmware was compiled against before reading
    // anything else from it, so a board carrying the wrong partitions is told exactly that.
    layout.check()?;

    // Which slot the MMU is actually executing from, read independently of otadata. The bootloader
    // may have fallen back, or a migration may have left a stale selection behind, and "write the
    // other slot" is only safe if the slot we are running from is known for certain.
    let booted = table
        .booted_partition()
        .map_err(InstallError::Partitions)?
        .ok_or(InstallError::BootedSlotUnknown)?
        .offset();
    let selected = {
        let mut ota = Ota::new(ota_data.as_embedded_storage(&mut storage), OTA_SLOT_COUNT)
            .map_err(InstallError::Partitions)?;
        ota.current_app_partition()
            .map_err(InstallError::Partitions)?
    };
    let staged_plan = plan::plan(&layout, declared_len, booted, planned_slot(selected))?;
    let target = app_subtype(staged_plan.target);
    let slot_offset = staged_plan.offset;
    let slot_len = staged_plan.len;

    log::info!(
        "update: staging {declared_len} bytes into {} at 0x{slot_offset:X}",
        slot_name(target)
    );
    Ok(FirmwareInstall {
        _guard: guard,
        flash: EspRomFlash::new(flash_capacity),
        flash_capacity,
        boot_selection,
        target,
        slot_offset,
        slot_len,
        declared_len,
        received: 0,
        buffered: 0,
        sector: alloc::vec![0u8; FLASH_SECTOR_LEN],
        streamed: Sha256::new(),
        expected,
    })
}

impl FirmwareInstall {
    pub(crate) fn target_slot(&self) -> AppPartitionSubType {
        self.target
    }

    /// Take the next stretch of the image. Chunk boundaries are the caller's business; flash only
    /// ever sees whole sectors.
    pub(crate) async fn write(&mut self, chunk: &[u8]) -> Result<(), InstallError> {
        let mut chunk = chunk;
        while !chunk.is_empty() {
            let accepted = self.received + self.buffered;
            if accepted >= self.declared_len {
                return Err(InstallError::BodyOverrun {
                    declared: self.declared_len,
                });
            }
            if accepted == 0 && chunk[0] != ESP_IMAGE_MAGIC {
                return Err(InstallError::ImageMagic {
                    first_byte: chunk[0],
                });
            }
            let take = chunk
                .len()
                .min(FLASH_SECTOR_LEN - self.buffered)
                .min(self.declared_len - accepted);
            self.sector[self.buffered..self.buffered + take].copy_from_slice(&chunk[..take]);
            self.buffered += take;
            chunk = &chunk[take..];
            if self.buffered == FLASH_SECTOR_LEN {
                self.flush_sector()?;
                // The erase and write above ran to completion with the other core parked. Hand the
                // rest of the system a turn before asking for the next sector, or a network
                // transport's receive window drains and never refills and the transfer starves
                // itself.
                yield_now().await;
            }
        }
        Ok(())
    }

    /// Verify what physically landed in flash, then move the boot selection. This is the only
    /// place the selection moves, and it happens after both digests agree.
    pub(crate) async fn finish(mut self) -> Result<InstalledImage, InstallError> {
        self.flush_sector()?;
        if self.received != self.declared_len {
            return Err(InstallError::BodyTruncated {
                received: self.received,
                expected: self.declared_len,
            });
        }
        let streamed_digest: [u8; SHA256_DIGEST_LEN] =
            core::mem::take(&mut self.streamed).finalize().into();
        if streamed_digest != self.expected {
            return Err(InstallError::DigestMismatch);
        }

        // The stream digest covers what the caller handed over; this second pass covers what NOR
        // actually holds. A dropped write or a worn sector surfaces here and nowhere else.
        let mut readback = Sha256::new();
        let mut verified = 0usize;
        while verified < self.received {
            let take = FLASH_SECTOR_LEN.min(self.received - verified);
            let offset = self.slot_offset + verified as u32;
            // Whole-sector reads keep the underlying word alignment; only `take` bytes count.
            ReadNorFlash::read(&mut self.flash, offset, &mut self.sector)
                .map_err(InstallError::Flash)?;
            readback.update(&self.sector[..take]);
            verified += take;
            if verified.is_multiple_of(READBACK_YIELD_SECTORS * FLASH_SECTOR_LEN) {
                yield_now().await;
            }
        }
        let readback_digest: [u8; SHA256_DIGEST_LEN] = readback.finalize().into();
        if readback_digest != self.expected {
            return Err(InstallError::ReadbackMismatch);
        }

        select_slot(self.flash_capacity, self.boot_selection, self.target)?;
        STAGED_DIGEST.lock(|staged| staged.borrow_mut().take());
        log::info!(
            "update: installed {} bytes into {}",
            self.received,
            slot_name(self.target)
        );
        Ok(InstalledImage {
            slot: self.target,
            image_len: self.received,
            _guard: self._guard,
        })
    }

    fn flush_sector(&mut self) -> Result<(), InstallError> {
        if self.buffered == 0 {
            return Ok(());
        }
        // Erased NOR reads as 0xFF, so a partial final sector is padded rather than left holding
        // whatever the previous image put there. Nothing past `received` is ever digested.
        self.sector[self.buffered..].fill(0xFF);
        let offset = self.slot_offset + self.received as u32;
        debug_assert!(self.received + FLASH_SECTOR_LEN <= self.slot_len);
        NorFlash::erase(&mut self.flash, offset, offset + FLASH_SECTOR_LEN as u32)
            .map_err(InstallError::Flash)?;
        NorFlash::write(&mut self.flash, offset, &self.sector).map_err(InstallError::Flash)?;
        self.streamed.update(&self.sector[..self.buffered]);
        self.received += self.buffered;
        self.buffered = 0;
        if self.received.is_multiple_of(INSTALL_PROGRESS_LOG_BYTES) {
            log::info!(
                "update: {}/{} bytes into {}",
                self.received,
                self.declared_len,
                slot_name(self.target)
            );
        }
        Ok(())
    }
}

pub(crate) enum SlotHealth {
    NoOtaSlots,
    AlreadyValid,
    MarkedValid,
    SelectionRepaired {
        selected: &'static str,
        booted: &'static str,
    },
}

/// Confirm the image that is actually running, once the engine has proven itself, so a bootloader
/// built with rollback support keeps this slot. On a rollback-less bootloader the state write is
/// inert but harmless.
///
/// The slot otadata *selects* is not the slot the node necessarily *booted*. The bootloader falls
/// back to the last working image when the selected one will not start, which is the case an
/// install cannot see coming, and a wired reflash can leave a selection naming the other slot.
/// Stamping Valid on the selection would confirm an image that has never run, erase the evidence
/// of that fallback, and leave every later install refused with booted-slot-disagrees and nothing
/// to repair it. So the booted slot is the one confirmed here, and a selection naming anything
/// else is moved onto it: a bootloader fallback and an erased otadata after a migration flash are
/// then the same repair.
pub(crate) fn mark_running_slot_valid(
    memory: &EspFirmwareMemory,
) -> Result<SlotHealth, InstallError> {
    let Some(boot_selection) = memory.boot_selection() else {
        return Ok(SlotHealth::NoOtaSlots);
    };
    let flash_capacity = memory.flash_capacity();
    let mut merge = alloc::vec![0u8; FLASH_SECTOR_LEN];
    let mut storage =
        RmwNorFlashStorage::new(EspRomFlash::new(flash_capacity), merge.as_mut_slice());
    let mut scratch = Box::new([0u8; partitions::PARTITION_TABLE_MAX_LEN]);
    let Ok(table) = partitions::read_partition_table(&mut storage, &mut scratch[..]) else {
        return Ok(SlotHealth::NoOtaSlots);
    };
    let (Some(ota_data), Some(_)) = (
        find_raw(&table, RAW_TYPE_DATA, RAW_SUBTYPE_DATA_OTA),
        find_raw(&table, RAW_TYPE_APP, RAW_SUBTYPE_OTA_0),
    ) else {
        // A single-slot table is not a fault, it just has nothing to confirm.
        return Ok(SlotHealth::NoOtaSlots);
    };
    check_boot_selection(&ota_data, boot_selection)?;
    // Read from the MMU, not from otadata: this is the one question otadata cannot answer.
    let booted = booted_slot(&table).ok_or(InstallError::BootedSlotUnknown)?;
    let mut ota = Ota::new(ota_data.as_embedded_storage(&mut storage), OTA_SLOT_COUNT)
        .map_err(InstallError::Partitions)?;
    // Factory means both sequence numbers are uninitialized, which is what an erased otadata looks
    // like after a migration flash; an unreadable selection is no more authoritative than that.
    let selected = ota
        .current_app_partition()
        .unwrap_or(AppPartitionSubType::Factory);
    if selected == booted {
        return match ota.current_ota_state() {
            Ok(OtaImageState::Valid) => Ok(SlotHealth::AlreadyValid),
            Ok(_) | Err(_) => {
                ota.set_current_ota_state(OtaImageState::Valid)
                    .map_err(InstallError::Partitions)?;
                Ok(SlotHealth::MarkedValid)
            }
        };
    }
    // The selection names a slot this node is not executing. Point it at the slot that has just
    // proven itself instead: `set_current_app_partition` writes the higher sequence into the other
    // otadata entry, so the Valid stamp below lands on that same entry.
    ota.set_current_app_partition(booted)
        .map_err(InstallError::Partitions)?;
    ota.set_current_ota_state(OtaImageState::Valid)
        .map_err(InstallError::Partitions)?;
    Ok(SlotHealth::SelectionRepaired {
        selected: slot_name(selected),
        booted: slot_name(booted),
    })
}

/// The application slot the memory map is executing. This reads the partition table and the
/// booted offset only. It does not touch the boot-selection region.
pub(crate) fn booted_slot_name(memory: &EspFirmwareMemory) -> &'static str {
    let flash_capacity = memory.flash_capacity();
    let mut merge = alloc::vec![0u8; FLASH_SECTOR_LEN];
    let mut storage =
        RmwNorFlashStorage::new(EspRomFlash::new(flash_capacity), merge.as_mut_slice());
    let mut scratch = Box::new([0u8; partitions::PARTITION_TABLE_MAX_LEN]);
    let Ok(table) = partitions::read_partition_table(&mut storage, &mut scratch[..]) else {
        return "unknown";
    };
    booted_slot(&table).map_or("unknown", slot_name)
}

/// What a transport reports when someone asks before uploading anything.
pub(crate) fn status(memory: &EspFirmwareMemory) -> InstallStatus {
    let digest_staged = staged_digest().is_some();
    if install_in_progress() {
        return InstallStatus {
            busy: true,
            selected: "unknown",
            state: "unknown",
            booted: "unknown",
            digest_staged,
        };
    }
    let (selected, state, booted) =
        read_slot_status(memory).unwrap_or(("unknown", "unknown", "unknown"));
    InstallStatus {
        busy: false,
        selected,
        state,
        booted,
        digest_staged,
    }
}

fn read_slot_status(
    memory: &EspFirmwareMemory,
) -> Result<(&'static str, &'static str, &'static str), InstallError> {
    let flash_capacity = memory.flash_capacity();
    let mut merge = alloc::vec![0u8; FLASH_SECTOR_LEN];
    let mut storage =
        RmwNorFlashStorage::new(EspRomFlash::new(flash_capacity), merge.as_mut_slice());
    let mut scratch = Box::new([0u8; partitions::PARTITION_TABLE_MAX_LEN]);
    let table = partitions::read_partition_table(&mut storage, &mut scratch[..])
        .map_err(InstallError::Partitions)?;
    let booted = booted_slot(&table).map_or("unknown", slot_name);
    let ota_data = find_raw(&table, RAW_TYPE_DATA, RAW_SUBTYPE_DATA_OTA)
        .ok_or(InstallError::BootSelectionMissing)?;
    let mut ota = Ota::new(ota_data.as_embedded_storage(&mut storage), OTA_SLOT_COUNT)
        .map_err(InstallError::Partitions)?;
    let selected = slot_name(
        ota.current_app_partition()
            .map_err(InstallError::Partitions)?,
    );
    let state = ota
        .current_ota_state()
        .map(state_name)
        .unwrap_or("undefined");
    Ok((selected, state, booted))
}

fn select_slot(
    flash_capacity: usize,
    boot_selection: [u32; 2],
    slot: AppPartitionSubType,
) -> Result<(), InstallError> {
    let mut merge = alloc::vec![0u8; FLASH_SECTOR_LEN];
    let mut storage =
        RmwNorFlashStorage::new(EspRomFlash::new(flash_capacity), merge.as_mut_slice());
    let mut scratch = Box::new([0u8; partitions::PARTITION_TABLE_MAX_LEN]);
    let table = partitions::read_partition_table(&mut storage, &mut scratch[..])
        .map_err(InstallError::Partitions)?;
    let ota_data = find_raw(&table, RAW_TYPE_DATA, RAW_SUBTYPE_DATA_OTA)
        .ok_or(InstallError::BootSelectionMissing)?;
    check_boot_selection(&ota_data, boot_selection)?;
    let mut ota = Ota::new(ota_data.as_embedded_storage(&mut storage), OTA_SLOT_COUNT)
        .map_err(InstallError::Partitions)?;
    ota.set_current_app_partition(slot)
        .map_err(InstallError::Partitions)?;
    ota.set_current_ota_state(OtaImageState::New)
        .map_err(InstallError::Partitions)?;
    // Those are two separate flash writes, and power loss between them leaves the new selection
    // carrying whatever state the first write left behind. Read both back before calling the
    // install done: a selection that does not say New is one the health task would later argue
    // with, and it is better to fail the install here, with the old image still selected and
    // bootable, than to reboot into a half-written decision.
    let written_slot = ota
        .current_app_partition()
        .map_err(InstallError::Partitions)?;
    let written_state = ota.current_ota_state().map_err(InstallError::Partitions)?;
    if written_slot != slot || written_state != OtaImageState::New {
        return Err(InstallError::BootSelectionNotConfirmed);
    }
    Ok(())
}

fn find_raw<'a>(
    table: &partitions::PartitionTable<'a>,
    raw_type: u8,
    raw_subtype: u8,
) -> Option<partitions::PartitionEntry<'a>> {
    (0..table.len())
        .filter_map(|index| table.get_partition(index).ok())
        .find(|entry| entry.raw_type() == raw_type && entry.raw_subtype() == raw_subtype)
}

/// The slot the node is executing, named by matching the booted partition's offset against this
/// table's own ota rows. `None` when the running image is not one of them, which on an A/B board
/// means the flashed table and the running image disagree about where firmware lives.
fn booted_slot(table: &partitions::PartitionTable<'_>) -> Option<AppPartitionSubType> {
    let booted = table.booted_partition().ok().flatten()?.offset();
    [
        (AppPartitionSubType::Ota0, RAW_SUBTYPE_OTA_0),
        (AppPartitionSubType::Ota1, RAW_SUBTYPE_OTA_1),
    ]
    .into_iter()
    .find(|(_, raw_subtype)| {
        find_raw(table, RAW_TYPE_APP, *raw_subtype).is_some_and(|slot| slot.offset() == booted)
    })
    .map(|(slot, _)| slot)
}

fn row_of(entry: &partitions::PartitionEntry<'_>) -> plan::Row {
    plan::Row {
        offset: entry.offset(),
        len: entry.len(),
    }
}

/// The planner speaks of the two application slots only. Anything else, including the Factory an
/// erased otadata reads back as, is "no usable selection".
fn planned_slot(slot: AppPartitionSubType) -> Option<plan::Slot> {
    match slot {
        AppPartitionSubType::Ota0 => Some(plan::Slot::Ota0),
        AppPartitionSubType::Ota1 => Some(plan::Slot::Ota1),
        _ => None,
    }
}

fn app_subtype(slot: plan::Slot) -> AppPartitionSubType {
    match slot {
        plan::Slot::Ota0 => AppPartitionSubType::Ota0,
        plan::Slot::Ota1 => AppPartitionSubType::Ota1,
    }
}

impl From<plan::Refusal> for InstallError {
    fn from(refusal: plan::Refusal) -> Self {
        match refusal {
            plan::Refusal::SlotOutsideProfile {
                slot,
                offset,
                len,
                expected,
            } => Self::SlotOutsideProfile {
                slot: app_subtype(slot),
                offset,
                len,
                expected,
            },
            plan::Refusal::BootSelectionOutsideProfile {
                offset,
                len,
                expected,
            } => Self::BootSelectionOutsideProfile {
                offset,
                len,
                expected,
            },
            plan::Refusal::RunningSlotUnknown => Self::RunningSlotUnknown,
            plan::Refusal::BootedSlotDisagrees { booted, selected } => {
                Self::BootedSlotDisagrees { booted, selected }
            }
            plan::Refusal::ImageTooLarge {
                image_len,
                slot_len,
            } => Self::ImageTooLarge {
                image_len,
                slot_len,
            },
        }
    }
}

fn check_boot_selection(
    entry: &partitions::PartitionEntry<'_>,
    region: [u32; 2],
) -> Result<(), InstallError> {
    if entry.offset() == region[0] && entry.len() == region[1] - region[0] {
        return Ok(());
    }
    Err(InstallError::BootSelectionOutsideProfile {
        offset: entry.offset(),
        len: entry.len(),
        expected: region,
    })
}

pub(crate) fn slot_name(slot: AppPartitionSubType) -> &'static str {
    match slot {
        AppPartitionSubType::Factory => "factory",
        AppPartitionSubType::Ota0 => "ota_0",
        AppPartitionSubType::Ota1 => "ota_1",
        _ => "ota_n",
    }
}

fn state_name(state: OtaImageState) -> &'static str {
    match state {
        OtaImageState::New => "new",
        OtaImageState::PendingVerify => "pending-verify",
        OtaImageState::Valid => "valid",
        OtaImageState::Invalid => "invalid",
        OtaImageState::Aborted => "aborted",
        OtaImageState::Undefined => "undefined",
    }
}

#[derive(Debug)]
pub(crate) enum InstallError {
    InstallInProgress,
    NoUpdateSlot,
    NoBootSelection,
    DigestNotStaged,
    DigestMalformed,
    DigestMismatch,
    ImageTooSmall {
        image_len: usize,
    },
    ImageMagic {
        first_byte: u8,
    },
    ImageTooLarge {
        image_len: usize,
        slot_len: usize,
    },
    BodyOverrun {
        declared: usize,
    },
    BodyTruncated {
        received: usize,
        expected: usize,
    },
    SlotMissing {
        slot: AppPartitionSubType,
    },
    SlotOutsideProfile {
        slot: AppPartitionSubType,
        offset: u32,
        len: u32,
        expected: [u32; 2],
    },
    BootSelectionMissing,
    BootSelectionNotConfirmed,
    BootSelectionOutsideProfile {
        offset: u32,
        len: u32,
        expected: [u32; 2],
    },
    RunningSlotUnknown,
    BootedSlotUnknown,
    BootedSlotDisagrees {
        booted: u32,
        selected: u32,
    },
    ReadbackMismatch,
    Flash(EspRomFlashError),
    Partitions(partitions::Error),
}

impl InstallError {
    pub(crate) fn code(&self) -> &'static str {
        match self {
            Self::InstallInProgress => "install-in-progress",
            Self::NoUpdateSlot => "no-update-slot",
            Self::NoBootSelection => "no-boot-selection",
            Self::DigestNotStaged => "digest-not-staged",
            Self::DigestMalformed => "digest-malformed",
            Self::DigestMismatch => "digest-mismatch",
            Self::ImageTooSmall { .. } => "image-too-small",
            Self::ImageMagic { .. } => "image-magic",
            Self::ImageTooLarge { .. } => "image-too-large",
            Self::BodyOverrun { .. } => "body-overrun",
            Self::BodyTruncated { .. } => "body-truncated",
            Self::SlotMissing { .. } => "slot-missing",
            Self::SlotOutsideProfile { .. } => "slot-outside-profile",
            Self::BootSelectionMissing => "boot-selection-missing",
            Self::BootSelectionNotConfirmed => "boot-selection-not-confirmed",
            Self::BootSelectionOutsideProfile { .. } => "boot-selection-outside-profile",
            Self::RunningSlotUnknown => "running-slot-unknown",
            Self::BootedSlotUnknown => "booted-slot-unknown",
            Self::BootedSlotDisagrees { .. } => "booted-slot-disagrees",
            Self::ReadbackMismatch => "readback-mismatch",
            Self::Flash(_) => "flash-access",
            Self::Partitions(_) => "partition-access",
        }
    }
}

impl core::fmt::Display for InstallError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InstallInProgress => write!(formatter, "another install is already running"),
            Self::NoUpdateSlot => write!(
                formatter,
                "this build's memory profile has no firmware update slot"
            ),
            Self::NoBootSelection => write!(
                formatter,
                "this build's memory profile has no boot selection region"
            ),
            Self::DigestNotStaged => {
                write!(formatter, "stage the image SHA-256 before the image")
            }
            Self::DigestMalformed => {
                write!(formatter, "the staged digest is not a 32 byte SHA-256")
            }
            Self::DigestMismatch => {
                write!(formatter, "the image does not match the staged SHA-256")
            }
            Self::ImageTooSmall { image_len } => {
                write!(formatter, "{image_len} bytes is too small for an app image")
            }
            Self::ImageMagic { first_byte } => write!(
                formatter,
                "first byte 0x{first_byte:02X} is not an ESP application image"
            ),
            Self::ImageTooLarge {
                image_len,
                slot_len,
            } => write!(
                formatter,
                "image of {image_len} bytes exceeds the {slot_len} byte slot"
            ),
            Self::BodyOverrun { declared } => write!(
                formatter,
                "more bytes arrived than the declared {declared} byte image"
            ),
            Self::BodyTruncated { received, expected } => write!(
                formatter,
                "received {received} of the declared {expected} bytes"
            ),
            Self::SlotMissing { slot } => write!(
                formatter,
                "partition table has no {} slot; flash the A/B table first",
                slot_name(*slot)
            ),
            Self::SlotOutsideProfile {
                slot,
                offset,
                len,
                expected,
            } => write!(
                formatter,
                "{} at 0x{offset:X}+0x{len:X} is not the 0x{:X}..0x{:X} this firmware was built for",
                slot_name(*slot),
                expected[0],
                expected[1]
            ),
            Self::BootSelectionMissing => write!(
                formatter,
                "partition table has no otadata slot; flash the A/B table first"
            ),
            Self::BootSelectionNotConfirmed => write!(
                formatter,
                "the boot selection did not read back as the slot and state just written"
            ),
            Self::BootSelectionOutsideProfile {
                offset,
                len,
                expected,
            } => write!(
                formatter,
                "otadata at 0x{offset:X}+0x{len:X} is not the 0x{:X}..0x{:X} this firmware was built for",
                expected[0], expected[1]
            ),
            Self::RunningSlotUnknown => write!(
                formatter,
                "the boot selection is unreadable; the node repairs it on its own, retry in a few seconds"
            ),
            Self::BootedSlotUnknown => write!(
                formatter,
                "the running slot could not be read from the flash mapping"
            ),
            Self::BootedSlotDisagrees { booted, selected } => write!(
                formatter,
                "the node is running 0x{booted:X} but the boot selection names 0x{selected:X}"
            ),
            Self::ReadbackMismatch => {
                write!(formatter, "flash readback does not match the received image")
            }
            Self::Flash(error) => write!(formatter, "slot flash access failed: {error}"),
            Self::Partitions(error) => write!(formatter, "partition access failed: {error:?}"),
        }
    }
}
