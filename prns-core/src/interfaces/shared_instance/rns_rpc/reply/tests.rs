use super::*;
use crate::identity::{
    MarkDestinationUsedOutcome, ReleaseDestinationOutcome, RetainDestinationOutcome,
    RetainIdentityOutcome,
};
use crate::interfaces::shared_instance::rns_rpc::RpcVerb;
use crate::interfaces::{
    InterfaceId, InterfaceKind, PacketPhyStats, RssiDbm, SignalQualityTenthsPercent, SnrQuarterDb,
};
use crate::routing::types::NextHop;
use crate::routing::{
    BlackholeIdentityOutcome, BlackholedIdentity, DropRouteOutcome, DropRoutesViaOutcome,
    UnblackholeIdentityOutcome,
};
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

#[test]
fn stock_scalar_policy_is_owned_by_the_reply_model() {
    let stats = PacketPhyStats {
        rssi: Some(RssiDbm::new(-82)),
        snr: Some(SnrQuarterDb::new(-9)),
        quality: SignalQualityTenthsPercent::new(875),
    };
    let decode = |reply: RnsRpcReply| {
        let encoded = reply.encode(RpcDialect::Msgpack).unwrap();
        rmpv::decode::read_value(&mut std::io::Cursor::new(encoded)).unwrap()
    };

    assert_eq!(
        decode(RnsRpcReply::first_hop_timeout()),
        rmpv::Value::from(6)
    );
    assert_eq!(
        decode(RnsRpcReply::packet_rssi(Some(stats))),
        rmpv::Value::from(-82)
    );
    assert_eq!(
        decode(RnsRpcReply::packet_snr(Some(stats))),
        rmpv::Value::F64(-2.25)
    );
    assert_eq!(
        decode(RnsRpcReply::packet_quality(Some(stats))),
        rmpv::Value::F64(87.5)
    );
    assert_eq!(decode(RnsRpcReply::packet_rssi(None)), rmpv::Value::Nil);
}

#[test]
fn control_outcomes_project_to_stock_boolean_count_and_none_replies() {
    let encode = |reply: RnsRpcReply| reply.encode(RpcDialect::Msgpack);

    assert_eq!(
        encode(RnsRpcReply::drop_path(RpcOperationOutcome::Completed(
            DropRouteOutcome::Dropped,
        ))),
        Ok(vec![0xc3])
    );
    assert_eq!(
        encode(RnsRpcReply::drop_path(RpcOperationOutcome::Completed(
            DropRouteOutcome::NotFound,
        ))),
        Ok(vec![0xc2])
    );
    assert_eq!(
        encode(RnsRpcReply::drop_path(RpcOperationOutcome::Unavailable)),
        Ok(vec![0xc2])
    );
    assert_eq!(
        encode(RnsRpcReply::drop_all_via(RpcOperationOutcome::Completed(
            DropRoutesViaOutcome { dropped_routes: 3 },
        ))),
        Ok(vec![0x03])
    );
    assert_eq!(
        encode(RnsRpcReply::drop_all_via(RpcOperationOutcome::Unavailable)),
        Ok(vec![0x00])
    );
    assert_eq!(encode(RnsRpcReply::drop_announce_queues()), Ok(vec![0xc0]));
    assert_eq!(
        encode(RnsRpcReply::is_blackholed(RpcOperationOutcome::Completed(
            true
        ),)),
        Ok(vec![0xc3])
    );
    assert_eq!(
        encode(RnsRpcReply::is_blackholed(RpcOperationOutcome::Unavailable,)),
        Ok(vec![0xc2])
    );
    assert_eq!(
        encode(RnsRpcReply::blackholed_identities(
            RpcOperationOutcome::<Vec<BlackholedIdentity<String>>>::Unavailable,
        )),
        Ok(vec![0x80])
    );
    assert_eq!(
        encode(RnsRpcReply::blackhole_identity(
            RpcOperationOutcome::Completed(BlackholeIdentityOutcome::Added),
        )),
        Ok(vec![0xc3])
    );
    assert_eq!(
        encode(RnsRpcReply::blackhole_identity(
            RpcOperationOutcome::Completed(BlackholeIdentityOutcome::AlreadyPresent),
        )),
        Ok(vec![0xc0])
    );
    assert_eq!(
        encode(RnsRpcReply::blackhole_identity(
            RpcOperationOutcome::Unavailable,
        )),
        Ok(vec![0xc2])
    );
    assert_eq!(
        encode(RnsRpcReply::unblackhole_identity(
            RpcOperationOutcome::Completed(UnblackholeIdentityOutcome::Removed),
        )),
        Ok(vec![0xc3])
    );
    assert_eq!(
        encode(RnsRpcReply::unblackhole_identity(
            RpcOperationOutcome::Completed(UnblackholeIdentityOutcome::NotFound),
        )),
        Ok(vec![0xc0])
    );
    assert_eq!(
        encode(RnsRpcReply::unblackhole_identity(
            RpcOperationOutcome::Unavailable,
        )),
        Ok(vec![0xc2])
    );
}

#[test]
fn retention_outcomes_project_to_stock_success_semantics() {
    let encode = |reply: RnsRpcReply| reply.encode(RpcDialect::Msgpack);
    let retained_identity = |newly_retained_destination_count,
                             already_retained_destination_count| {
        RetainIdentityOutcome {
            newly_retained_destination_count,
            already_retained_destination_count,
        }
    };

    for outcome in [
        MarkDestinationUsedOutcome::Recorded,
        MarkDestinationUsedOutcome::Refreshed,
    ] {
        assert_eq!(
            encode(RnsRpcReply::mark_destination_used(
                RpcOperationOutcome::Completed(outcome),
            )),
            Ok(vec![0xc3])
        );
    }
    for outcome in [
        MarkDestinationUsedOutcome::Retained,
        MarkDestinationUsedOutcome::NotFound,
    ] {
        assert_eq!(
            encode(RnsRpcReply::mark_destination_used(
                RpcOperationOutcome::Completed(outcome),
            )),
            Ok(vec![0xc2])
        );
    }
    for outcome in [
        RetainDestinationOutcome::Retained,
        RetainDestinationOutcome::AlreadyRetained,
    ] {
        assert_eq!(
            encode(RnsRpcReply::retain_destination(
                RpcOperationOutcome::Completed(outcome),
            )),
            Ok(vec![0xc3])
        );
    }
    assert_eq!(
        encode(RnsRpcReply::retain_destination(
            RpcOperationOutcome::Completed(RetainDestinationOutcome::NotFound),
        )),
        Ok(vec![0xc2])
    );
    for outcome in [
        ReleaseDestinationOutcome::Released,
        ReleaseDestinationOutcome::UseRecorded,
        ReleaseDestinationOutcome::UseRefreshed,
    ] {
        assert_eq!(
            encode(RnsRpcReply::release_destination(
                RpcOperationOutcome::Completed(outcome),
            )),
            Ok(vec![0xc3])
        );
    }
    assert_eq!(
        encode(RnsRpcReply::release_destination(
            RpcOperationOutcome::Completed(ReleaseDestinationOutcome::NotFound),
        )),
        Ok(vec![0xc2])
    );
    assert_eq!(
        encode(RnsRpcReply::retain_identity(
            RpcOperationOutcome::Completed(retained_identity(1, 0)),
        )),
        Ok(vec![0xc3])
    );
    assert_eq!(
        encode(RnsRpcReply::retain_identity(
            RpcOperationOutcome::Completed(retained_identity(0, 1)),
        )),
        Ok(vec![0xc3])
    );
    assert_eq!(
        encode(RnsRpcReply::retain_identity(
            RpcOperationOutcome::Completed(retained_identity(0, 0)),
        )),
        Ok(vec![0xc2])
    );
    assert_eq!(
        encode(RnsRpcReply::retain_identity(
            RpcOperationOutcome::Unavailable,
        )),
        Ok(vec![0xc2])
    );
    assert_eq!(
        encode(RnsRpcReply::mark_destination_used(
            RpcOperationOutcome::Unavailable,
        )),
        Ok(vec![0xc2])
    );
    assert_eq!(
        encode(RnsRpcReply::retain_destination(
            RpcOperationOutcome::Unavailable,
        )),
        Ok(vec![0xc2])
    );
    assert_eq!(
        encode(RnsRpcReply::release_destination(
            RpcOperationOutcome::Unavailable,
        )),
        Ok(vec![0xc2])
    );
}

#[test]
fn legacy_reply_plans_only_request_runtime_data_when_it_exists() {
    let destination = DestinationHash::new([0x73; 16]);

    assert!(matches!(
        LegacyRpcReplyPlan::for_request(RpcVerb::GetNextHop, Some(destination)),
        LegacyRpcReplyPlan::NextHop(planned) if planned == destination
    ));
    assert!(matches!(
        LegacyRpcReplyPlan::for_request(RpcVerb::GetNextHopInterfaceName, Some(destination)),
        LegacyRpcReplyPlan::NextHopInterfaceName(planned) if planned == destination
    ));

    let LegacyRpcReplyPlan::Immediate(missing_destination) =
        LegacyRpcReplyPlan::for_request(RpcVerb::GetNextHop, None)
    else {
        panic!("a missing destination cannot become a runtime route query");
    };
    assert_eq!(
        missing_destination.encode(RpcDialect::Pickle),
        Ok(b"N.".to_vec())
    );

    let LegacyRpcReplyPlan::Immediate(timeout) =
        LegacyRpcReplyPlan::for_request(RpcVerb::GetFirstHopTimeout, None)
    else {
        panic!("the timeout reply must not need runtime data");
    };
    assert_eq!(timeout.encode(RpcDialect::Pickle), Ok(b"I6\n.".to_vec()));
}
