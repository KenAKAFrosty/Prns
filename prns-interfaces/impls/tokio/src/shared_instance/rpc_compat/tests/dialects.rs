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
