use super::*;

#[test]
fn a_listen_only_udp_disables_egress_and_remains_constructible() {
    let plan = plan_of(
        "[interfaces]\n[[Mesh]]\ntype = UDPInterface\nenabled = Yes\n\
             listen_ip = 0.0.0.0\nlisten_port = 4848\n",
    );
    let interface = named(&plan, "Mesh");
    assert_eq!(
        interface.medium,
        PlannedMedium::Udp {
            flow: UdpFlowPlan::ReceiveOnly {
                listen: udp_address("0.0.0.0", 4848),
            }
        }
    );
    assert_eq!(
        interface.policy.capabilities.egress,
        EgressCapability::Disabled
    );
}

#[test]
fn send_only_udp_disables_ingress_and_explicit_outgoing_no_still_wins() {
    let enabled = plan_of(
        "[interfaces]\n[[Mesh]]\ntype = UDPInterface\nenabled = Yes\n\
             forward_ip = 255.255.255.255\nforward_port = 4848\n",
    );
    let interface = named(&enabled, "Mesh");
    assert_eq!(
        interface.medium,
        PlannedMedium::Udp {
            flow: UdpFlowPlan::SendOnly {
                forward: udp_address("255.255.255.255", 4848),
            }
        }
    );
    assert_eq!(
        interface.policy.capabilities.ingress,
        IngressCapability::Disabled
    );
    assert!(interface.policy.capabilities.allows_transmit());

    let disabled = plan_of(
        "[interfaces]\n[[Mesh]]\ntype = UDPInterface\nenabled = Yes\noutgoing = No\n\
             forward_ip = 255.255.255.255\nforward_port = 4848\n",
    );
    assert_eq!(
        named(&disabled, "Mesh").policy.capabilities.egress,
        EgressCapability::Disabled
    );
}

#[test]
fn udp_device_and_port_form_a_bidirectional_broadcast_flow() {
    let plan = plan_of(
        "[interfaces]\n[[Mesh]]\ntype = UDPInterface\nenabled = Yes\ndevice = eth0\nport = 4848\n",
    );
    let endpoint = UdpEndpointPlan {
        host: UdpEndpointHost::DeviceBroadcast("eth0".to_string()),
        port: 4848,
    };
    assert_eq!(
        named(&plan, "Mesh").medium,
        PlannedMedium::Udp {
            flow: UdpFlowPlan::Bidirectional {
                listen: endpoint.clone(),
                forward: endpoint,
            }
        }
    );
}

#[test]
fn udp_device_supplies_the_address_without_changing_a_partial_direction() {
    let receive = plan_of(
        "[interfaces]\n[[Receive]]\ntype = UDPInterface\nenabled = Yes\ndevice = eth0\n\
             listen_port = 4848\n",
    );
    let send = plan_of(
        "[interfaces]\n[[Send]]\ntype = UDPInterface\nenabled = Yes\ndevice = eth0\n\
             forward_port = 4849\n",
    );

    assert_eq!(
        named(&receive, "Receive").medium,
        PlannedMedium::Udp {
            flow: UdpFlowPlan::ReceiveOnly {
                listen: UdpEndpointPlan {
                    host: UdpEndpointHost::DeviceBroadcast("eth0".to_string()),
                    port: 4848,
                },
            }
        }
    );
    assert_eq!(
        named(&send, "Send").medium,
        PlannedMedium::Udp {
            flow: UdpFlowPlan::SendOnly {
                forward: UdpEndpointPlan {
                    host: UdpEndpointHost::DeviceBroadcast("eth0".to_string()),
                    port: 4849,
                },
            }
        }
    );
}

#[test]
fn the_serial_baud_defaults_to_the_rns_default_when_unset() {
    let plan = plan_of(
        "[interfaces]\n[[Modem]]\ntype = SerialInterface\nenabled = Yes\nport = /dev/ttyUSB0\n",
    );
    assert_eq!(
        named(&plan, "Modem").medium,
        PlannedMedium::Serial {
            device: "/dev/ttyUSB0".to_string(),
            line: serial_line_plan(RNS_DEFAULT_SERIAL_BAUD),
        }
    );
    assert_eq!(named(&plan, "Modem").policy.bitrate.get(), 9_600);
}

#[test]
fn serial_line_settings_are_typed_and_drive_the_bitrate() {
    let plan = plan_of(
            "[interfaces]\n[[Modem]]\ntype = SerialInterface\nenabled = Yes\nport = /dev/ttyUSB0\nspeed = 57600\ndatabits = 7\nparity = even\nstopbits = 2\n",
        );
    assert_eq!(
        named(&plan, "Modem").medium,
        PlannedMedium::Serial {
            device: "/dev/ttyUSB0".to_string(),
            line: SerialLinePlan {
                baud: 57_600,
                data_bits: SerialDataBits::Seven,
                parity: SerialParity::Even,
                stop_bits: SerialStopBits::Two,
            },
        }
    );
    assert_eq!(named(&plan, "Modem").policy.bitrate.get(), 57_600);
}

#[test]
fn traversed_network_defaults_share_the_500_mbps_policy() {
    let plan = plan_of(
        "[interfaces]\n\
               [[Tcp]]\n\
                 type = TCPClientInterface\n\
                 enabled = Yes\n\
                 target_host = example.com\n\
                 target_port = 4242\n\
               [[Udp]]\n\
                 type = UDPInterface\n\
                 enabled = Yes\n\
                 listen_ip = 0.0.0.0\n\
                 listen_port = 4242\n\
                 forward_ip = 255.255.255.255\n\
                 forward_port = 4242\n\
               [[Backbone]]\n\
                 type = BackboneInterface\n\
                 enabled = Yes\n\
                 listen_port = 4243\n",
    );

    let tcp = named(&plan, "Tcp");
    assert_eq!(tcp.policy.bitrate.get(), 500_000_000);
    assert_eq!(tcp.policy.mtu.resolve(tcp.policy.bitrate), Some(131_072));
    let udp = named(&plan, "Udp");
    assert_eq!(udp.policy.bitrate.get(), 500_000_000);
    assert_eq!(
        udp.policy.mtu.resolve(udp.policy.bitrate),
        Some(prns_core::interfaces::udp::core::UDP_DATAGRAM_MAX)
    );
    let backbone = named(&plan, "Backbone");
    assert_eq!(backbone.policy.bitrate.get(), 500_000_000);
    assert_eq!(
        backbone.policy.mtu.resolve(backbone.policy.bitrate),
        Some(131_072)
    );
}

#[test]
fn auto_wifi_keeps_its_gigabit_estimate_without_overpromising_its_datagram() {
    let plan = plan_of("[interfaces]\n[[Wifi]]\ntype = AutoInterface\nenabled = Yes\n");
    let wifi = named(&plan, "Wifi");

    assert_eq!(wifi.policy.bitrate.get(), 1_000_000_000);
    assert_eq!(
        wifi.policy.mtu.resolve(wifi.policy.bitrate),
        Some(prns_core::interfaces::wifi_auto::core::HARDWARE_MTU)
    );
}

#[test]
fn configured_u64_bitrate_and_fixed_mtu_are_preserved_without_clamping() {
    let plan = plan_of(
        "[interfaces]\n\
               [[Fast]]\n\
                 type = TCPClientInterface\n\
                 enabled = Yes\n\
                 target_host = example.com\n\
                 target_port = 4242\n\
                 bitrate = 5000000000\n\
                 fixed_mtu = 4096\n",
    );
    let fast = named(&plan, "Fast");

    assert_eq!(fast.policy.bitrate.get(), 5_000_000_000);
    assert_eq!(fast.policy.mtu.resolve(fast.policy.bitrate), Some(4_096));
}

#[test]
fn lower_rate_media_own_their_effective_estimates() {
    let plan = plan_of(
        "[interfaces]\n\
               [[Serial]]\n\
                 type = SerialInterface\n\
                 enabled = Yes\n\
                 port = /dev/ttyUSB0\n\
                 speed = 115200\n\
               [[Kiss]]\n\
                 type = KISSInterface\n\
                 enabled = Yes\n\
                 port = /dev/ttyUSB1\n\
                 speed = 9600\n\
               [[Pipe]]\n\
                 type = PipeInterface\n\
                 enabled = Yes\n\
                 command = example\n\
               [[Radio]]\n\
                 type = RNodeInterface\n\
                 enabled = Yes\n\
                 port = /dev/ttyUSB2\n\
                 frequency = 868000000\n\
                 bandwidth = 125000\n\
                 txpower = 7\n\
                 spreadingfactor = 8\n\
                 codingrate = 5\n",
    );

    assert_eq!(named(&plan, "Serial").policy.bitrate.get(), 115_200);
    assert_eq!(named(&plan, "Kiss").policy.bitrate.get(), 1_200);
    assert_eq!(named(&plan, "Pipe").policy.bitrate.get(), 1_000_000);
    assert_eq!(named(&plan, "Radio").policy.bitrate.get(), 3_125);
}

#[test]
fn common_and_medium_settings_are_applied() {
    let plan = plan_of(
        "[interfaces]\n\
               [[Hub]]\n\
                 type = TCPClientInterface\n\
                 enabled = Yes\n\
                 target_host = h\n\
                 target_port = 1\n\
                 mode = gateway\n\
                 announce_cap = 5.0\n\
                 announce_rate_target = 3600\n\
                 network_name = secret-net\n\
                 kiss_framing = Yes\n",
    );
    let hub = named(&plan, "Hub");
    assert_eq!(hub.policy.mode, InterfaceMode::Gateway);
    assert_eq!(
        hub.policy.announce_bandwidth_cap,
        AnnounceBandwidthCap::Limited { cap_per_mille: 50 }
    );
    assert_eq!(
        hub.policy.announce_rate_limit,
        Some(AnnounceRateLimit {
            target_ms: 3_600_000,
            grace: 0,
            penalty_ms: 0,
        })
    );
    assert_eq!(
        hub.access,
        InterfaceAccessPlan::Ifac {
            network_name: Some("secret-net".to_string()),
            passphrase: None,
            size: IfacSize::WIDE,
        }
    );
    assert!(matches!(
        hub.medium,
        PlannedMedium::TcpClient {
            framing: TcpWireFraming::Kiss,
            ..
        }
    ));
}

#[test]
fn ifac_defaults_follow_the_reference_mediums() {
    let plan = plan_of(
            "[interfaces]\n[[Internet]]\ntype = TCPClientInterface\nenabled = Yes\ntarget_host = h\ntarget_port = 1\nnetwork_name = n\n\
             [[Radio]]\ntype = SerialInterface\nenabled = Yes\nport = /dev/ttyUSB0\npassphrase = p\n",
        );
    assert!(matches!(
        named(&plan, "Internet").access,
        InterfaceAccessPlan::Ifac {
            size: IfacSize::WIDE,
            ..
        }
    ));
    assert!(matches!(
        named(&plan, "Radio").access,
        InterfaceAccessPlan::Ifac {
            size: IfacSize::NARROW,
            ..
        }
    ));
}

#[test]
fn ifac_size_is_a_bit_count_floored_to_whole_bytes() {
    let plan = plan_of(
            "[interfaces]\n[[Seven]]\ntype = TCPClientInterface\nenabled = Yes\ntarget_host = h\ntarget_port = 1\nnetwork_name = n\nifac_size = 7\n\
             [[SeventyOne]]\ntype = TCPClientInterface\nenabled = Yes\ntarget_host = h\ntarget_port = 2\nnetwork_name = n\nifac_size = 71\n\
             [[FiveNineteen]]\ntype = TCPClientInterface\nenabled = Yes\ntarget_host = h\ntarget_port = 3\nnetwork_name = n\nifac_size = 519\n",
        );
    assert!(matches!(
        named(&plan, "Seven").access,
        InterfaceAccessPlan::Ifac {
            size: IfacSize::WIDE,
            ..
        }
    ));
    assert!(matches!(
        named(&plan, "SeventyOne").access,
        InterfaceAccessPlan::Ifac { size, .. } if size.bytes() == 8
    ));
    assert!(matches!(
        named(&plan, "FiveNineteen").access,
        InterfaceAccessPlan::Ifac {
            size: IfacSize::MAX,
            ..
        }
    ));
}

#[test]
fn an_oversize_ifac_fails_before_a_plan_is_returned() {
    let protected = parse_and_plan(
            "[interfaces]\n[[TooWide]]\ntype = TCPClientInterface\nenabled = Yes\ntarget_host = h\ntarget_port = 1\nnetwork_name = n\nifac_size = 520\n",
        )
        .expect_err("oversize IFAC is invalid");
    assert_eq!(
        protected.diagnostics()[0].code(),
        ConfigDiagnosticCode::InvalidValue
    );
    let open = parse_and_plan(
            "[interfaces]\n[[Open]]\ntype = TCPClientInterface\nenabled = Yes\ntarget_host = h\ntarget_port = 1\nifac_size = 520\n",
        )
        .expect_err("unused invalid IFAC is still invalid config");
    assert_eq!(
        open.diagnostics()[0].code(),
        ConfigDiagnosticCode::InvalidValue
    );
}
