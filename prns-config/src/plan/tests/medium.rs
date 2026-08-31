use super::*;

#[test]
fn every_host_constructible_medium_maps() {
    let plan = plan_of(STOCK);
    assert_eq!(plan.interfaces.len(), 5);
    let PlannedMedium::AutoWifi(auto) = &named(&plan, "Default Interface").medium else {
        panic!("AutoInterface medium expected")
    };
    assert_eq!(auto.group_id().as_str(), "reticulum");
    assert_eq!(auto.discovery_scope(), AutoInterfaceDiscoveryScope::Link);
    assert_eq!(auto.discovery_port().get(), 29_716);
    assert_eq!(auto.discovery_port().reverse_discovery_port(), 29_717);
    assert_eq!(auto.data_port().get(), 42_671);
    assert!(auto.devices().allowed().is_empty());
    assert!(auto.devices().ignored().is_empty());
    assert_eq!(
        auto.multicast_address_type(),
        AutoInterfaceMulticastAddressType::Temporary,
    );
    assert_eq!(
        named(&plan, "Hub").medium,
        PlannedMedium::TcpClient {
            connection: tcp_dial("hub.example.com", 4965),
            framing: TcpWireFraming::Hdlc,
        }
    );
    assert_eq!(
        named(&plan, "Listener").medium,
        PlannedMedium::TcpServer {
            listener: tcp_listener(TcpListenHost::Address("0.0.0.0".to_string()), 4242),
            framing: TcpWireFraming::Hdlc,
        }
    );
    assert_eq!(
        named(&plan, "Mesh").medium,
        PlannedMedium::Udp {
            flow: UdpFlowPlan::Bidirectional {
                listen: udp_address("0.0.0.0", 4848),
                forward: udp_address("255.255.255.255", 4848),
            },
        }
    );
    assert_eq!(
        named(&plan, "Modem").medium,
        PlannedMedium::Serial {
            device: "/dev/ttyUSB0".to_string(),
            line: serial_line_plan(115_200),
        }
    );
}

#[test]
fn prns_owned_host_interfaces_reach_typed_plans() {
    let plan = plan_of(
        "[interfaces]\n\
         [[USB]]\ntype = prnsusbautointerface\nenabled = Yes\nbitrate = 7654321\n\
         [[BLE]]\ntype = PrnsBleAuto\nenabled = Yes\nbitrate = 7654321\n\
         [[WebSocket Client]]\ntype = PRNSWEBSOCKETCLIENT\nenabled = Yes\ntarget = wss://peer.example/prns\nframing = kiss\nbitrate = 7654321\n\
         [[WebSocket Server]]\ntype = PrnsWebSocketServerInterface\nenabled = Yes\nlisten_ip = ::1\nlisten_port = 4243\nprefer_ipv6 = Yes\nframing = hdlc\nbitrate = 7654321\n",
    );

    assert!(matches!(
        named(&plan, "USB").medium,
        PlannedMedium::PrnsUsbAuto
    ));
    assert!(matches!(
        named(&plan, "BLE").medium,
        PlannedMedium::PrnsBluetoothAuto { .. }
    ));
    let PlannedMedium::PrnsWebSocketClient { target, framing } =
        &named(&plan, "WebSocket Client").medium
    else {
        panic!("Prns WebSocket client medium expected")
    };
    assert_eq!(target.as_str(), "wss://peer.example/prns");
    assert_eq!(
        *framing,
        prns_core::interfaces::websocket::WebSocketFramingSelection::Fixed(
            prns_core::interfaces::websocket::WebSocketWireFraming::Kiss,
        )
    );
    assert_eq!(
        named(&plan, "WebSocket Server").medium,
        PlannedMedium::PrnsWebSocketServer {
            listener: TcpListenPlan {
                host: TcpListenHost::Address("::1".to_string()),
                port: 4243,
                address_family: AddressFamilyPreference::Ipv6,
                tunnel: TcpTunnelMode::Direct,
            },
            framing: prns_core::interfaces::websocket::WebSocketFramingSelection::Fixed(
                prns_core::interfaces::websocket::WebSocketWireFraming::Hdlc,
            ),
        }
    );
    for interface in &plan.interfaces {
        assert_eq!(
            interface.policy.bitrate,
            BitrateBps::new(7_654_321).unwrap()
        );
    }
}

#[test]
fn prns_bluetooth_auto_group_id_defaults_and_overrides() {
    let defaulted = plan_of("[interfaces]\n[[BLE]]\ntype = PrnsBluetoothAuto\nenabled = Yes\n");
    let PlannedMedium::PrnsBluetoothAuto { group_id } = &named(&defaulted, "BLE").medium else {
        panic!("PrnsBluetoothAuto medium expected")
    };
    assert_eq!(group_id.as_str(), "reticulum");

    let overridden = plan_of(
        "[interfaces]\n[[BLE]]\ntype = PrnsBluetoothAuto\nenabled = Yes\ngroup_id = mt-leg-a\n",
    );
    let PlannedMedium::PrnsBluetoothAuto { group_id } = &named(&overridden, "BLE").medium else {
        panic!("PrnsBluetoothAuto medium expected")
    };
    assert_eq!(group_id.as_str(), "mt-leg-a");
}

#[test]
fn websocket_framing_defaults_to_automatic_selection() {
    let plan = plan_of(
        "[interfaces]\n\
         [[WebSocket Client]]\ntype = PrnsWebSocketClient\nenabled = Yes\ntarget = ws://peer.example/prns\n\
         [[WebSocket Server]]\ntype = PrnsWebSocketServer\nenabled = Yes\nport = 4243\n",
    );

    assert!(matches!(
        named(&plan, "WebSocket Client").medium,
        PlannedMedium::PrnsWebSocketClient {
            framing: prns_core::interfaces::websocket::WebSocketFramingSelection::Auto,
            ..
        }
    ));
    assert!(matches!(
        named(&plan, "WebSocket Server").medium,
        PlannedMedium::PrnsWebSocketServer {
            framing: prns_core::interfaces::websocket::WebSocketFramingSelection::Auto,
            ..
        }
    ));
}

#[test]
fn auto_interface_settings_and_bootstrap_lifecycle_are_fully_typed() {
    let plan = plan_of(
        "[interfaces]\n[[Field LAN]]\ntype = AutoInterface\nenabled = Yes\n\
         bootstrap_only = Yes\ngroup_id = field-team\ndiscovery_scope = organisation\n\
         discovery_port = 30100\ndata_port = 30200\ndevices = en0, eth0\n\
         ignored_devices = eth0\nmulticast_address_type = permanent\n",
    );
    let interface = named(&plan, "Field LAN");
    let PlannedMedium::AutoWifi(auto) = &interface.medium else {
        panic!("AutoInterface medium expected")
    };

    assert_eq!(
        interface.lifecycle,
        ConfiguredInterfaceLifecycle::BootstrapOnly,
    );
    assert_eq!(auto.group_id().as_str(), "field-team");
    assert_eq!(
        auto.discovery_scope(),
        AutoInterfaceDiscoveryScope::Organisation,
    );
    assert_eq!(auto.discovery_port().get(), 30_100);
    assert_eq!(auto.discovery_port().reverse_discovery_port(), 30_101);
    assert_eq!(auto.data_port().get(), 30_200);
    assert_eq!(auto.devices().allowed(), ["en0", "eth0"]);
    assert_eq!(auto.devices().ignored(), ["eth0"]);
    assert_eq!(
        auto.multicast_address_type(),
        AutoInterfaceMulticastAddressType::Permanent,
    );
}

#[test]
fn invalid_auto_interface_values_report_exact_source_corrections() {
    for (line, key, value, accepted) in [
        (5, "discovery_scope", "neighborhood", "organisation"),
        (5, "multicast_address_type", "stable", "temporary"),
        (5, "discovery_port", "65535", "65534"),
        (5, "data_port", "0", "65535"),
    ] {
        let errors = parse_and_plan_named(
            "/etc/reticulum/config",
            &format!(
                "[interfaces]\n[[LAN]]\ntype = AutoInterface\nenabled = Yes\n{key} = {value}\n"
            ),
        )
        .expect_err("invalid AutoInterface setting must fail");
        let diagnostic = &errors.diagnostics()[0];

        assert_eq!(diagnostic.code(), ConfigDiagnosticCode::InvalidValue);
        assert_eq!(diagnostic.source(), "/etc/reticulum/config");
        assert_eq!(diagnostic.line(), line);
        assert_eq!(diagnostic.path(), format!("[interfaces] > [[LAN]] > {key}"));
        assert_eq!(diagnostic.value(), Some(value));
        assert!(diagnostic
            .accepted()
            .is_some_and(|forms| forms.contains(accepted)));
    }
}

#[test]
fn tcp_socket_settings_are_typed_into_the_plan() {
    let plan = plan_of(
            "[interfaces]\n\
             [[Client]]\ntype = TCPClientInterface\nenabled = Yes\ntarget_host = peer\ntarget_port = 4242\n\
             i2p_tunneled = Yes\nconnect_timeout = 11\nmax_reconnect_tries = 3\n\
             [[Server]]\ntype = TCPServerInterface\nenabled = Yes\nport = 4243\nprefer_ipv6 = Yes\n\
             i2p_tunneled = Yes\nkiss_framing = Yes\n",
        );
    assert_eq!(
        named(&plan, "Client").medium,
        PlannedMedium::TcpClient {
            connection: TcpDialPlan {
                host: "peer".to_string(),
                port: 4242,
                connect_timeout: ConnectTimeoutSeconds::new(11),
                reconnect_limit: ReconnectLimit::Attempts(3),
                address_family: AddressFamilyPreference::System,
                tunnel: TcpTunnelMode::I2p,
            },
            framing: TcpWireFraming::Hdlc,
        }
    );
    assert_eq!(
        named(&plan, "Server").medium,
        PlannedMedium::TcpServer {
            listener: TcpListenPlan {
                host: TcpListenHost::Any,
                port: 4243,
                address_family: AddressFamilyPreference::Ipv6,
                tunnel: TcpTunnelMode::I2p,
            },
            framing: TcpWireFraming::Kiss,
        }
    );
}

#[test]
fn a_kiss_tnc_plans_on_its_serial_device_with_reference_tnc_defaults() {
    let plan = plan_of(
            "[interfaces]\n[[TNC]]\ntype = KISSInterface\nenabled = Yes\nport = /dev/ttyUSB0\nspeed = 115200\n",
        );
    assert_eq!(
        named(&plan, "TNC").medium,
        PlannedMedium::Kiss {
            device: "/dev/ttyUSB0".to_string(),
            line: serial_line_plan(115_200),
            preamble_ms: 350,
            txtail_ms: 20,
            persistence: 64,
            slottime_ms: 20,
            flow_control: ReadyCommandFlowControl::Disabled,
            station_id: None,
        }
    );
}

#[test]
fn a_kiss_tnc_carries_configured_timing_flow_control_and_station_id() {
    let plan = plan_of(
        "[interfaces]\n[[TNC]]\ntype = KISSInterface\nenabled = Yes\nport = /dev/ttyUSB0\n\
             preamble = 150\ntxtail = 50\npersistence = 200\nslottime = 30\nflow_control = Yes\n\
             id_callsign = N0CALL\nid_interval = 600\n",
    );
    let tnc = named(&plan, "TNC");
    assert_eq!(
        tnc.medium,
        PlannedMedium::Kiss {
            device: "/dev/ttyUSB0".to_string(),
            line: serial_line_plan(RNS_DEFAULT_SERIAL_BAUD),
            preamble_ms: 150,
            txtail_ms: 50,
            persistence: 200,
            slottime_ms: 30,
            flow_control: ReadyCommandFlowControl::Enabled,
            station_id: Some(StationIdentificationPlan {
                callsign: "N0CALL".to_string(),
                interval_seconds: 600,
            }),
        }
    );
}

#[test]
fn an_ax25_tnc_plans_with_its_callsign_ssid_and_tnc_defaults() {
    let plan = plan_of(
        "[interfaces]\n[[Packet]]\ntype = AX25KISSInterface\nenabled = Yes\nport = /dev/ttyUSB0\n\
             callsign = N0CALL\nssid = 2\n",
    );
    assert_eq!(
        named(&plan, "Packet").medium,
        PlannedMedium::Ax25Kiss {
            device: "/dev/ttyUSB0".to_string(),
            line: serial_line_plan(RNS_DEFAULT_SERIAL_BAUD),
            preamble_ms: 350,
            txtail_ms: 20,
            persistence: 64,
            slottime_ms: 20,
            flow_control: ReadyCommandFlowControl::Disabled,
            callsign: "N0CALL".to_string(),
            ssid: 2,
        }
    );
}

#[test]
fn an_ax25_tnc_without_a_callsign_or_ssid_is_invalid() {
    let no_call = parse(
            "[interfaces]\n[[Packet]]\ntype = AX25KISSInterface\nenabled = Yes\nport = /dev/ttyUSB0\nssid = 0\n",
        );
    assert!(no_call.is_err());
    let no_ssid = parse(
            "[interfaces]\n[[Packet]]\ntype = AX25KISSInterface\nenabled = Yes\nport = /dev/ttyUSB0\ncallsign = N0CALL\n",
        );
    assert!(no_ssid.is_err());
}

#[test]
fn a_pipe_plans_with_its_command_and_the_default_respawn_delay() {
    let plan = plan_of(
        "[interfaces]\n[[Subprocess]]\ntype = PipeInterface\nenabled = Yes\ncommand = nc -l 4242\n",
    );
    assert_eq!(
        named(&plan, "Subprocess").medium,
        PlannedMedium::Pipe {
            command: PipeCommandPlan {
                source: "nc -l 4242".to_string(),
                argv: vec!["nc".to_string(), "-l".to_string(), "4242".to_string()],
            },
            respawn_delay: PipeRespawnDelay(Duration::from_secs(5)),
        }
    );
}

#[test]
fn a_pipe_respawn_delay_is_read_in_seconds() {
    let plan = plan_of(
            "[interfaces]\n[[Subprocess]]\ntype = PipeInterface\nenabled = Yes\ncommand = prog\nrespawn_delay = 2.5\n",
        );
    assert_eq!(
        named(&plan, "Subprocess").medium,
        PlannedMedium::Pipe {
            command: PipeCommandPlan {
                source: "prog".to_string(),
                argv: vec!["prog".to_string()],
            },
            respawn_delay: PipeRespawnDelay(Duration::from_millis(2_500)),
        }
    );
}

#[test]
fn a_pipe_without_a_command_is_invalid() {
    assert!(parse("[interfaces]\n[[Subprocess]]\ntype = PipeInterface\nenabled = Yes\n").is_err());
}

#[test]
fn a_backbone_listener_plans_on_its_bind_address() {
    let plan = plan_of(
        "[interfaces]\n[[Spine]]\ntype = BackboneInterface\nenabled = Yes\n\
             listen_ip = 0.0.0.0\nlisten_port = 4242\n",
    );
    assert_eq!(
        named(&plan, "Spine").medium,
        PlannedMedium::Backbone {
            listener: tcp_listener(TcpListenHost::Address("0.0.0.0".to_string()), 4242),
        }
    );
}

#[test]
fn a_backbone_listener_defaults_its_ip_and_accepts_the_port_alias() {
    let plan = plan_of(
        "[interfaces]\n[[Spine]]\ntype = BackboneInterface\nenabled = Yes\n\
             port = 5959\n",
    );
    assert_eq!(
        named(&plan, "Spine").medium,
        PlannedMedium::Backbone {
            listener: tcp_listener(TcpListenHost::Any, 5959),
        }
    );
}

#[test]
fn a_backbone_client_plans_on_its_target() {
    let plan = plan_of(
        "[interfaces]\n[[Uplink]]\ntype = BackboneClientInterface\nenabled = Yes\n\
             target_host = spine.example.com\ntarget_port = 4242\n",
    );
    assert_eq!(
        named(&plan, "Uplink").medium,
        PlannedMedium::BackboneClient {
            connection: TcpDialPlan {
                address_family: AddressFamilyPreference::Ipv4,
                ..tcp_dial("spine.example.com", 4242)
            },
        }
    );
}

#[test]
fn backbone_remote_alias_selects_the_client_role_on_the_listener_type() {
    let plan = plan_of(
        "[interfaces]\n[[Uplink]]\ntype = BackboneInterface\nenabled = Yes\n\
             remote = spine.example.com\nport = 4242\nprefer_ipv6 = Yes\n",
    );
    assert_eq!(
        named(&plan, "Uplink").medium,
        PlannedMedium::BackboneClient {
            connection: TcpDialPlan {
                host: "spine.example.com".to_string(),
                port: 4242,
                connect_timeout: ConnectTimeoutSeconds::new(5),
                reconnect_limit: ReconnectLimit::Unlimited,
                address_family: AddressFamilyPreference::Ipv6,
                tunnel: TcpTunnelMode::Direct,
            }
        }
    );
}

#[test]
fn a_backbone_listener_without_a_port_is_invalid() {
    let invalid = parse(
        "[interfaces]\n[[Spine]]\ntype = BackboneInterface\nenabled = Yes\nlisten_ip = 0.0.0.0\n",
    );
    assert!(invalid.is_err());
}

#[test]
fn a_backbone_client_without_a_target_is_invalid() {
    let no_host = parse(
            "[interfaces]\n[[Uplink]]\ntype = BackboneClientInterface\nenabled = Yes\ntarget_port = 4242\n",
        );
    assert!(no_host.is_err());
    let no_port = parse(
            "[interfaces]\n[[Uplink]]\ntype = BackboneClientInterface\nenabled = Yes\ntarget_host = spine\n",
        );
    assert!(no_port.is_err());
}

#[test]
fn backbone_host_options_are_fully_planned() {
    let listener = plan_of(
        "[interfaces]\n[[Spine]]\ntype = BackboneInterface\nenabled = Yes\n\
             listen_port = 4242\ndevice = eth0\nprefer_ipv6 = Yes\n",
    );
    let spine = named(&listener, "Spine");
    assert_eq!(
        spine.medium,
        PlannedMedium::Backbone {
            listener: TcpListenPlan {
                host: TcpListenHost::Device("eth0".to_string()),
                port: 4242,
                address_family: AddressFamilyPreference::Ipv6,
                tunnel: TcpTunnelMode::Direct,
            }
        }
    );

    let client = plan_of(
        "[interfaces]\n[[Uplink]]\ntype = BackboneClientInterface\nenabled = Yes\n\
             target_host = spine\ntarget_port = 4242\ni2p_tunneled = Yes\nconnect_timeout = 10\n\
             max_reconnect_tries = 3\n",
    );
    let uplink = named(&client, "Uplink");
    assert_eq!(
        uplink.medium,
        PlannedMedium::BackboneClient {
            connection: TcpDialPlan {
                host: "spine".to_string(),
                port: 4242,
                connect_timeout: ConnectTimeoutSeconds::new(10),
                reconnect_limit: ReconnectLimit::Attempts(3),
                address_family: AddressFamilyPreference::Ipv4,
                tunnel: TcpTunnelMode::I2p,
            }
        }
    );
}

#[test]
fn a_disabled_interface_is_skipped_before_planning() {
    let plan = plan_of(
        "[interfaces]\n[[Off]]\ntype = TCPClientInterface\ntarget_host = h\ntarget_port = 1\n",
    );
    assert!(plan.interfaces.is_empty());
}

#[test]
fn a_missing_required_field_is_invalid_before_planning() {
    let invalid =
        parse("[interfaces]\n[[Hub]]\ntype = TCPClientInterface\nenabled = Yes\ntarget_host = h\n");
    assert!(invalid.is_err());
}

#[test]
fn a_weave_interface_plans_its_serial_device() {
    let plan = plan_of(
        "[interfaces]\n[[Mesh]]\ntype = WeaveInterface\nenabled = Yes\nport = /dev/ttyACM0\n",
    );
    assert_eq!(
        named(&plan, "Mesh").medium,
        PlannedMedium::Weave {
            device: "/dev/ttyACM0".to_string(),
        }
    );
}

#[test]
fn a_weave_interface_requires_a_serial_device() {
    let errors =
        parse("[interfaces]\n[[Mesh]]\ntype = WeaveInterface\nenabled = Yes\n").unwrap_err();
    assert!(errors.diagnostics().iter().any(|diagnostic| {
        diagnostic.code() == crate::ConfigDiagnosticCode::MissingRequiredKey
            && diagnostic.path().ends_with(" > port")
    }));
}

#[test]
fn an_rnode_plans_with_its_radio_channel_and_scales_its_airtime_locks() {
    let plan = plan_of(
        "[interfaces]\n[[Radio]]\ntype = RNodeInterface\nenabled = Yes\nport = /dev/ttyUSB0\n\
             frequency = 868000000\nbandwidth = 125000\ntxpower = 7\nspreadingfactor = 8\n\
             codingrate = 5\nairtime_limit_short = 1.5\nairtime_limit_long = 5.0\n",
    );
    let PlannedMedium::Rnode {
        transport,
        frequency_hz,
        bandwidth_hz,
        tx_power_dbm,
        spreading_factor,
        coding_rate,
        flow_control,
        station_id,
        airtime_limit_short,
        airtime_limit_long,
    } = &named(&plan, "Radio").medium
    else {
        panic!("RNode medium expected")
    };
    let RNodeTransportPlan::Serial(device) = transport else {
        panic!("serial RNode transport expected")
    };
    assert_eq!(device.as_str(), "/dev/ttyUSB0");
    assert_eq!(*frequency_hz, 868_000_000);
    assert_eq!(*bandwidth_hz, 125_000);
    assert_eq!(*tx_power_dbm, 7);
    assert_eq!(*spreading_factor, 8);
    assert_eq!(*coding_rate, 5);
    assert_eq!(*flow_control, ReadyCommandFlowControl::Disabled);
    assert_eq!(*station_id, None);
    assert_eq!(*airtime_limit_short, Some(AirtimeLimitCentiPercent(150)));
    assert_eq!(*airtime_limit_long, Some(AirtimeLimitCentiPercent(500)));
}

#[test]
fn an_rnode_tcp_uri_plans_the_fixed_stock_endpoint() {
    let plan = plan_of(
        "[interfaces]\n[[Radio]]\ntype = RNodeInterface\nenabled = Yes\nport = tcp://radio.example\n\
             frequency = 868000000\nbandwidth = 125000\ntxpower = 7\nspreadingfactor = 8\n\
             codingrate = 5\n",
    );
    let PlannedMedium::Rnode { transport, .. } = &named(&plan, "Radio").medium else {
        panic!("RNode medium expected")
    };
    let RNodeTransportPlan::Tcp(target) = transport else {
        panic!("TCP RNode transport expected")
    };
    assert_eq!(target.socket_target(), "radio.example:7633");
    assert_eq!(transport.channel_tag(), b"tcp://radio.example");
}

#[test]
fn rnode_ble_uris_plan_automatic_name_and_address_targets() {
    let plan_for = |port: &str| {
        plan_of(&format!(
            "[interfaces]\n[[Radio]]\ntype = RNodeInterface\nenabled = Yes\nport = {port}\n\
             frequency = 868000000\nbandwidth = 125000\ntxpower = 7\nspreadingfactor = 8\n\
             codingrate = 5\n"
        ))
    };

    let automatic = plan_for("ble://");
    let PlannedMedium::Rnode { transport, .. } = &named(&automatic, "Radio").medium else {
        panic!("RNode medium expected")
    };
    assert_eq!(
        transport,
        &RNodeTransportPlan::Ble(RNodeBleTarget::FirstBondedRnode)
    );

    let named_target = plan_for("ble://RNode 1234");
    let PlannedMedium::Rnode { transport, .. } = &named(&named_target, "Radio").medium else {
        panic!("RNode medium expected")
    };
    let RNodeTransportPlan::Ble(RNodeBleTarget::Name(name)) = transport else {
        panic!("named BLE target expected")
    };
    assert_eq!(name.as_str(), "RNode 1234");

    let addressed = plan_for("ble://AA:BB:CC:DD:EE:FF");
    let PlannedMedium::Rnode { transport, .. } = &named(&addressed, "Radio").medium else {
        panic!("RNode medium expected")
    };
    let RNodeTransportPlan::Ble(RNodeBleTarget::Address(address)) = transport else {
        panic!("addressed BLE target expected")
    };
    assert_eq!(address.octets(), [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF]);
}

#[test]
fn an_rnode_without_a_radio_field_is_invalid() {
    let no_freq = parse(
        "[interfaces]\n[[Radio]]\ntype = RNodeInterface\nenabled = Yes\nport = /dev/ttyUSB0\n\
             bandwidth = 125000\ntxpower = 7\nspreadingfactor = 8\ncodingrate = 5\n",
    );
    assert!(no_freq.is_err());
    let no_sf = parse(
        "[interfaces]\n[[Radio]]\ntype = RNodeInterface\nenabled = Yes\nport = /dev/ttyUSB0\n\
             frequency = 868000000\nbandwidth = 125000\ntxpower = 7\ncodingrate = 5\n",
    );
    assert!(no_sf.is_err());
}

#[test]
fn an_rnode_plans_flow_control_and_station_identification() {
    let plan = plan_of(
        "[interfaces]\n[[Radio]]\ntype = RNodeInterface\nenabled = Yes\nport = /dev/ttyUSB0\n\
             frequency = 868000000\nbandwidth = 125000\ntxpower = 7\nspreadingfactor = 8\n\
             codingrate = 5\nflow_control = Yes\nid_callsign = N0CALL\nid_interval = 600\n",
    );
    let radio = named(&plan, "Radio");
    let PlannedMedium::Rnode {
        flow_control,
        station_id,
        ..
    } = &radio.medium
    else {
        panic!("RNode medium expected")
    };
    assert_eq!(*flow_control, ReadyCommandFlowControl::Enabled);
    assert_eq!(
        station_id.as_ref(),
        Some(&StationIdentificationPlan {
            callsign: "N0CALL".to_string(),
            interval_seconds: 600,
        })
    );
}

#[test]
fn i2p_settings_become_one_effective_typed_plan() {
    let plan = plan_of(
        "[interfaces]\n\
               [[Private I2P]]\n\
                 type = I2PInterface\n\
                 enabled = Yes\n\
                 peers = example.i2p, QUJDRA==\n\
                 connectable = Yes\n\
                 outgoing = No\n\
                 bitrate = 500000\n\
                 network_name = private-overlay\n",
    );
    let interface = named(&plan, "Private I2P");
    let PlannedMedium::I2p {
        peers,
        reachability,
    } = &interface.medium
    else {
        panic!("I2P medium expected")
    };

    assert_eq!(
        peers.iter().map(I2pPeerPlan::as_str).collect::<Vec<_>>(),
        vec!["example.i2p", "QUJDRA=="]
    );
    assert_eq!(*reachability, I2pReachabilityPlan::Connectable);
    assert_eq!(interface.policy.bitrate.get(), 500_000);
    assert_eq!(
        interface.policy.mtu.resolve(interface.policy.bitrate),
        Some(1_064)
    );
    assert_eq!(
        interface.policy.capabilities.egress,
        EgressCapability::Disabled
    );
    assert!(matches!(
        interface.access,
        InterfaceAccessPlan::Ifac {
            size: IfacSize::WIDE,
            ..
        }
    ));
}

#[test]
fn i2p_omissions_are_outbound_only_with_stock_defaults() {
    let plan = plan_of("[interfaces]\n[[Private I2P]]\ntype = I2PInterface\nenabled = Yes\n");
    let interface = named(&plan, "Private I2P");
    let PlannedMedium::I2p {
        peers,
        reachability,
    } = &interface.medium
    else {
        panic!("I2P medium expected")
    };

    assert!(peers.is_empty());
    assert_eq!(*reachability, I2pReachabilityPlan::OutboundOnly);
    assert_eq!(interface.policy.bitrate.get(), 256_000);
    assert_eq!(
        interface.policy.mtu.resolve(interface.policy.bitrate),
        Some(1_064)
    );
}

#[test]
fn invalid_i2p_peers_have_source_keyed_corrections() {
    let errors = parse_and_plan_named(
            "/etc/reticulum/config",
            "[interfaces]\n[[Private I2P]]\ntype = I2PInterface\nenabled = Yes\npeers = one.i2p, two.i2p, one.i2p\n",
        )
        .expect_err("duplicate I2P peers are invalid");
    let diagnostic = &errors.diagnostics()[0];

    assert_eq!(diagnostic.code(), ConfigDiagnosticCode::InvalidValue);
    assert_eq!(diagnostic.source(), "/etc/reticulum/config");
    assert_eq!(diagnostic.line(), 5);
    assert_eq!(diagnostic.path(), "[interfaces] > [[Private I2P]] > peers");
    assert!(diagnostic
        .message()
        .contains("I2P peer 3 duplicates peer 1"));
    assert!(diagnostic
        .accepted()
        .is_some_and(|accepted| accepted.contains(".i2p names")));
    assert_eq!(
        diagnostic.correction(),
        "set `peers = example.i2p, QUJDRA==`"
    );
}

#[test]
fn disabled_i2p_stanzas_do_not_validate_unused_peer_settings() {
    let plan = plan_of(
            "[interfaces]\n[[Dormant I2P]]\ntype = I2PInterface\nenabled = No\npeers = not a destination\n",
        );

    assert!(plan.interfaces.is_empty());
}
