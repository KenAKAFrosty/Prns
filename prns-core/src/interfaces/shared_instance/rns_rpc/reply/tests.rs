use super::*;
use crate::interfaces::{InterfaceId, InterfaceKind};
use crate::routing::types::NextHop;
use crate::units::InstantMillis;
use crate::wire::DestinationHash;

fn route(hops: u8) -> RouteSnapshot {
    RouteSnapshot {
        destination: DestinationHash::new([0x42; 16]),
        hops,
        via: NextHop::Direct,
        learned_at: InstantMillis(1_000),
        last_relayed_at: InstantMillis(1_500),
        expires_at: InstantMillis(2_000),
        interface: InterfaceId::from_channel_tag(InterfaceKind::TcpClient, b"route"),
    }
}

#[test]
fn scalar_replies_preserve_each_clients_dialect() {
    assert_eq!(
        RnsRpcReply::none().encode(RpcDialect::Pickle),
        Ok(b"N.".to_vec())
    );
    assert_eq!(
        RnsRpcReply::boolean(true).encode(RpcDialect::Pickle),
        Ok(b"I01\n.".to_vec())
    );
    assert_eq!(
        RnsRpcReply::integer(6).encode(RpcDialect::Pickle),
        Ok(b"I6\n.".to_vec())
    );
    assert_eq!(
        RnsRpcReply::none().encode(RpcDialect::Msgpack),
        Ok(vec![0xc0])
    );
    assert_eq!(
        RnsRpcReply::boolean(true).encode(RpcDialect::Msgpack),
        Ok(vec![0xc3])
    );
    assert_eq!(
        RnsRpcReply::integer(6).encode(RpcDialect::Msgpack),
        Ok(vec![0x06])
    );
}

#[test]
fn route_replies_apply_stock_next_hop_and_hop_filter_semantics() {
    assert_eq!(
        RnsRpcReply::next_hop(Some(route(2))).encode(RpcDialect::Msgpack),
        Ok([vec![0xc4, 0x10], vec![0x42; 16]].concat())
    );
    let maximum = RnsInteger::from_u64(2);
    let Ok(encoded) = RnsRpcReply::path_table(vec![route(1), route(2), route(3)], Some(&maximum))
        .encode(RpcDialect::Msgpack)
    else {
        panic!("path reply must encode");
    };
    let decoded = rmpv::decode::read_value(&mut std::io::Cursor::new(encoded));
    assert!(matches!(decoded, Ok(rmpv::Value::Array(rows)) if rows.len() == 2));

    let negative = RnsInteger::from_i64(-1);
    assert_eq!(
        RnsRpcReply::path_table(vec![route(1)], Some(&negative)).encode(RpcDialect::Msgpack),
        Ok(vec![0x90])
    );
    assert_eq!(
        RnsRpcReply::path_table(vec![route(1)], None).encode(RpcDialect::Pickle),
        Ok(b"].".to_vec())
    );
}
