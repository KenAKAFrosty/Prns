// LL-only DNS-SD rejects IPv4-only records. Scope fill from a local ifindex still
// keeps a mixed A+AAAA publication eligible when the AAAA lacks a reported scope.

#[test]
fn ipv4_only_tcp_record_is_rejected_without_aaaa() {
    let central_publications = central_publications();
    let android = resolved_service(
        DiscoveryTransport::Tcp,
        "prns-androidv4",
        &["192.168.1.40".parse().unwrap()],
        contract::TCP_RENDEZVOUS_PORT,
        &[(contract::TXT_VERSION_KEY, contract::TXT_VERSION_VALUE)],
    );
    assert_eq!(
        build_service_advertisement(&android, &central_publications, &[]),
        Err(ServiceAdvertisementRejection::NoEligibleEndpoints)
    );
}

#[test]
fn ipv4_interface_id_fills_missing_link_local_scope_on_aaaa() {
    let central_publications = central_publications();
    let mixed = resolved_service(
        DiscoveryTransport::Udp,
        "prns-mixed",
        &[
            "fe80::12bd:a3ff:fe9d:f90c".parse().unwrap(),
            "192.168.1.62".parse().unwrap(),
        ],
        contract::UNICAST_DISCOVERY_PORT,
        &[(contract::TXT_VERSION_KEY, contract::TXT_VERSION_VALUE)],
    );
    let advertisement = build_service_advertisement(&mixed, &central_publications, &[14])
        .expect("scoped AAAA stays eligible");
    let sockets: Vec<_> = advertisement
        .endpoints()
        .iter()
        .map(|endpoint| endpoint.socket_addr())
        .collect();
    assert_eq!(sockets.len(), 1);
    assert!(sockets.contains(&"[fe80::12bd:a3ff:fe9d:f90c%14]:29717".parse().unwrap()));
}

#[test]
fn ipv4_only_udp_record_is_rejected_without_aaaa() {
    let central_publications = central_publications();
    let android = resolved_service(
        DiscoveryTransport::Udp,
        "prns-androidudp",
        &["192.168.1.40".parse().unwrap()],
        contract::UNICAST_DISCOVERY_PORT,
        &[(contract::TXT_VERSION_KEY, contract::TXT_VERSION_VALUE)],
    );
    assert_eq!(
        build_service_advertisement(&android, &central_publications, &[]),
        Err(ServiceAdvertisementRejection::NoEligibleEndpoints)
    );
}
