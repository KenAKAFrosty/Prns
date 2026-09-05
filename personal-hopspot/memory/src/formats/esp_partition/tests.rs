use super::*;
use crate::{ESP_16_MIB_PARTITION_TABLE, HELTEC_V4};

const HELTEC_V4_ONLY: [MemoryProfileId; 1] = [HELTEC_V4.id];
const DUPLICATE_REGIONS: [EspPartitionBinding; 2] = [
    ESP_16_MIB_PARTITION_TABLE.partitions[0],
    ESP_16_MIB_PARTITION_TABLE.partitions[0],
];
const OUT_OF_ORDER: [EspPartitionBinding; 2] = [
    ESP_16_MIB_PARTITION_TABLE.partitions[1],
    ESP_16_MIB_PARTITION_TABLE.partitions[0],
];
const WRONG_KIND: [EspPartitionBinding; 1] = [EspPartitionBinding {
    region: MemoryRegionId("firmware"),
    name: "factory",
    kind: EspPartitionKind::NvsData,
}];

#[test]
fn duplicate_region_bindings_are_rejected() {
    let table = EspPartitionTable {
        profiles: &HELTEC_V4_ONLY,
        partitions: &DUPLICATE_REGIONS,
    };
    assert!(matches!(
        table.validate(&HELTEC_V4),
        Err(EspPartitionTableError::DuplicateRegion { .. })
    ));
}

#[test]
fn partition_bindings_must_follow_flash_order() {
    let table = EspPartitionTable {
        profiles: &HELTEC_V4_ONLY,
        partitions: &OUT_OF_ORDER,
    };
    assert!(matches!(
        table.validate(&HELTEC_V4),
        Err(EspPartitionTableError::PartitionsOutOfOrder { .. })
    ));
}

#[test]
fn partition_kinds_must_match_region_roles() {
    let table = EspPartitionTable {
        profiles: &HELTEC_V4_ONLY,
        partitions: &WRONG_KIND,
    };
    assert_eq!(
        table.validate(&HELTEC_V4),
        Err(EspPartitionTableError::PartitionKindMismatch {
            region: MemoryRegionId("firmware"),
            kind: EspPartitionKind::NvsData,
        })
    );
}
