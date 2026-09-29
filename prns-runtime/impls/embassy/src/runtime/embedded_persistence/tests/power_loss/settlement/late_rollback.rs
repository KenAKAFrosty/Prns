use super::*;

#[test]
fn late_rollback_has_no_flash_cut_window_and_reboots_to_the_committed_candidate() {
    let outcome = verify(Finalize::RollBack);
    assert_eq!(outcome.trace, Vec::<Operation>::new());
    let candidate = crate::runtime::node_facade::test_remote_control_grant(
        RemoteControlRequestKind::AnnounceSelf,
    );
    for _ in 0..2 {
        embassy_futures::block_on(super::super::grants::restore(outcome.image, &[candidate]));
    }
}
