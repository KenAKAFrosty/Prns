use super::*;

#[test]
fn telemetry_classification_uses_the_decoded_operation() {
    let bytes = msgpack_request(std::vec![
        ("blackhole_identity", Value::Binary(std::vec![5; 16])),
        ("until", Value::Nil),
        ("reason", Value::from("interface_stats next_hop")),
    ]);
    let request = RpcRequest::decode(&bytes).unwrap();
    assert!(matches!(request.verb(), RpcVerb::BlackholeIdentity));
}

#[cfg(feature = "tracing")]
#[test]
fn rpc_log_fields_are_stable() {
    assert_eq!(RpcDialect::Pickle.as_str(), "pickle");
    assert_eq!(RpcDialect::Msgpack.as_str(), "msgpack");
    assert_eq!(RpcVerb::InterfaceStats.as_str(), "interface_stats");
    assert_eq!(RpcVerb::PathTable.as_str(), "path_table");
    assert_eq!(RpcVerb::RateTable.as_str(), "rate_table");
    assert_eq!(RpcVerb::LinkCount.as_str(), "link_count");
    assert_eq!(RpcVerb::NextHop.as_str(), "next_hop");
    assert_eq!(RpcVerb::NextHopIfName.as_str(), "next_hop_if_name");
    assert_eq!(RpcVerb::FirstHopTimeout.as_str(), "first_hop_timeout");
    assert_eq!(RpcVerb::PacketRssi.as_str(), "packet_rssi");
    assert_eq!(RpcVerb::PacketSnr.as_str(), "packet_snr");
    assert_eq!(RpcVerb::PacketQuality.as_str(), "packet_q");
    assert_eq!(
        RpcVerb::BlackholedIdentities.as_str(),
        "blackholed_identities"
    );
    assert_eq!(RpcVerb::IsBlackholed.as_str(), "is_blackholed");
    assert_eq!(RpcVerb::DropPath.as_str(), "drop_path");
    assert_eq!(RpcVerb::DropAllVia.as_str(), "drop_all_via");
    assert_eq!(RpcVerb::DropAnnounceQueues.as_str(), "drop_announce_queues");
    assert_eq!(RpcVerb::BlackholeIdentity.as_str(), "blackhole_identity");
    assert_eq!(
        RpcVerb::UnblackholeIdentity.as_str(),
        "unblackhole_identity"
    );
    assert_eq!(RpcVerb::DestinationData.as_str(), "destination_data");
    assert_eq!(RpcVerb::IdentityData.as_str(), "identity_data");
    assert_eq!(RpcVerb::Unknown.as_str(), "unknown");
}

#[test]
fn msgpack_reply_codec_carries_binary_map_keys_and_floats() {
    let value = Value::Map(std::vec![(
        Value::Binary(std::vec![0x5a; 16]),
        Value::Map(std::vec![(Value::from("until"), Value::F64(123.5))]),
    )]);
    let bytes = encode_msgpack(value.clone()).unwrap();
    let decoded = rmpv::decode::read_value(&mut std::io::Cursor::new(bytes)).unwrap();
    assert_eq!(decoded, value);
}
