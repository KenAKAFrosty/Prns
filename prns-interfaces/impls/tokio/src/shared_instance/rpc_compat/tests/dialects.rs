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
