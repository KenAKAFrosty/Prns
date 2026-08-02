use super::*;

#[test]
fn enabled_discovery_carries_stamp_trust_and_bounded_autoconnect_policy() {
    let plan = plan_of(
        "[reticulum]\n\
               network_identity = ~/.reticulum/storage/identity/network\n\
               discover_interfaces = Yes\n\
               required_discovery_value = 18\n\
               interface_discovery_sources = 00112233445566778899aabbccddeeff\n\
               autoconnect_discovered_interfaces = 3\n\
               autoconnect_interface_gravity = -9\n",
    );
    assert_eq!(
        plan.network_identity_path.as_deref(),
        Some(std::path::Path::new(
            "~/.reticulum/storage/identity/network"
        )),
    );
    let policy = plan
        .discovery
        .enabled_policy()
        .unwrap_or_else(|| panic!("discovery should be enabled"));
    assert_eq!(policy.required_stamp_cost().get(), 18);
    assert!(policy
        .sources()
        .accepts(&prns_core::identity::IdentityHash::new([
            0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
            0xee, 0xff,
        ])));
    assert!(!policy
        .sources()
        .accepts(&prns_core::identity::IdentityHash::new([0xff; 16])));
    assert_eq!(policy.auto_connect().maximum(), Some(3));
    assert_eq!(policy.auto_connect_gravity(), InterfaceGravity::new(-9));
}

#[test]
fn zero_discovery_controls_use_the_stock_stamp_and_disable_autoconnect() {
    let plan = plan_of(
            "[reticulum]\ndiscover_interfaces = Yes\nrequired_discovery_value = 0\nautoconnect_discovered_interfaces = 0\n",
        );
    let policy = plan
        .discovery
        .enabled_policy()
        .unwrap_or_else(|| panic!("discovery should be enabled"));
    assert_eq!(policy.required_stamp_cost(), DEFAULT_STAMP_COST);
    assert_eq!(policy.sources().allow_list(), None);
    assert_eq!(policy.auto_connect().maximum(), None);
    assert_eq!(policy.auto_connect_gravity(), InterfaceGravity::ZERO);
}

#[test]
fn disabled_discovery_cannot_plan_autoconnect() {
    let plan =
        plan_of("[reticulum]\ndiscover_interfaces = No\nautoconnect_discovered_interfaces = 4\n");
    assert_eq!(plan.discovery, InterfaceDiscoveryPolicy::Disabled);
}

#[test]
fn a_discoverable_listener_plans_its_announcement_and_gateway_mode() {
    let plan = plan_of(
        "[interfaces]\n\
               [[Spine]]\n\
                 type = BackboneInterface\n\
                 enabled = Yes\n\
                 listen_port = 4242\n\
                 discoverable = Yes\n\
                 announce_interval = 2\n\
                 discovery_stamp_value = 20\n\
                 discovery_name = Public Spine\n\
                 discovery_encrypt = Yes\n\
                 reachable_on = spine.example.com\n\
                 reachable_port = 18443\n\
                 publish_ifac = Yes\n\
                 latitude = 41.88\n\
                 longitude = -87.63\n\
                 height = 181.5\n",
    );
    let spine = named(&plan, "Spine");
    assert_eq!(spine.policy.mode, InterfaceMode::Gateway);
    let InterfaceDiscoveryPlan::Announce(announcement) = &spine.discovery else {
        panic!("spine should publish discovery announces");
    };
    assert_eq!(announcement.interval, DurationMillis(5 * 60 * 1_000));
    assert_eq!(announcement.stamp_cost.get(), 20);
    assert_eq!(announcement.name.as_deref(), Some("Public Spine"));
    assert_eq!(
        announcement.encryption,
        DiscoveryEncryption::NetworkIdentity
    );
    assert_eq!(announcement.ifac, DiscoveryIfacPublication::Include);
    assert_eq!(announcement.location.latitude, Some(41.88));
    assert_eq!(announcement.location.longitude, Some(-87.63));
    assert_eq!(announcement.location.height, Some(181.5));
    assert_eq!(
        announcement.advertisement,
        DiscoveryAdvertisementPlan::Backbone {
            reachable_on: "spine.example.com".to_string(),
            port: 18443,
        }
    );
}

#[test]
fn a_discoverable_rnode_defaults_to_ap_and_six_hour_announcements() {
    let plan = plan_of(
        "[interfaces]\n\
               [[Radio]]\n\
                 type = RNodeInterface\n\
                 enabled = Yes\n\
                 port = /dev/ttyUSB0\n\
                 frequency = 868000000\n\
                 bandwidth = 125000\n\
                 txpower = 7\n\
                 spreadingfactor = 8\n\
                 codingrate = 5\n\
                 discoverable = Yes\n",
    );
    let radio = named(&plan, "Radio");
    assert_eq!(radio.policy.mode, InterfaceMode::AccessPoint);
    let InterfaceDiscoveryPlan::Announce(announcement) = &radio.discovery else {
        panic!("radio should publish discovery announces");
    };
    assert_eq!(announcement.interval, DurationMillis(6 * 60 * 60 * 1_000));
    assert_eq!(announcement.stamp_cost, DEFAULT_STAMP_COST);
    assert_eq!(announcement.encryption, DiscoveryEncryption::Plaintext);
    assert_eq!(announcement.ifac, DiscoveryIfacPublication::Omit);
    assert_eq!(
        announcement.advertisement,
        DiscoveryAdvertisementPlan::RNode {
            frequency_hz: 868_000_000,
            bandwidth_hz: 125_000,
            spreading_factor: 8,
            coding_rate: 5,
        }
    );
}

#[test]
fn explicit_internal_mode_survives_discovery_on_backbone_and_rnode_interfaces() {
    let plan = plan_of(
        "[interfaces]\n\
               [[Spine]]\n\
                 type = BackboneInterface\n\
                 enabled = Yes\n\
                 listen_port = 4242\n\
                 reachable_on = spine.example.com\n\
                 discoverable = Yes\n\
                 mode = internal\n\
               [[Radio]]\n\
                 type = RNodeInterface\n\
                 enabled = Yes\n\
                 port = /dev/ttyUSB0\n\
                 frequency = 868000000\n\
                 bandwidth = 125000\n\
                 txpower = 7\n\
                 spreadingfactor = 8\n\
                 codingrate = 5\n\
                 discoverable = Yes\n\
                 mode = internal\n",
    );

    assert_eq!(named(&plan, "Spine").policy.mode, InterfaceMode::Internal);
    assert_eq!(named(&plan, "Radio").policy.mode, InterfaceMode::Internal);
}

#[test]
fn discoverable_tcp_and_kiss_plans_are_wire_complete() {
    let plan = plan_of(
        "[interfaces]\n\
               [[Public TCP]]\n\
                 type = TCPServerInterface\n\
                 enabled = Yes\n\
                 listen_ip = 0.0.0.0\n\
                 listen_port = 4242\n\
                 discoverable = Yes\n\
                 reachable_on = tcp.example.com\n\
               [[KISS Tunnel]]\n\
                 type = TCPClientInterface\n\
                 enabled = Yes\n\
                 target_host = kiss.example.com\n\
                 target_port = 8001\n\
                 kiss_framing = Yes\n\
                 discoverable = Yes\n\
                 discovery_frequency = 144800000\n\
                 discovery_bandwidth = 12500\n\
                 discovery_modulation = AFSK\n",
    );
    let InterfaceDiscoveryPlan::Announce(tcp) = &named(&plan, "Public TCP").discovery else {
        panic!("the TCP listener should publish discovery announces");
    };
    assert_eq!(
        tcp.advertisement,
        DiscoveryAdvertisementPlan::TcpServer {
            reachable_on: "tcp.example.com".to_string(),
            port: 4242,
        }
    );
    let kiss_tunnel = named(&plan, "KISS Tunnel");
    assert!(matches!(
        kiss_tunnel.medium,
        PlannedMedium::TcpClient {
            framing: TcpWireFraming::Kiss,
            ..
        }
    ));
    let InterfaceDiscoveryPlan::Announce(kiss) = &kiss_tunnel.discovery else {
        panic!("the KISS tunnel should publish discovery announces");
    };
    assert_eq!(
        kiss.advertisement,
        DiscoveryAdvertisementPlan::Kiss {
            frequency_hz: 144_800_000,
            bandwidth_hz: 12_500,
            modulation: "AFSK".to_string(),
        }
    );
}

#[test]
fn unpublishable_discovery_configuration_keeps_the_interface_and_the_reason() {
    let plan = plan_of(
        "[interfaces]\n\
               [[Private TCP]]\n\
                 type = TCPClientInterface\n\
                 enabled = Yes\n\
                 target_host = peer.example.com\n\
                 target_port = 4242\n\
                 discoverable = Yes\n\
               [[Incomplete Server]]\n\
                 type = TCPServerInterface\n\
                 enabled = Yes\n\
                 listen_ip = 0.0.0.0\n\
                 listen_port = 4243\n\
                 discoverable = Yes\n",
    );
    assert_eq!(plan.interfaces.len(), 2);
    assert_eq!(
        named(&plan, "Private TCP").discovery,
        InterfaceDiscoveryPlan::Unpublishable(DiscoveryPublicationProblem::IncompatibleSetting {
            key: interface_key::KISS_FRAMING,
        })
    );
    assert_eq!(
        named(&plan, "Incomplete Server").discovery,
        InterfaceDiscoveryPlan::Unpublishable(
            DiscoveryPublicationProblem::MissingRequiredSetting {
                key: interface_key::REACHABLE_ON,
            }
        )
    );
}
