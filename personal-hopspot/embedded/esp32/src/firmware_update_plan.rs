//! Where the next image is allowed to land, decided without touching flash.
//!
//! [`plan`] answers the one question an install cannot get wrong: which slot receives the bytes.
//! Getting it wrong writes over the image the node is executing. The inputs are the rows actually
//! read out of the flashed partition table and the regions the firmware was compiled against, so
//! the decision is exercised on a host with the same values the board supplies.
//!
//! Nothing here knows about `esp-bootloader-esp-idf`, a partition entry or a flash driver, which is
//! what lets it be tested at all: [`firmware_update`](crate::firmware_update) is compiled only for
//! the target.

/// One of the two application slots in the A/B table. Factory and test rows do not appear here;
/// this table has none, and a selection naming one is a refusal, not a slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Slot {
    Ota0,
    Ota1,
}

impl Slot {
    /// The slot an install targets while `self` is the one running.
    const fn other(self) -> Self {
        match self {
            Self::Ota0 => Self::Ota1,
            Self::Ota1 => Self::Ota0,
        }
    }
}

/// A partition row as the flashed table reports it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Row {
    pub(crate) offset: u32,
    pub(crate) len: u32,
}

impl Row {
    /// True when the row sits exactly on the compiled region, given as `[start, end)`.
    const fn matches(self, region: [u32; 2]) -> bool {
        self.offset == region[0] && self.len == region[1] - region[0]
    }
}

/// The flashed table's application rows beside the regions this firmware was compiled against.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Table {
    pub(crate) ota_0: Row,
    pub(crate) ota_1: Row,
    pub(crate) ota_data: Row,
    /// The compiled region ota_0 has to occupy, as `[start, end)`.
    pub(crate) firmware_owned: [u32; 2],
    /// The compiled region ota_1 has to occupy, as `[start, end)`.
    pub(crate) update_slot: [u32; 2],
    /// The compiled region otadata has to occupy, as `[start, end)`.
    pub(crate) boot_selection: [u32; 2],
}

impl Table {
    /// Prove the flashed table is the one this firmware was built against, before a single byte is
    /// written. One comparison against the compiled profile replaces a hand-kept list of regions to
    /// avoid: whatever the profile protects, from the identity head to the route journal, is
    /// protected here too, including regions added after this file was written.
    pub(crate) fn check(&self) -> Result<(), Refusal> {
        if !self.ota_0.matches(self.firmware_owned) {
            return Err(Refusal::SlotOutsideProfile {
                slot: Slot::Ota0,
                offset: self.ota_0.offset,
                len: self.ota_0.len,
                expected: self.firmware_owned,
            });
        }
        if !self.ota_1.matches(self.update_slot) {
            return Err(Refusal::SlotOutsideProfile {
                slot: Slot::Ota1,
                offset: self.ota_1.offset,
                len: self.ota_1.len,
                expected: self.update_slot,
            });
        }
        if !self.ota_data.matches(self.boot_selection) {
            return Err(Refusal::BootSelectionOutsideProfile {
                offset: self.ota_data.offset,
                len: self.ota_data.len,
                expected: self.boot_selection,
            });
        }
        Ok(())
    }

    const fn row(&self, slot: Slot) -> Row {
        match slot {
            Slot::Ota0 => self.ota_0,
            Slot::Ota1 => self.ota_1,
        }
    }
}

/// Where the image goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Plan {
    pub(crate) target: Slot,
    pub(crate) offset: u32,
    pub(crate) len: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Refusal {
    SlotOutsideProfile {
        slot: Slot,
        offset: u32,
        len: u32,
        expected: [u32; 2],
    },
    BootSelectionOutsideProfile {
        offset: u32,
        len: u32,
        expected: [u32; 2],
    },
    RunningSlotUnknown,
    BootedSlotDisagrees {
        booted: u32,
        selected: u32,
    },
    ImageTooLarge {
        image_len: usize,
        slot_len: usize,
    },
}

/// Decide which slot receives the image.
///
/// `booted_offset` is where the MMU says the running image lives, read independently of otadata.
/// `selected` is what otadata names, or `None` when it names neither application slot.
///
/// The table guard runs first, so a board carrying the wrong partitions is told so rather than
/// being sent around the selection-repair path that cannot help it.
pub(crate) fn plan(
    table: &Table,
    declared_len: usize,
    booted_offset: u32,
    selected: Option<Slot>,
) -> Result<Plan, Refusal> {
    table.check()?;

    // An erased otadata reads back as a Factory selection, and this table has no factory row, so
    // "the other slot" of Factory would fall through to ota_0: the exact slot a freshly migrated
    // board is executing from. Refuse rather than guess; the health task repairs an unreadable
    // selection within seconds and the retry then targets the right slot.
    let Some(selected) = selected else {
        return Err(Refusal::RunningSlotUnknown);
    };
    // The bootloader may have fallen back to the other image, and a wired reflash can leave a
    // selection naming a slot that never ran. "Write the other slot" is only safe when both
    // answers agree about which slot that is.
    let selected_offset = table.row(selected).offset;
    if booted_offset != selected_offset {
        return Err(Refusal::BootedSlotDisagrees {
            booted: booted_offset,
            selected: selected_offset,
        });
    }

    let target = selected.other();
    let row = table.row(target);
    let slot_len = row.len as usize;
    if declared_len > slot_len {
        return Err(Refusal::ImageTooLarge {
            image_len: declared_len,
            slot_len,
        });
    }
    Ok(Plan {
        target,
        offset: row.offset,
        len: slot_len,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shipping HELTEC_V4_R8_AB numbers, so the cases below are the board's own geometry.
    const OTA_0: [u32; 2] = [0x10000, 0x740000];
    const OTA_1: [u32; 2] = [0x740000, 0xE70000];
    const OTA_DATA: [u32; 2] = [0xE70000, 0xE72000];
    /// The image installed over the air on 2026-09-17.
    const IMAGE_LEN: usize = 2_323_824;

    fn row(region: [u32; 2]) -> Row {
        Row {
            offset: region[0],
            len: region[1] - region[0],
        }
    }

    fn table() -> Table {
        Table {
            ota_0: row(OTA_0),
            ota_1: row(OTA_1),
            ota_data: row(OTA_DATA),
            firmware_owned: OTA_0,
            update_slot: OTA_1,
            boot_selection: OTA_DATA,
        }
    }

    fn offset_of(slot: Slot) -> u32 {
        match slot {
            Slot::Ota0 => OTA_0[0],
            Slot::Ota1 => OTA_1[0],
        }
    }

    /// A node running `selected` with otadata agreeing, which is the ordinary case.
    fn healthy(selected: Slot) -> (Table, u32, Option<Slot>) {
        (table(), offset_of(selected), Some(selected))
    }

    #[test]
    fn the_install_targets_the_slot_that_is_not_running() {
        let cases = [
            (Slot::Ota0, Slot::Ota1, OTA_1),
            (Slot::Ota1, Slot::Ota0, OTA_0),
        ];
        for (selected, target, region) in cases {
            let (table, booted, selected_slot) = healthy(selected);
            assert_eq!(
                plan(&table, IMAGE_LEN, booted, selected_slot),
                Ok(Plan {
                    target,
                    offset: region[0],
                    len: (region[1] - region[0]) as usize,
                }),
                "selected={selected:?}"
            );
        }
    }

    #[test]
    fn an_unreadable_selection_is_refused_rather_than_defaulted() {
        let (table, booted, _) = healthy(Slot::Ota0);
        assert_eq!(
            plan(&table, IMAGE_LEN, booted, None),
            Err(Refusal::RunningSlotUnknown)
        );
    }

    #[test]
    fn a_selection_that_is_not_the_running_slot_is_refused() {
        // The bootloader fell back to ota_0 while otadata still names ota_1: writing "the other
        // slot" here would land on the image actually executing.
        let table = table();
        assert_eq!(
            plan(&table, IMAGE_LEN, OTA_0[0], Some(Slot::Ota1)),
            Err(Refusal::BootedSlotDisagrees {
                booted: OTA_0[0],
                selected: OTA_1[0],
            })
        );
    }

    #[test]
    fn a_single_slot_table_on_an_ab_build_is_refused() {
        // ota_0 spanning what the profile reserves for both slots: obeying it would write over the
        // route journal.
        let mut table = table();
        table.ota_0 = Row {
            offset: 0x10000,
            len: 0xE60000,
        };
        let expected = Err(Refusal::SlotOutsideProfile {
            slot: Slot::Ota0,
            offset: 0x10000,
            len: 0xE60000,
            expected: OTA_0,
        });
        assert_eq!(table.check(), expected);
        // The guard the caller runs early and the one inside `plan` are the same rule.
        assert_eq!(
            plan(&table, IMAGE_LEN, OTA_0[0], Some(Slot::Ota0)),
            expected.map(|()| unreachable!())
        );
    }

    #[test]
    fn an_update_slot_that_overruns_its_region_is_refused() {
        let mut table = table();
        table.ota_1 = Row {
            offset: 0x740000,
            len: 0x740000,
        };
        assert_eq!(
            plan(&table, IMAGE_LEN, OTA_0[0], Some(Slot::Ota0)),
            Err(Refusal::SlotOutsideProfile {
                slot: Slot::Ota1,
                offset: 0x740000,
                len: 0x740000,
                expected: OTA_1,
            })
        );
    }

    #[test]
    fn an_otadata_row_outside_the_profile_is_refused() {
        let mut table = table();
        table.ota_data = Row {
            offset: 0xE70000,
            len: 0x1000,
        };
        assert_eq!(
            plan(&table, IMAGE_LEN, OTA_0[0], Some(Slot::Ota0)),
            Err(Refusal::BootSelectionOutsideProfile {
                offset: 0xE70000,
                len: 0x1000,
                expected: OTA_DATA,
            })
        );
    }

    #[test]
    fn the_table_guard_runs_before_the_slot_is_chosen() {
        // A bad table and an unreadable selection at once. The table has to win, or a board with
        // the wrong partitions flashed would be told to retry in a few seconds, forever.
        let mut table = table();
        table.ota_0 = Row {
            offset: 0x0,
            len: 0x730000,
        };
        assert!(matches!(
            plan(&table, IMAGE_LEN, OTA_0[0], None),
            Err(Refusal::SlotOutsideProfile { .. })
        ));
    }

    #[test]
    fn an_image_larger_than_the_target_slot_is_refused() {
        let table = table();
        let slot_len = (OTA_1[1] - OTA_1[0]) as usize;
        assert_eq!(
            plan(&table, slot_len + 1, OTA_0[0], Some(Slot::Ota0)),
            Err(Refusal::ImageTooLarge {
                image_len: slot_len + 1,
                slot_len,
            })
        );
        // Exactly filling the slot is allowed.
        assert_eq!(
            plan(&table, slot_len, OTA_0[0], Some(Slot::Ota0)).map(|plan| plan.len),
            Ok(slot_len)
        );
    }

    #[test]
    fn the_two_slots_are_each_others_target() {
        assert_eq!(Slot::Ota0.other(), Slot::Ota1);
        assert_eq!(Slot::Ota1.other(), Slot::Ota0);
    }
}
