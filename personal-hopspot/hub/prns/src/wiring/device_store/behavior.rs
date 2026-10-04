use expect_test::expect;
use pipecircuit_visualization::{BehaviorError, BehaviorInventory};
#[test]
fn behavior_remains_reviewable() -> Result<(), BehaviorError> {
    expect![[r#"
        - Arbitrary valid records roundtrip and single bit corruption is detected
        - Archive limits duplicates and io failures do not silently reset storage
        - Archive roundtrip preserves unicode pairing and the versioned wire format
        - Failed write leaves enrollment pending and can be retried
        - Impossible post write refusal is an invariant error
        - Insufficient snapshot capacity never replaces an existing archive
        - Malformed archives are rejected without partial decoding
        - Persisted completion survives restart and retains other records
        - Published durability failure preserves the new file and retains the lock
        - Refusals preserve the command and do not touch storage
        - Restoration failure translation retains invariants and refused records
        - Save and restart restore records and replace renames and forgetting
        - Staged write and sync failures preserve the published archive
        - Stored records and replacements are private
        - Uncertain publication preserves pending memory and exact retry input
    "#]]
    .assert_eq(&BehaviorInventory::for_snapshot(module_path!())?.render());
    Ok(())
}
