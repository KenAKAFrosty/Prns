use super::*;

#[test]
fn first_frame_identity_requires_only_scope_and_source_mac() {
    let instance = InstanceTag::new(b"halow-main").unwrap();
    let source = PeerMac::new(MacAddress::new([0x50, 0x37, 0xcd, 0xad, 0x32, 0x0e])).unwrap();
    assert_eq!(
        instance.peer_channel_tag(source).as_slice(),
        b"\x01\x0ahalow-main\x50\x37\xcd\xad\x32\x0e"
    );
    assert_eq!(
        instance.peer_id(source).kind(),
        Some(InterfaceKind::WifiHaLowPeer)
    );
    assert_eq!(
        instance.peer_id(source),
        InstanceTag::new(b"halow-main").unwrap().peer_id(source)
    );
    assert_ne!(
        instance.peer_id(source),
        InstanceTag::new(b"halow-other").unwrap().peer_id(source)
    );
    let changed = PeerMac::new(MacAddress::new([0x50, 0x37, 0xcd, 0xad, 0x32, 0x0f])).unwrap();
    assert_ne!(instance.peer_id(source), instance.peer_id(changed));
}

#[test]
fn rejects_group_and_unspecified_sources_but_accepts_local_addresses() {
    assert_eq!(
        PeerMac::new(MacAddress::new([0; 6])),
        Err(PeerMacError::Unspecified)
    );
    for bytes in [[0xff; 6], [1, 0, 0x5e, 0, 0, 1]] {
        assert_eq!(
            PeerMac::new(MacAddress::new(bytes)),
            Err(PeerMacError::GroupAddress)
        );
    }
    assert!(PeerMac::new(MacAddress::new([2, 0, 0, 0, 0, 1])).is_ok());
}

#[test]
fn instance_boundaries_preserve_every_byte() {
    assert_eq!(InstanceTag::new(b""), Err(InstanceTagError::Empty));
    assert_eq!(
        InstanceTag::new(&[1; INSTANCE_TAG_MAX_LEN + 1]),
        Err(InstanceTagError::TooLong)
    );
    let peer = PeerMac::new(MacAddress::new([2, 0, 0, 0, 0, 1])).unwrap();
    let tag = InstanceTag::new(&[7; INSTANCE_TAG_MAX_LEN])
        .unwrap()
        .peer_channel_tag(peer);
    assert_eq!(tag.len(), CHANNEL_TAG_MAX_LEN);
    assert_eq!(
        &tag[2..2 + INSTANCE_TAG_MAX_LEN],
        &[7; INSTANCE_TAG_MAX_LEN]
    );
    assert_eq!(&tag[2 + INSTANCE_TAG_MAX_LEN..], &peer.address().octets());
}

#[test]
fn halow_kinds_append_without_reusing_persisted_discriminants() {
    use crate::interfaces::{RadioFamily, RadioIndication, WifiIndication};
    assert_eq!(InterfaceKind::WifiHaLow.radio_family(), RadioFamily::HaLow);
    assert_eq!(
        InterfaceKind::WifiHaLowPeer.radio_family(),
        RadioFamily::HaLow
    );
    assert_eq!(
        RadioIndication::for_kind(Some(InterfaceKind::WifiHaLowPeer)),
        RadioIndication::HaLow(WifiIndication::Unavailable)
    );
    assert_eq!(InterfaceKind::from_u8(34), Some(InterfaceKind::WifiHaLow));
    assert_eq!(
        InterfaceKind::from_u8(35),
        Some(InterfaceKind::WifiHaLowPeer)
    );
    assert_eq!(
        InterfaceKind::WifiHaLow.member_kind(),
        Some(InterfaceKind::WifiHaLowPeer)
    );
    assert_eq!(
        InterfaceKind::WifiHaLowPeer.supervisor_kind(),
        Some(InterfaceKind::WifiHaLow)
    );
}

#[test]
fn shared_channel_is_a_distinct_halow_wire_in_the_same_fanout_family() {
    let instance = InstanceTag::new(b"radio").unwrap();
    let shared =
        InterfaceId::from_channel_tag(InterfaceKind::WifiHaLowBroadcast, &instance.channel_tag());
    let supervisor =
        InterfaceId::from_channel_tag(InterfaceKind::WifiHaLow, &instance.channel_tag());
    assert_ne!(shared, supervisor);
    assert_eq!(shared.kind(), InterfaceKind::from_u8(36));
    assert_eq!(
        InterfaceKind::WifiHaLowBroadcast.fanout_kind(),
        Some(InterfaceKind::WifiHaLow)
    );
    assert_eq!(
        InterfaceKind::WifiHaLowBroadcast.radio_family(),
        crate::interfaces::RadioFamily::HaLow
    );
}
