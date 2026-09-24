use super::*;

fn message(kind: u16, index: i32, flags: u32, extra: &[u8]) -> Vec<u8> {
    let length = NETLINK_HEADER_BYTES + LINK_HEADER_BYTES + extra.len();
    let mut bytes = vec![0; length.next_multiple_of(MESSAGE_ALIGNMENT)];
    bytes[..4].copy_from_slice(&(length as u32).to_ne_bytes());
    bytes[4..6].copy_from_slice(&kind.to_ne_bytes());
    bytes[20..24].copy_from_slice(&index.to_ne_bytes());
    bytes[24..28].copy_from_slice(&flags.to_ne_bytes());
    bytes[32..length].copy_from_slice(extra);
    bytes
}

#[test]
fn down_and_delete_survive_a_batch_ending_in_up() {
    let mut bytes = message(libc::RTM_NEWLINK, 9, libc::IFF_UP as u32, &[1]);
    bytes.extend(message(libc::RTM_NEWLINK, 10, 0, &[]));
    bytes.extend(message(libc::RTM_DELLINK, 10, 0, &[]));
    bytes.extend(message(libc::RTM_NEWLINK, 11, libc::IFF_UP as u32, &[]));
    assert_eq!(
        parse(&bytes),
        LinkChanges::Observed(vec![
            LinkChange::Updated,
            LinkChange::Down { index: 10 },
            LinkChange::Deleted { index: 10 },
            LinkChange::Updated,
        ])
    );
}

#[test]
fn incomplete_or_lost_notifications_require_a_fresh_binding() {
    let valid = message(libc::RTM_NEWLINK, 10, 0, &[]);
    for length in 1..valid.len() {
        assert_eq!(parse(&valid[..length]), LinkChanges::ResyncRequired);
    }
    for length in [0u32, 15, 16, u32::MAX] {
        let mut bytes = valid.clone();
        bytes[..4].copy_from_slice(&length.to_ne_bytes());
        assert_eq!(parse(&bytes), LinkChanges::ResyncRequired);
    }
    for kind in [libc::NLMSG_ERROR as u16, libc::NLMSG_OVERRUN as u16] {
        assert_eq!(
            parse(&message(kind, 10, 0, &[])),
            LinkChanges::ResyncRequired
        );
    }
}

#[test]
fn unrelated_messages_do_not_hide_link_notifications() {
    let mut bytes = message(libc::NLMSG_NOOP as u16, 0, 0, &[]);
    bytes.extend(message(libc::RTM_DELLINK, 42, 0, &[]));
    assert_eq!(
        parse(&bytes),
        LinkChanges::Observed(vec![LinkChange::Deleted { index: 42 }])
    );
    assert_eq!(parse(&[]), LinkChanges::Observed(vec![]));
}
